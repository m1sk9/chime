use serde::Deserialize;

use super::{DecodeError, Incident, IncidentState, IncidentUpdate, parse_time};
use crate::config::Impact;

// Why not `deny_unknown_fields`: every other struct in chime rejects unknown keys,
// but these mirror a third-party API. Statuspage adds fields without notice, and a
// strict decoder would turn a harmless addition into a total outage of the feature.
// Why not `#[serde(default)]` on `incidents`: a defaulted field makes every JSON
// object decode, so a URL that is not a Statuspage instance would look like a page
// with no incidents forever instead of failing the poll with a decode error. Only
// `id`/`name` are required below — `impact` is optional for the same reason it is
// parsed leniently: an absent severity must degrade to "Unknown", not silence the
// page behind a decode error on every poll.
#[derive(Debug, Deserialize)]
struct WireResponse {
    incidents: Vec<WireIncident>,
}

#[derive(Debug, Deserialize)]
struct WireIncident {
    id: String,
    name: String,
    impact: Option<String>,
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

pub(super) fn decode(body: &[u8]) -> Result<Vec<Incident>, DecodeError> {
    let response: WireResponse = serde_json::from_slice(body)?;
    Ok(response
        .incidents
        .into_iter()
        .map(WireIncident::normalize)
        .collect())
}

impl WireIncident {
    fn normalize(self) -> Incident {
        let updates = self
            .incident_updates
            .into_iter()
            .filter_map(|u| {
                Some(IncidentUpdate {
                    created_at: parse_time(&u.created_at)?,
                    id: u.id,
                    state: IncidentState::from_wire(&u.status),
                    body: u.body,
                })
            })
            .collect();
        Incident {
            id: self.id,
            name: self.name,
            impact: self.impact.as_deref().and_then(Impact::from_wire),
            link: self.shortlink,
            components: self.components.into_iter().map(|c| c.name).collect(),
            updates,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::ts;

    fn one(json: &str) -> Incident {
        decode(json.as_bytes()).unwrap().remove(0)
    }

    #[test]
    fn decodes_a_real_statuspage_payload() {
        let incident = one(r#"{
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
        }"#);
        assert_eq!(incident.id, "inc1");
        assert_eq!(incident.impact, Some(Impact::Minor));
        assert_eq!(
            incident.link.as_deref(),
            Some("https://stspg.io/4rf459q4pjc3")
        );
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
        let incident = one(r#"{"incidents":[{
            "id":"i","name":"n","impact":"minor",
            "incident_updates":[
              {"id":"good","status":"resolved","body":"b","created_at":"2026-09-01T10:00:00Z"},
              {"id":"bad","status":"resolved","body":"b","created_at":"yesterday"}
            ]}]}"#);
        assert_eq!(incident.updates.len(), 1);
        assert_eq!(incident.updates[0].id, "good");
    }

    #[test]
    fn json_without_an_incidents_array_is_not_a_statuspage_feed() {
        assert!(matches!(
            decode(br#"{"error":"not found"}"#),
            Err(DecodeError::Json(_))
        ));
    }

    #[test]
    fn a_missing_wire_impact_does_not_fail_the_whole_feed() {
        let incident = one(r#"{"incidents":[{"id":"i","name":"n","incident_updates":[]}]}"#);
        assert!(incident.impact.is_none());
    }

    #[test]
    fn unknown_wire_impact_becomes_none() {
        let incident = one(
            r#"{"incidents":[{"id":"i","name":"n","impact":"catastrophic","incident_updates":[]}]}"#,
        );
        assert!(incident.impact.is_none());
        assert!(incident.link.is_none());
    }
}
