mod custom;
mod datadog;
mod incidentio;
mod instatus;
mod statuspage;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use reqwest::Client;
use url::Url;

use crate::config::{CustomMapping, Impact, StatusProvider};
use crate::fetch::{FetchError, Fetched, fetch_conditional};
use crate::notifier::{DiscordMessage, Embed};
use crate::rss::{FEED_ACCEPT, RssError};
use crate::runtime::RunStatusPage;

/// Appended to an Atlassian Statuspage base URL. `incidents.json` is used rather
/// than `summary.json` because summary omits resolved incidents, and "resolved" is
/// the single most useful update to forward.
pub const STATUSPAGE_PATH: &str = "api/v2/incidents.json";
/// Instatus v3: unlike the legacy `/summary.json`, it carries `lastUpdate` and the
/// recently resolved incidents.
pub const INSTATUS_PATH: &str = "v3/summary.json";
/// Why not incident.io's widget API: it is opt-in per page and its schema is shown
/// only in the page's admin UI. The Atom feed is public on every page.
pub const INCIDENTIO_PATH: &str = "feed.atom";
/// The document the Datadog Status Pages site itself renders from; the product
/// documents no public API, and its RSS exists only when the page enables it.
pub const DATADOG_PATH: &str = "config.json";

const COLOR_RESOLVED: u32 = 0x57F287;
const COLOR_CRITICAL: u32 = 0xED4245;
const COLOR_MAJOR: u32 = 0xE67E22;
const COLOR_MINOR: u32 = 0xFEE75C;
const COLOR_NEUTRAL: u32 = 0x99AAB5;

#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("status page returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("response does not decode as {kind}: {source}")]
    Decode {
        kind: &'static str,
        #[source]
        source: DecodeError,
    },
    #[error("status page body exceeded {limit} bytes")]
    TooLarge { limit: usize },
}

impl From<FetchError> for StatusError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Request(e) => StatusError::Request(e),
            FetchError::Status { status, body } => StatusError::Status { status, body },
            FetchError::TooLarge { limit } => StatusError::TooLarge { limit },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Feed(#[from] RssError),
    #[error(transparent)]
    Mapping(#[from] custom::MappingError),
}

/// How a fetched body becomes incidents. Adding a provider is a new variant here
/// plus the `StatusProvider` that selects it; the poller never learns the difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoder {
    Statuspage,
    Instatus,
    IncidentIo,
    Datadog,
    Custom(CustomMapping),
}

impl Decoder {
    /// Sent as `Accept`. The feed needs the feed media types or a server may 406.
    pub fn accept(&self) -> &'static str {
        match self {
            Decoder::IncidentIo => FEED_ACCEPT,
            _ => "application/json",
        }
    }

    /// Names the provider in the decode error.
    pub fn label(&self) -> &'static str {
        match self {
            Decoder::Statuspage => "Atlassian Statuspage",
            Decoder::Instatus => "Instatus",
            Decoder::IncidentIo => "incident.io",
            Decoder::Datadog => "Datadog Status Pages",
            Decoder::Custom(_) => "custom JSON",
        }
    }

    /// Pure. `page_url` is the configured URL (Datadog builds incident links from
    /// it), `api_url` the fetched one (incident.io's feed base), and `now` the
    /// timestamp for an update whose body carries none.
    pub fn decode(
        &self,
        body: &[u8],
        page_url: &Url,
        api_url: &Url,
        now: DateTime<Utc>,
    ) -> Result<Vec<Incident>, DecodeError> {
        match self {
            Decoder::Statuspage => statuspage::decode(body),
            Decoder::Instatus => instatus::decode(body),
            Decoder::IncidentIo => incidentio::decode(body, api_url, now),
            Decoder::Datadog => datadog::decode(body, page_url),
            Decoder::Custom(mapping) => custom::decode(mapping, body, now),
        }
    }
}

/// Where a status page is fetched from and how its body is read, settled once at
/// resolve time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    pub api_url: Url,
    pub decoder: Decoder,
}

