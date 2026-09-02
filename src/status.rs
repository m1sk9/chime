use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::Client;
use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH};
use serde::Deserialize;
use url::Url;

use crate::config::Impact;
use crate::notifier::{DiscordMessage, Embed};
use crate::runtime::RunStatusPage;

/// Path appended to a status page base URL. `incidents.json` is used rather than
/// `summary.json` because summary omits resolved incidents, and "resolved" is the
/// single most useful update to forward.
pub const INCIDENTS_PATH: &str = "api/v2/incidents.json";

/// Deliberately shorter than the Discord timeout: this request runs inside the
/// scheduler tick, so a slow status page must not delay a reminder past its minute.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY: usize = 512;

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
    #[error("response is not an Atlassian Statuspage incidents feed: {0}")]
    Decode(#[source] reqwest::Error),
}

// Why not `deny_unknown_fields`: every other struct in chime rejects unknown keys,
// but these mirror a third-party API. Statuspage adds fields without notice, and a
// strict decoder would turn a harmless addition into a total outage of the feature.
#[derive(Debug, Deserialize)]
struct WireResponse {
    #[serde(default)]
    incidents: Vec<WireIncident>,
}

#[derive(Debug, Deserialize)]
struct WireIncident {
    id: String,
    name: String,
    impact: String,
    shortlink: Option<String>,
    #[serde(default)]
    incident_updates: Vec<WireUpdate>,
    #[serde(default)]
    components: Vec<WireComponent>,
}

