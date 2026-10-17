use chrono::{DateTime, Utc};
use url::Url;

use super::{DecodeError, Incident, IncidentState, IncidentUpdate};
use crate::rss::{RssEntry, parse_feed};

const STATUS_PREFIX: &str = "Status:";
const COMPONENTS_HEADING: &str = "Affected components";

/// `feed_url` is the base feed-rs resolves relative links against. Each entry is
/// one incident carrying only its latest update; the feed has no severity, so
/// `impact` is always unknown and `min_impact` never filters these incidents.
pub(super) fn decode(
    body: &[u8],
    feed_url: &Url,
    now: DateTime<Utc>,
) -> Result<Vec<Incident>, DecodeError> {
    Ok(parse_feed(body, feed_url)?
        .into_iter()
        .filter_map(|e| normalize(e.render(), now))
        .collect())
}

fn normalize(entry: RssEntry, now: DateTime<Utc>) -> Option<Incident> {
    let name = entry.title?;
    let text = parse_body(entry.body.as_deref().unwrap_or_default());
    Some(Incident {
        // Kept verbatim, `//incidents/` included: it is the dedup key, and tidying
        // it would make every known incident look new after an upgrade.
        id: entry.id,
        name,
        impact: None,
        link: entry.link,
        components: text.components,
        updates: vec![IncidentUpdate {
            id: entry.published.map(|t| t.to_rfc3339()).unwrap_or_default(),
            state: text.state,
            body: text.body,
            created_at: entry.published.unwrap_or(now),
        }],
    })
}

#[derive(Debug, PartialEq)]
struct Body {
    state: IncidentState,
    body: String,
    components: Vec<String>,
}

/// Splits the rendered entry body, `Status: X` / text / `Affected components` /
/// `- Name (State)` lines, into its parts. Lines that do not fit stay in the text.
fn parse_body(text: &str) -> Body {
    let mut lines = text.lines().peekable();
    let state = match lines.peek().and_then(|l| l.strip_prefix(STATUS_PREFIX)) {
        Some(label) => {
            let state = IncidentState::from_wire(label.trim());
            lines.next();
            state
        }
        None => IncidentState::Unknown,
    };
    let mut body = Vec::new();
    let mut components = Vec::new();
    let mut in_components = false;
    for line in lines {
        if line.trim().eq_ignore_ascii_case(COMPONENTS_HEADING) {
            in_components = true;
            continue;
        }
        match line.strip_prefix("- ") {
            Some(item) if in_components => components.push(component_name(item).to_string()),
            _ => body.push(line),
        }
    }
    Body {
        state,
        body: body.join("\n").trim().to_string(),
        components,
    }
}