pub fn resolve_source(provider: &StatusProvider, url: &Url) -> Result<SourceSpec, url::ParseError> {
    let (path, decoder) = match provider {
        StatusProvider::Statuspage => (STATUSPAGE_PATH, Decoder::Statuspage),
        StatusProvider::Instatus => (INSTATUS_PATH, Decoder::Instatus),
        StatusProvider::Incidentio => (INCIDENTIO_PATH, Decoder::IncidentIo),
        StatusProvider::Datadog => (DATADOG_PATH, Decoder::Datadog),
        StatusProvider::Custom(mapping) => {
            return Ok(SourceSpec {
                api_url: url.clone(),
                decoder: Decoder::Custom(mapping.clone()),
            });
        }
    };
    Ok(SourceSpec {
        api_url: join_base(url, path)?,
        decoder,
    })
}

/// `url` with `path` appended below it, whether or not `url` ends in `/`.
pub fn join_base(url: &Url, path: &str) -> Result<Url, url::ParseError> {
    // `Url::join` replaces the final path segment unless the base path ends in `/`,
    // which would turn `https://x.example/status` into `https://x.example/api/...`.
    let mut base = url.clone();
    if !base.path().ends_with('/') {
        let dir = format!("{}/", base.path());
        base.set_path(&dir);
    }
    base.join(path)
}

/// Parses a wire timestamp; `None` drops the update it belongs to.
pub(crate) fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncidentState {
    Investigating,
    Identified,
    Monitoring,
    Resolved,
    Postmortem,
    Unknown,
}

impl IncidentState {
    /// Case-insensitive: Instatus spells states in upper case, incident.io in title case.
    fn from_wire(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "investigating" => IncidentState::Investigating,
            "identified" => IncidentState::Identified,
            "monitoring" => IncidentState::Monitoring,
            "resolved" => IncidentState::Resolved,
            "postmortem" => IncidentState::Postmortem,
            _ => IncidentState::Unknown,
        }
    }

    fn from_wire_opt(s: Option<&str>) -> Self {
        s.map_or(IncidentState::Unknown, IncidentState::from_wire)
    }

    pub fn is_resolved(self) -> bool {
        matches!(self, IncidentState::Resolved | IncidentState::Postmortem)
    }

    pub fn label(self) -> &'static str {
        match self {
            IncidentState::Investigating => "Investigating",
            IncidentState::Identified => "Identified",
            IncidentState::Monitoring => "Monitoring",
            IncidentState::Resolved => "Resolved",
            IncidentState::Postmortem => "Postmortem",
            IncidentState::Unknown => "Update",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            IncidentState::Investigating => "🔍",
            IncidentState::Identified => "🎯",
            IncidentState::Monitoring => "👀",
            IncidentState::Resolved => "✅",
            IncidentState::Postmortem => "📄",
            IncidentState::Unknown => "ℹ️",
        }
    }
}

/// An incident from any provider, normalized: timestamps parsed, severities typed.
/// The wire types never leave their provider's module.
#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub id: String,
    pub name: String,
    /// `None` when the provider reported a severity chime does not know, or none.
    pub impact: Option<Impact>,
    pub link: Option<String>,
    pub components: Vec<String>,
    pub updates: Vec<IncidentUpdate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IncidentUpdate {
    pub id: String,
    pub state: IncidentState,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// One notification: the latest update of one incident, at the moment chime first
/// observed it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusEvent {
    pub incident_id: String,
    pub title: String,
    pub body: String,
    pub state: IncidentState,
    pub impact: Option<Impact>,
    pub link: Option<String>,
    pub components: Vec<String>,
    pub occurred_at: DateTime<Utc>,
}

/// Per-page polling state. In-memory only: a restart re-baselines rather than
/// replaying history, matching chime's no-catch-up model for reminders.
#[derive(Debug, Default)]
pub struct PageState {
    seen: HashMap<String, String>,
    initialized: bool,
    pub etag: Option<String>,
}

