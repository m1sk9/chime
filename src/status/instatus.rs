use std::collections::HashSet;

use serde::Deserialize;

use super::{DecodeError, Incident, IncidentState, IncidentUpdate, parse_time};
use crate::config::Impact;

// Why `page` is required while both lists default: a quiet page omits the lists
// altogether (`{"page":{...,"status":"UP"}}`), so they cannot be the signature
// that tells an Instatus summary from any other JSON object. Maintenances are not
// read: no provider's incidents are forwarded together with its maintenances.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSummary {
    #[serde(rename = "page")]
    _page: WirePage,
    #[serde(default)]
    active_incidents: Vec<WireActive>,
    #[serde(default)]
    recently_resolved_incidents: Vec<WireResolved>,
}

#[derive(Debug, Deserialize)]
struct WirePage {
    #[serde(rename = "status")]
    _status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireActive {
    id: String,
    name: String,
    status: Option<String>,
    impact: Option<String>,
    url: Option<String>,
    started: Option<String>,
    updated_at: Option<String>,
    last_update: Option<WireLastUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLastUpdate {
    message: Option<String>,
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireResolved {
    id: String,
    name: String,
    resolved: Option<String>,
    impact: Option<String>,
    url: Option<String>,
}

pub(super) fn decode(body: &[u8]) -> Result<Vec<Incident>, DecodeError> {
    let summary: WireSummary = serde_json::from_slice(body)?;
    let active = summary
        .active_incidents
        .into_iter()
        .map(WireActive::normalize);
    let resolved = summary
        .recently_resolved_incidents
        .into_iter()
        .map(WireResolved::normalize);
    // An incident resolved a moment ago can sit in both lists; the active entry
    // carries the update text, so it wins.
    let mut ids = HashSet::new();
    Ok(active
        .chain(resolved)
        .filter(|i| ids.insert(i.id.clone()))
        .collect())
}

impl WireActive {
    /// The summary carries only the latest update, so its time is the update's id:
    /// several updates between two polls are forwarded as the last one.
    fn normalize(self) -> Incident {
        let (message, last_at) = match self.last_update {
            Some(u) => (u.message, u.created_at),
            None => (None, None),
        };
        let at = last_at.or(self.updated_at).or(self.started);
        let updates = at
            .and_then(|at| {
                Some(IncidentUpdate {
                    created_at: parse_time(&at)?,
                    id: at,
                    state: self
                        .status
                        .as_deref()
                        .map_or(IncidentState::Unknown, IncidentState::from_wire),
                    body: message.unwrap_or_default(),
                })
            })
            .into_iter()
            .collect();
        Incident {
            id: self.id,
            name: self.name,
            impact: self.impact.as_deref().and_then(impact),
            link: self.url,
            components: Vec::new(),
            updates,
        }
    }
}

impl WireResolved {
    fn normalize(self) -> Incident {
        let updates = self
            .resolved
            .and_then(|at| {
                Some(IncidentUpdate {
                    created_at: parse_time(&at)?,
                    id: at,
                    state: IncidentState::Resolved,
                    body: String::new(),
                })
            })
            .into_iter()
            .collect();
        Incident {
            id: self.id,
            name: self.name,
            impact: self.impact.as_deref().and_then(impact),
            link: self.url,
            components: Vec::new(),
            updates,
        }
    }
}

fn impact(s: &str) -> Option<Impact> {
    match s.to_ascii_uppercase().as_str() {
        "MAJOROUTAGE" => Some(Impact::Critical),
        "PARTIALOUTAGE" => Some(Impact::Major),
        "DEGRADEDPERFORMANCE" => Some(Impact::Minor),
        "UNDERMAINTENANCE" => Some(Impact::Maintenance),
        "OPERATIONAL" => Some(Impact::None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::ts;

    /// The v3 example from Instatus's `/public-api` page.
    const SUMMARY: &str = r#"{
      "page": { "name": "Instatus", "url": "https://instat.us", "status": "HASISSUES" },
      "activeIncidents": [
        {
          "id": "cl4a8n3070721qtmzvts760c4",
          "name": "We're facing an issue with our API",
          "started": "2022-06-11T18:55:50Z",
          "status": "INVESTIGATING",
          "impact": "MAJOROUTAGE",
          "url": "https://testing-widget.instatus.com/incident/cl4a8n3070721qtmzvts760c4",
          "updatedAt": "2022-06-11T19:30:00Z",
          "lastUpdate": { "message": "We're looking into errors on the API.", "createdAt": "2022-06-11T19:30:00Z" }
        }
      ],
      "activeMaintenances": [
        {
          "id": "cl4a8nlv50918qtmzdlw60ivu",
          "name": "Database maintenance",
          "start": "2022-06-11T18:55:54Z",
          "status": "NOTSTARTEDYET",
          "duration": "60",
          "url": "https://testing-widget.instatus.com/maintenance/cl4a8nlv50918qtmzdlw60ivu",
          "updatedAt": "2022-06-11T19:30:00Z"
        }
      ],
      "recentlyResolvedIncidents": [
        {
          "id": "cl4a8m6xk0612qtmzq2b1v8de",
          "name": "Slow dashboard loading",
          "started": "2022-06-11T09:12:00Z",
          "resolved": "2022-06-11T10:40:00Z",
          "impact": "DEGRADEDPERFORMANCE",
          "url": "https://testing-widget.instatus.com/incident/cl4a8m6xk0612qtmzq2b1v8de"
        }
      ]
    }"#;

    #[test]
    fn active_incident_becomes_one_update_keyed_by_last_update_time() {
        let incident = decode(SUMMARY.as_bytes()).unwrap().remove(0);
        assert_eq!(incident.id, "cl4a8n3070721qtmzvts760c4");
        assert_eq!(incident.name, "We're facing an issue with our API");
        assert_eq!(incident.impact, Some(Impact::Critical));
        assert_eq!(
            incident.link.as_deref(),
            Some("https://testing-widget.instatus.com/incident/cl4a8n3070721qtmzvts760c4")
        );
        assert_eq!(incident.updates.len(), 1);
        let update = &incident.updates[0];
        assert_eq!(update.id, "2022-06-11T19:30:00Z");
        assert_eq!(update.state, IncidentState::Investigating);
        assert_eq!(update.body, "We're looking into errors on the API.");
        assert_eq!(update.created_at, ts("2022-06-11T19:30:00Z"));
    }

    #[test]
    fn active_incident_without_last_update_falls_back_to_updated_at_then_started() {
        let decoded = decode(
            br#"{"page":{"status":"HASISSUES"},"activeIncidents":[
              {"id":"a","name":"n","status":"IDENTIFIED","updatedAt":"2022-06-11T19:00:00Z","started":"2022-06-11T18:00:00Z"},
              {"id":"b","name":"n","status":"IDENTIFIED","started":"2022-06-11T18:00:00Z"},
              {"id":"c","name":"n","status":"IDENTIFIED"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(decoded[0].updates[0].id, "2022-06-11T19:00:00Z");
        assert_eq!(decoded[0].updates[0].body, "");
        assert_eq!(decoded[1].updates[0].id, "2022-06-11T18:00:00Z");
        assert!(
            decoded[2].updates.is_empty(),
            "no time at all leaves nothing to report"
        );
    }

    #[test]
    fn recently_resolved_incident_becomes_a_resolved_update_without_body() {
        let incident = decode(SUMMARY.as_bytes()).unwrap().remove(1);
        assert_eq!(incident.id, "cl4a8m6xk0612qtmzq2b1v8de");
        assert_eq!(incident.impact, Some(Impact::Minor));
        let update = &incident.updates[0];
        assert_eq!(update.id, "2022-06-11T10:40:00Z");
        assert_eq!(update.state, IncidentState::Resolved);
        assert_eq!(update.body, "");
        assert_eq!(update.created_at, ts("2022-06-11T10:40:00Z"));
    }

    #[test]
    fn maintenances_are_ignored() {
        let ids: Vec<String> = decode(SUMMARY.as_bytes())
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(
            ids,
            ["cl4a8n3070721qtmzvts760c4", "cl4a8m6xk0612qtmzq2b1v8de"]
        );
    }

    #[test]
    fn quiet_summary_without_arrays_decodes_to_no_incidents() {
        let body = br#"{"page":{"name":"Instatus","url":"https://instat.us","status":"UP"}}"#;
        assert!(decode(body).unwrap().is_empty());
    }

    #[test]
    fn json_without_page_object_is_not_instatus() {
        for body in [&br#"{"error":"not found"}"#[..], br#"{"incidents":[]}"#] {
            assert!(matches!(decode(body), Err(DecodeError::Json(_))));
        }
    }

    #[test]
    fn same_id_in_both_lists_keeps_the_active_one() {
        let decoded = decode(
            br#"{"page":{"status":"UP"},
              "activeIncidents":[{"id":"x","name":"n","status":"RESOLVED",
                "lastUpdate":{"message":"Fixed.","createdAt":"2022-06-11T10:40:00Z"}}],
              "recentlyResolvedIncidents":[{"id":"x","name":"n","resolved":"2022-06-11T10:40:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].updates[0].body, "Fixed.");
    }

    #[test]
    fn impact_maps_instatus_vocabulary() {
        assert_eq!(impact("MAJOROUTAGE"), Some(Impact::Critical));
        assert_eq!(impact("PARTIALOUTAGE"), Some(Impact::Major));
        assert_eq!(impact("DEGRADEDPERFORMANCE"), Some(Impact::Minor));
        assert_eq!(impact("UNDERMAINTENANCE"), Some(Impact::Maintenance));
        assert_eq!(impact("OPERATIONAL"), Some(Impact::None));
        assert_eq!(impact("majoroutage"), Some(Impact::Critical));
        assert_eq!(impact("APOCALYPSE"), None);
    }
}