#[derive(Debug, Deserialize)]
struct WireUpdate {
    id: String,
    status: String,
    body: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct WireComponent {
    name: String,
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
    fn from_wire(s: &str) -> Self {
        match s {
            "investigating" => IncidentState::Investigating,
            "identified" => IncidentState::Identified,
            "monitoring" => IncidentState::Monitoring,
            "resolved" => IncidentState::Resolved,
            "postmortem" => IncidentState::Postmortem,
            _ => IncidentState::Unknown,
        }
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

/// A Statuspage incident, normalized: timestamps parsed, severities typed. The
/// wire types never leave this module.
#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub id: String,
    pub name: String,
    /// `None` when Statuspage reported a severity chime does not know.
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

impl WireIncident {
    fn normalize(self) -> Incident {
        let updates = self
            .incident_updates
            .into_iter()
            .filter_map(|u| {
                let created_at = DateTime::parse_from_rfc3339(&u.created_at).ok()?;
                Some(IncidentUpdate {
                    id: u.id,
                    state: IncidentState::from_wire(&u.status),
                    body: u.body,
                    created_at: created_at.with_timezone(&Utc),
                })
            })
            .collect();
        Incident {
            id: self.id,
            name: self.name,
            impact: Impact::from_wire(&self.impact),
            link: self.shortlink,
            components: self.components.into_iter().map(|c| c.name).collect(),
            updates,
        }
    }
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
    let current: HashSet<&str> = incidents.iter().map(|i| i.id.as_str()).collect();
    state.seen.retain(|id, _| current.contains(id.as_str()));

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

    let impact_label = match event.impact {
        Some(i) => i.to_string(),
        None => "Unknown".to_string(),
    };

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

    DiscordMessage::embed(embed).with_identity(&page.display_name, page.avatar_url.as_deref())
}

#[derive(Debug)]
pub enum Fetched {
    NotModified,
    Modified {
        incidents: Vec<Incident>,
        etag: Option<String>,
    },
}

#[allow(async_fn_in_trait)]
pub(crate) trait StatusSource {
    async fn fetch(&self, url: &Url, etag: Option<&str>) -> Result<Fetched, StatusError>;
}

#[derive(Debug, Clone)]
pub struct Statuspage {
    client: Client,
}

impl Statuspage {
    pub fn new(client: Client) -> Self {
        Statuspage { client }
    }
}

impl StatusSource for Statuspage {
    async fn fetch(&self, url: &Url, etag: Option<&str>) -> Result<Fetched, StatusError> {
        // `Accept` is required, not merely polite. The status page CDN answers with
        // `Vary: Accept, Accept-Encoding`, and a request that omits `Accept` lands on
        // a variant that returns 200 to every `If-None-Match` — verified against
        // status.claude.com. Dropping this header silently disables conditional GETs.
        let mut request = self
            .client
            .get(url.clone())
            .timeout(FETCH_TIMEOUT)
            .header(ACCEPT, "application/json");
        if let Some(tag) = etag {
            request = request.header(IF_NONE_MATCH, tag);
        }
        let resp = request.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        if !status.is_success() {
            let bytes = resp.bytes().await?;
            let end = bytes.len().min(MAX_ERROR_BODY);
            return Err(StatusError::Status {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes[..end]).into_owned(),
            });
        }
        let new_etag = resp
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body: WireResponse = resp.json().await.map_err(StatusError::Decode)?;
        Ok(Fetched::Modified {
            incidents: body
                .incidents
                .into_iter()
                .map(WireIncident::normalize)
                .collect(),
            etag: new_etag,
        })
    }
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
    fn vanished_incidents_are_pruned_from_state() {
        let baseline = vec![mk_incident(
            "a",
            Impact::Minor,
            vec![mk_update("a1", "investigating", "2026-09-01T10:00:00Z")],
        )];
        let mut state = baselined(&baseline);
        assert!(diff(&mut state, &[], Impact::None).is_empty());
        assert!(state.seen.is_empty());

        // The incident reappearing is treated as new, not as already-seen.
        let events = diff(&mut state, &baseline, Impact::None);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn wire_response_decodes_real_statuspage_payload() {
        let json = r#"{
          "page": {"id": "abc", "name": "Claude"},
          "incidents": [{
            "id": "inc1",
            "name": "Degraded performance on platform.claude.com",
            "status": "investigating",
            "created_at": "2026-09-01T17:05:11.462Z",
            "impact": "minor",
            "shortlink": "https://stspg.io/4rf459q4pjc3",
            "future_field_statuspage_added_later": 42,
            "components": [{"id": "c1", "name": "claude.ai"}],
            "incident_updates": [{
              "id": "upd1",
              "status": "investigating",
              "body": "We are investigating reports of degraded performance.",
              "created_at": "2026-09-01T17:05:11.575Z",
              "deliver_notifications": true
            }]
          }]
        }"#;
        let parsed: WireResponse = serde_json::from_str(json).unwrap();
        let incident = parsed.incidents.into_iter().next().unwrap().normalize();
        assert_eq!(incident.id, "inc1");
        assert_eq!(incident.impact, Some(Impact::Minor));
        assert_eq!(incident.components, vec!["claude.ai".to_string()]);
        assert_eq!(incident.updates.len(), 1);
        assert_eq!(incident.updates[0].state, IncidentState::Investigating);
        assert_eq!(
            incident.updates[0].created_at,
            ts("2026-09-01T17:05:11.575Z")
        );
    }

    #[test]
    fn unparsable_timestamps_drop_only_that_update() {
        let json = r#"{"incidents":[{
            "id":"i","name":"n","impact":"minor",
            "incident_updates":[
              {"id":"good","status":"resolved","body":"b","created_at":"2026-09-01T10:00:00Z"},
              {"id":"bad","status":"resolved","body":"b","created_at":"yesterday"}
            ]}]}"#;
        let parsed: WireResponse = serde_json::from_str(json).unwrap();
        let incident = parsed.incidents.into_iter().next().unwrap().normalize();
        assert_eq!(incident.updates.len(), 1);
        assert_eq!(incident.updates[0].id, "good");
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
    fn message_without_link_or_components_omits_them() {
        let page = crate::runtime::mk_run_status_page("claude", Duration::from_secs(300));
        let mut ev = event("monitoring", Some(Impact::Minor));
        ev.link = None;
        ev.components.clear();
        let msg = build_message(&page, &ev);
        assert!(msg.embeds[0].url.is_none());
        assert_eq!(msg.embeds[0].fields.len(), 2);
    }

    #[test]
    fn unknown_wire_impact_becomes_none() {
        let json = r#"{"incidents":[{"id":"i","name":"n","impact":"catastrophic","incident_updates":[]}]}"#;
        let parsed: WireResponse = serde_json::from_str(json).unwrap();
        let incident = parsed.incidents.into_iter().next().unwrap().normalize();
        assert!(incident.impact.is_none());
        assert!(incident.link.is_none());
    }
}