/// `Slack app (Operational)` → `Slack app`: the state in parentheses is the
/// component's state now, not during the incident.
fn component_name(item: &str) -> &str {
    let item = item.trim();
    match item.rfind(" (") {
        Some(open) if item.ends_with(')') => &item[..open],
        _ => item,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Impact;
    use crate::rss::html_to_text;
    use crate::status::ts;

    /// The first two entries of `https://status.incident.io/feed.atom`, 2026-10-17.
    const FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
    <id>https://status.incident.io/</id>
    <title>incident.io status</title>
    <updated>2026-10-17T13:35:41.169Z</updated>
    <generator>incident.io</generator>
    <link rel="alternate" href="https://status.incident.io/"/>
    <subtitle>incident.io status page updates</subtitle>
    <entry>
        <title type="html"><![CDATA[Login errors for subset of Slack Enterprise Grid customers]]></title>
        <id>https://status.incident.io//incidents/01M3ZC0F8MHYZFB7ZCGB9NQHEY</id>
        <link href="https://status.incident.io//incidents/01M3ZC0F8MHYZFB7ZCGB9NQHEY"/>
        <updated>2026-10-02T23:47:11.298Z</updated>
        <summary type="html"><![CDATA[<b>Status: Resolved</b><br/><br/>This incident has been resolved. Login via Slack is operating normally again, and no further action is required.<br/><br/><b>Affected components</b>
          <ul>
          <li>Slack app (Operational)</li>
          </ul>]]></summary>
        <content type="html"><![CDATA[<b>Status: Resolved</b><br/><br/>This incident has been resolved. Login via Slack is operating normally again, and no further action is required.<br/><br/><b>Affected components</b>
          <ul>
          <li>Slack app (Operational)</li>
          </ul>]]></content>
    </entry>
    <entry>
        <title type="html"><![CDATA[Increased latency in Microsoft Teams]]></title>
        <id>https://status.incident.io//incidents/01M0FT01J8EJQNVVYTMG9DJE94</id>
        <link href="https://status.incident.io//incidents/01M0FT01J8EJQNVVYTMG9DJE94"/>
        <updated>2026-08-20T16:31:44.765Z</updated>
        <summary type="html"><![CDATA[<b>Status: Resolved</b><br/><br/>Latency has now returned to normal.

Between 13:00 and 15:08 UTC there were periods of increased latency from some Microsoft APIs.<br/><br/><b>Affected components</b>
          <ul>
          <li>Microsoft Teams app (Operational)</li>
          </ul>]]></summary>
        <content type="html"><![CDATA[<b>Status: Resolved</b><br/><br/>Latency has now returned to normal.

Between 13:00 and 15:08 UTC there were periods of increased latency from some Microsoft APIs.<br/><br/><b>Affected components</b>
          <ul>
          <li>Microsoft Teams app (Operational)</li>
          </ul>]]></content>
    </entry>
</feed>"#;

    fn feed_url() -> Url {
        Url::parse("https://status.incident.io/feed.atom").unwrap()
    }

    fn now() -> DateTime<Utc> {
        ts("2026-10-17T14:00:00Z")
    }

    fn incidents() -> Vec<Incident> {
        decode(FEED.as_bytes(), &feed_url(), now()).unwrap()
    }

    #[test]
    fn the_rendered_body_has_the_shape_the_splitter_expects() {
        let html = r#"<b>Status: Resolved</b><br/><br/>This incident has been resolved.<br/><br/><b>Affected components</b>
          <ul>
          <li>Slack app (Operational)</li>
          </ul>"#;
        assert_eq!(
            html_to_text(html),
            "Status: Resolved\n\nThis incident has been resolved.\n\nAffected components\n- Slack app (Operational)"
        );
    }

    #[test]
    fn status_line_sets_the_state_and_is_stripped_from_the_body() {
        let incident = &incidents()[0];
        assert_eq!(
            incident.name,
            "Login errors for subset of Slack Enterprise Grid customers"
        );
        assert_eq!(incident.updates[0].state, IncidentState::Resolved);
        assert_eq!(
            incident.updates[0].body,
            "This incident has been resolved. Login via Slack is operating normally again, and no further action is required."
        );
        // The raw newlines inside the CDATA are HTML whitespace, so they collapse.
        assert_eq!(
            incidents()[1].updates[0].body,
            "Latency has now returned to normal. Between 13:00 and 15:08 UTC there were periods of increased latency from some Microsoft APIs."
        );
    }

    #[test]
    fn affected_components_become_component_names_without_their_status() {
        assert_eq!(incidents()[0].components, ["Slack app"]);
        assert_eq!(incidents()[1].components, ["Microsoft Teams app"]);
        assert_eq!(component_name("Slack app"), "Slack app");
        assert_eq!(component_name("API (EU) beta"), "API (EU) beta");
    }

    #[test]
    fn incident_id_and_link_are_the_entry_url_verbatim() {
        let incident = &incidents()[0];
        assert_eq!(
            incident.id,
            "https://status.incident.io//incidents/01M3ZC0F8MHYZFB7ZCGB9NQHEY"
        );
        assert_eq!(incident.link.as_deref(), Some(incident.id.as_str()));
    }

    #[test]
    fn update_id_is_the_entry_updated_time() {
        let update = &incidents()[0].updates[0];
        assert_eq!(update.created_at, ts("2026-10-02T23:47:11.298Z"));
        assert_eq!(update.id, ts("2026-10-02T23:47:11.298Z").to_rfc3339());
    }

    #[test]
    fn impact_is_always_unknown() {
        assert!(incidents().iter().all(|i| i.impact.is_none()));
        // `None` is "unknown", which `diff` always forwards — not `Impact::None`.
        assert!(incidents().iter().all(|i| i.impact != Some(Impact::None)));
    }

    #[test]
    fn entry_without_status_line_is_unknown() {
        let body = parse_body("We are looking into it.\n- not a component");
        assert_eq!(body.state, IncidentState::Unknown);
        assert_eq!(body.body, "We are looking into it.\n- not a component");
        assert!(body.components.is_empty());
    }

    #[test]
    fn status_label_is_matched_case_insensitively() {
        assert_eq!(
            parse_body("Status: Investigating\n\nLooking.").state,
            IncidentState::Investigating
        );
        assert_eq!(
            parse_body("Status: Scheduled").state,
            IncidentState::Unknown
        );
    }

    #[test]
    fn entry_without_date_is_timed_at_detection() {
        let feed = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
            <entry><id>urn:x</id><title>Down</title><summary>Status: Identified</summary></entry></feed>"#;
        let incident = decode(feed.as_bytes(), &feed_url(), now())
            .unwrap()
            .remove(0);
        assert_eq!(incident.updates[0].created_at, now());
        assert_eq!(incident.updates[0].id, "");
        assert_eq!(incident.updates[0].state, IncidentState::Identified);
    }

    #[test]
    fn non_feed_body_is_a_decode_error() {
        assert!(matches!(
            decode(br#"{"incidents":[]}"#, &feed_url(), now()),
            Err(DecodeError::Feed(_))
        ));
    }
}