/// Compare a freshly fetched incident list against what this page has already
/// reported, and return the updates worth sending.
///
/// The `seen` record is written *before* the caller sends anything, so a Discord
/// failure never causes the same update to be re-sent on the next poll — the same
/// rule the reminder path follows.
pub fn diff(state: &mut PageState, incidents: &[Incident], min_impact: Impact) -> Vec<StatusEvent> {
    // Prune only against a non-empty feed. A page may legitimately answer with an
    // empty `incidents` array, and dropping the whole baseline there would re-report
    // every one of the 50 incidents as new the moment the feed came back.
    if !incidents.is_empty() {
        let current: HashSet<&str> = incidents.iter().map(|i| i.id.as_str()).collect();
        state.seen.retain(|id, _| current.contains(id.as_str()));
    }

    let first_poll = !state.initialized;
    state.initialized = true;

    let mut events = Vec::new();
    for incident in incidents {
        let Some(latest) = incident.updates.iter().max_by_key(|u| u.created_at) else {
            continue;
        };
        let previous = state.seen.insert(incident.id.clone(), latest.id.clone());

        // `incidents.json` returns the 50 most recent incidents, so the first poll
        // after startup would otherwise flood the channel with old history.
        if first_poll || previous.as_deref() == Some(latest.id.as_str()) {
            continue;
        }
        if matches!(incident.impact, Some(i) if i < min_impact) {
            continue;
        }
        events.push(StatusEvent {
            incident_id: incident.id.clone(),
            title: incident.name.clone(),
            body: latest.body.clone(),
            state: latest.state,
            impact: incident.impact,
            link: incident.link.clone(),
            components: incident.components.clone(),
            occurred_at: latest.created_at,
        });
    }
    events.sort_by_key(|e| e.occurred_at);
    events
}

#[allow(async_fn_in_trait)]
pub(crate) trait StatusSource {
    async fn fetch(
        &self,
        page: &RunStatusPage,
        etag: Option<&str>,
    ) -> Result<Fetched<Vec<Incident>>, StatusError>;
}

#[derive(Debug, Clone)]
pub struct StatusEndpoint {
    client: Client,
}

impl StatusEndpoint {
    pub fn new(client: Client) -> Self {
        StatusEndpoint { client }
    }
}

impl StatusSource for StatusEndpoint {
    async fn fetch(
        &self,
        page: &RunStatusPage,
        etag: Option<&str>,
    ) -> Result<Fetched<Vec<Incident>>, StatusError> {
        let accept = page.decoder.accept();
        let (bytes, etag) =
            match fetch_conditional(&self.client, &page.api_url, etag, accept).await? {
                Fetched::NotModified => return Ok(Fetched::NotModified),
                Fetched::Modified { value, etag } => (value, etag),
            };
        // Returning the error drops `etag` with it: the scheduler stores an ETag only
        // from `Ok`, so a body chime cannot read never hides behind later 304s.
        let incidents = page
            .decoder
            .decode(&bytes, &page.url, &page.api_url, Utc::now())
            .map_err(|source| StatusError::Decode {
                kind: page.decoder.label(),
                source,
            })?;
        Ok(Fetched::Modified {
            value: incidents,
            etag,
        })
    }
}

pub fn build_message(page: &RunStatusPage, event: &StatusEvent) -> DiscordMessage {
    // A resolved incident is green regardless of how severe it was: showing a red
    // bar on "this is fixed" is the one misread worth designing against.
    let color = if event.state.is_resolved() {
        COLOR_RESOLVED
    } else {
        match event.impact {
            Some(Impact::Critical) => COLOR_CRITICAL,
            Some(Impact::Major) => COLOR_MAJOR,
            Some(Impact::Minor) => COLOR_MINOR,
            _ => COLOR_NEUTRAL,
        }
    };

    let impact_label = event
        .impact
        .map_or_else(|| "Unknown".to_string(), |i| i.to_string());

    let mut embed = Embed::new(&event.title, color)
        .with_description(&event.body)
        .with_field(
            "Status",
            &format!("{} {}", event.state.emoji(), event.state.label()),
            true,
        )
        .with_field("Impact", &impact_label, true);

    if let Some(link) = &event.link {
        embed = embed.with_url(link);
    }
    if !event.components.is_empty() {
        embed = embed.with_field("Components", &event.components.join(", "), false);
    }
    embed = embed
        .with_footer(&page.host)
        .with_timestamp(&event.occurred_at.to_rfc3339());

    page.identity.apply(DiscordMessage::embed(embed))
}

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use test_support::*;

#[cfg(test)]
mod test_support {
    use super::*;

    pub(crate) fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    pub(crate) fn mk_update(id: &str, state: &str, at: &str) -> IncidentUpdate {
        IncidentUpdate {
            id: id.to_string(),
            state: IncidentState::from_wire(state),
            body: format!("body of {id}"),
            created_at: ts(at),
        }
    }

    pub(crate) fn mk_incident(id: &str, impact: Impact, updates: Vec<IncidentUpdate>) -> Incident {
        Incident {
            id: id.to_string(),
            name: format!("incident {id}"),
            impact: Some(impact),
            link: Some(format!("https://stspg.io/{id}")),
            components: vec!["api".to_string()],
            updates,
        }
    }

    /// Drive a state past its cold-start baseline so a test can assert on real diffs.
    pub(crate) fn baselined(incidents: &[Incident]) -> PageState {
        let mut state = PageState::default();
        assert!(diff(&mut state, incidents, Impact::None).is_empty());
        state
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn first_poll_reports_nothing() {
        let incidents = vec![
            mk_incident(
                "a",
                Impact::Critical,
                vec![mk_update("a1", "investigating", "2026-09-01T10:00:00Z")],
            ),
            mk_incident(
                "b",
                Impact::Minor,
                vec![mk_update("b1", "resolved", "2026-09-01T09:00:00Z")],
            ),
        ];
        let mut state = PageState::default();
        assert!(diff(&mut state, &incidents, Impact::None).is_empty());
    }

    #[test]
    fn new_incident_after_baseline_is_reported() {
        let baseline = vec![mk_incident(
            "a",
            Impact::Minor,
            vec![mk_update("a1", "investigating", "2026-09-01T10:00:00Z")],
        )];
        let mut state = baselined(&baseline);

        let mut next = baseline.clone();
        next.push(mk_incident(
            "b",
            Impact::Major,
            vec![mk_update("b1", "investigating", "2026-09-01T11:00:00Z")],
        ));

        let events = diff(&mut state, &next, Impact::None);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].incident_id, "b");
        assert_eq!(events[0].state, IncidentState::Investigating);
    }

    #[test]
    fn unchanged_incident_is_not_reported_twice() {
        let incidents = vec![mk_incident(
            "a",
            Impact::Minor,
            vec![mk_update("a1", "investigating", "2026-09-01T10:00:00Z")],
        )];
        let mut state = baselined(&incidents);
        assert!(diff(&mut state, &incidents, Impact::None).is_empty());
        assert!(diff(&mut state, &incidents, Impact::None).is_empty());
    }

    #[test]
    fn new_update_on_known_incident_is_reported() {
        let baseline = vec![mk_incident(
            "a",
            Impact::Major,
            vec![mk_update("a1", "investigating", "2026-09-01T10:00:00Z")],
        )];
        let mut state = baselined(&baseline);

        let next = vec![mk_incident(
            "a",
            Impact::Major,
            vec![
                mk_update("a2", "resolved", "2026-09-01T10:30:00Z"),
                mk_update("a1", "investigating", "2026-09-01T10:00:00Z"),
            ],
        )];
        let events = diff(&mut state, &next, Impact::None);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, IncidentState::Resolved);
        assert_eq!(events[0].occurred_at, ts("2026-09-01T10:30:00Z"));
    }

    #[test]
    fn latest_update_wins_regardless_of_array_order() {
        let baseline = vec![mk_incident("a", Impact::Minor, vec![])];
        let mut state = baselined(&baseline);
        let next = vec![mk_incident(
            "a",
            Impact::Minor,
            vec![
                mk_update("old", "investigating", "2026-09-01T10:00:00Z"),
                mk_update("new", "monitoring", "2026-09-01T12:00:00Z"),
                mk_update("mid", "identified", "2026-09-01T11:00:00Z"),
            ],
        )];
        let events = diff(&mut state, &next, Impact::None);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, IncidentState::Monitoring);
    }

    #[test]
    fn incident_without_updates_is_skipped() {
        let mut state = baselined(&[]);
        let next = vec![mk_incident("a", Impact::Critical, vec![])];
        assert!(diff(&mut state, &next, Impact::None).is_empty());
    }

    #[test]
    fn min_impact_filters_below_threshold() {
        let mut state = baselined(&[]);
        let next = vec![
            mk_incident(
                "low",
                Impact::Minor,
                vec![mk_update("l1", "investigating", "2026-09-01T10:00:00Z")],
            ),
            mk_incident(
                "high",
                Impact::Critical,
                vec![mk_update("h1", "investigating", "2026-09-01T10:00:00Z")],
            ),
        ];
        let events = diff(&mut state, &next, Impact::Major);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].incident_id, "high");
    }

    #[test]
    fn filtered_incident_is_still_recorded_as_seen() {
        // Otherwise lowering `min_impact` later would replay the whole backlog.
        let mut state = baselined(&[]);
        let next = vec![mk_incident(
            "low",
            Impact::Minor,
            vec![mk_update("l1", "investigating", "2026-09-01T10:00:00Z")],
        )];
        assert!(diff(&mut state, &next, Impact::Major).is_empty());
        assert!(diff(&mut state, &next, Impact::None).is_empty());
    }

    #[test]
    fn unknown_impact_always_passes_the_filter() {
        let mut state = baselined(&[]);
        let mut incident = mk_incident(
            "x",
            Impact::None,
            vec![mk_update("x1", "investigating", "2026-09-01T10:00:00Z")],
        );
        incident.impact = None;
        let events = diff(&mut state, &[incident], Impact::Critical);
        assert_eq!(events.len(), 1);
        assert!(events[0].impact.is_none());
    }

    #[test]
    fn events_are_ordered_oldest_first() {
        let mut state = baselined(&[]);
        let next = vec![
            mk_incident(
                "late",
                Impact::Minor,
                vec![mk_update("l", "investigating", "2026-09-01T12:00:00Z")],
            ),
            mk_incident(
                "early",
                Impact::Minor,
                vec![mk_update("e", "investigating", "2026-09-01T09:00:00Z")],
            ),
        ];
        let events = diff(&mut state, &next, Impact::None);
        assert_eq!(events[0].incident_id, "early");
        assert_eq!(events[1].incident_id, "late");
    }

    #[test]
    fn incidents_that_roll_out_of_the_feed_are_pruned_from_state() {
        let a = mk_incident(
            "a",
            Impact::Minor,
            vec![mk_update("a1", "investigating", "2026-09-01T10:00:00Z")],
        );
        let b = mk_incident(
            "b",
            Impact::Minor,
            vec![mk_update("b1", "investigating", "2026-09-01T11:00:00Z")],
        );
        let mut state = baselined(&[a.clone(), b.clone()]);

        // `a` aged out of the 50 most recent incidents; `b` is unchanged.
        assert!(diff(&mut state, std::slice::from_ref(&b), Impact::None).is_empty());
        assert_eq!(state.seen.len(), 1);

        // `a` reappearing is treated as new, not as already-seen.
        let events = diff(&mut state, &[a, b], Impact::None);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].incident_id, "a");
    }

    #[test]
    fn an_empty_feed_keeps_the_baseline() {
        let baseline = vec![mk_incident(
            "a",
            Impact::Minor,
            vec![mk_update("a1", "investigating", "2026-09-01T10:00:00Z")],
        )];
        let mut state = baselined(&baseline);
        assert!(diff(&mut state, &[], Impact::None).is_empty());
        // The baseline survived, so the returning feed reports nothing.
        assert!(diff(&mut state, &baseline, Impact::None).is_empty());
    }

    fn event(state: &str, impact: Option<Impact>) -> StatusEvent {
        StatusEvent {
            incident_id: "inc1".to_string(),
            title: "Degraded performance on platform.claude.com".to_string(),
            body: "We are investigating reports of degraded performance.".to_string(),
            state: IncidentState::from_wire(state),
            impact,
            link: Some("https://stspg.io/4rf459q4pjc3".to_string()),
            components: vec!["claude.ai".to_string(), "Claude Console".to_string()],
            occurred_at: ts("2026-09-01T17:05:11.575Z"),
        }
    }

    #[test]
    fn message_carries_the_page_identity_and_incident_link() {
        let page = crate::runtime::mk_run_status_page("claude", Duration::from_secs(300));
        let msg = build_message(&page, &event("investigating", Some(Impact::Minor)));

        assert_eq!(msg.username.as_deref(), Some("claude Status"));
        assert!(msg.content.is_none());
        let embed = &msg.embeds[0];
        assert_eq!(embed.title, "Degraded performance on platform.claude.com");
        assert_eq!(embed.url.as_deref(), Some("https://stspg.io/4rf459q4pjc3"));
        assert_eq!(embed.footer.as_ref().unwrap().text, "status.claude.example");
        assert_eq!(
            embed.timestamp.as_deref(),
            Some("2026-09-01T17:05:11.575+00:00")
        );
    }

    #[test]
    fn message_fields_report_status_impact_and_components() {
        let page = crate::runtime::mk_run_status_page("claude", Duration::from_secs(300));
        let msg = build_message(&page, &event("identified", Some(Impact::Major)));
        let fields = &msg.embeds[0].fields;

        assert_eq!(fields[0].name, "Status");
        assert_eq!(fields[0].value, "🎯 Identified");
        assert!(fields[0].inline);
        assert_eq!(fields[1].name, "Impact");
        assert_eq!(fields[1].value, "Major");
        assert_eq!(fields[2].name, "Components");
        assert_eq!(fields[2].value, "claude.ai, Claude Console");
        assert!(!fields[2].inline);
    }

    #[test]
    fn embed_color_follows_impact() {
        let page = crate::runtime::mk_run_status_page("claude", Duration::from_secs(300));
        let color = |impact| build_message(&page, &event("investigating", impact)).embeds[0].color;
        assert_eq!(color(Some(Impact::Critical)), COLOR_CRITICAL);
        assert_eq!(color(Some(Impact::Major)), COLOR_MAJOR);
        assert_eq!(color(Some(Impact::Minor)), COLOR_MINOR);
        assert_eq!(color(Some(Impact::None)), COLOR_NEUTRAL);
        // An unrecognized severity must not be dressed up as a known one.
        assert_eq!(color(None), COLOR_NEUTRAL);
    }

    #[test]
    fn resolved_is_green_even_for_a_critical_incident() {
        let page = crate::runtime::mk_run_status_page("claude", Duration::from_secs(300));
        let msg = build_message(&page, &event("resolved", Some(Impact::Critical)));
        assert_eq!(msg.embeds[0].color, COLOR_RESOLVED);
        assert_eq!(msg.embeds[0].fields[0].value, "✅ Resolved");
        // The severity it *had* is still reported.
        assert_eq!(msg.embeds[0].fields[1].value, "Critical");
    }

    #[test]
    fn unknown_impact_is_labelled_rather_than_guessed() {
        let page = crate::runtime::mk_run_status_page("claude", Duration::from_secs(300));
        let msg = build_message(&page, &event("investigating", None));
        assert_eq!(msg.embeds[0].fields[1].value, "Unknown");
    }

    #[test]
    fn join_base_adds_the_missing_slash() {
        let url = Url::parse("https://status.claude.com").unwrap();
        assert_eq!(
            join_base(&url, STATUSPAGE_PATH).unwrap().as_str(),
            "https://status.claude.com/api/v2/incidents.json"
        );
    }

    #[test]
    fn join_base_keeps_an_existing_path() {
        for base in ["https://x.example/status", "https://x.example/status/"] {
            let url = Url::parse(base).unwrap();
            assert_eq!(
                join_base(&url, DATADOG_PATH).unwrap().as_str(),
                "https://x.example/status/config.json",
                "{base}"
            );
        }
    }

    #[test]
    fn decoder_accept_is_feed_types_only_for_incidentio() {
        assert_eq!(Decoder::IncidentIo.accept(), FEED_ACCEPT);
        for decoder in [Decoder::Statuspage, Decoder::Instatus, Decoder::Datadog] {
            assert_eq!(decoder.accept(), "application/json", "{decoder:?}");
        }
    }

    #[test]
    fn decoder_hands_the_body_to_its_provider() {
        let page_url = Url::parse("https://x.example/status").unwrap();
        let api_url = Url::parse("https://x.example/status/feed.atom").unwrap();
        let now = ts("2026-10-17T14:00:00Z");
        let mapping: CustomMapping = toml::from_str(
            r#"id = "/id"
title = "/name""#,
        )
        .unwrap();
        for (decoder, body) in [
            (
                Decoder::Statuspage,
                r#"{"incidents":[{"id":"i","name":"Down"}]}"#,
            ),
            (
                Decoder::Instatus,
                r#"{"page":{"status":"HASISSUES"},"activeIncidents":[{"id":"i","name":"Down"}]}"#,
            ),
            (
                Decoder::IncidentIo,
                r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><entry><id>i</id><title>Down</title></entry></feed>"#,
            ),
            (
                Decoder::Datadog,
                r#"{"incidents":[{"id":"i","title":"Down"}]}"#,
            ),
            (Decoder::Custom(mapping), r#"[{"id":"i","name":"Down"}]"#),
        ] {
            let incidents = decoder
                .decode(body.as_bytes(), &page_url, &api_url, now)
                .unwrap_or_else(|e| panic!("{}: {e}", decoder.label()));
            assert_eq!(incidents.len(), 1, "{}", decoder.label());
            assert_eq!(incidents[0].name, "Down", "{}", decoder.label());
        }
    }

    #[test]
    fn decode_error_names_the_provider() {
        let url = Url::parse("https://x.example").unwrap();
        let now = ts("2026-10-17T14:00:00Z");
        let mapping: CustomMapping = toml::from_str(
            r#"id = "/id"
title = "/name""#,
        )
        .unwrap();
        for (decoder, label) in [
            (Decoder::Statuspage, "Atlassian Statuspage"),
            (Decoder::Instatus, "Instatus"),
            (Decoder::IncidentIo, "incident.io"),
            (Decoder::Datadog, "Datadog Status Pages"),
            (Decoder::Custom(mapping), "custom JSON"),
        ] {
            let source = decoder.decode(b"<html>", &url, &url, now).unwrap_err();
            let err = StatusError::Decode {
                kind: decoder.label(),
                source,
            };
            assert!(
                err.to_string()
                    .starts_with(&format!("response does not decode as {label}: ")),
                "{err}"
            );
        }
    }

    #[test]
    fn state_names_match_case_insensitively() {
        assert_eq!(
            IncidentState::from_wire("RESOLVED"),
            IncidentState::Resolved
        );
        assert_eq!(
            IncidentState::from_wire("Monitoring"),
            IncidentState::Monitoring
        );
        assert_eq!(
            IncidentState::from_wire("Scheduled"),
            IncidentState::Unknown
        );
    }

    #[test]
    fn message_without_link_or_components_omits_them() {
        let page = crate::runtime::mk_run_status_page("claude", Duration::from_secs(300));
        let mut ev = event("monitoring", Some(Impact::Minor));
        ev.link = None;
        ev.components.clear();
        let msg = build_message(&page, &ev);
        assert!(msg.embeds[0].url.is_none());
        assert_eq!(msg.embeds[0].fields.len(), 2);
    }
}
