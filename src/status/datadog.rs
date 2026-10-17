use std::collections::HashSet;

use serde::Deserialize;
use url::Url;

use super::{DecodeError, Incident, IncidentState, IncidentUpdate, join_base, parse_time};
use crate::config::Impact;

// Why `incidents` is required: it is what tells this document from other JSON.
// `maintenances` is not read: maintenances are never forwarded, for any provider.
#[derive(Debug, Deserialize)]
struct WireConfig {
    incidents: Vec<WireIncident>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireIncident {
    id: String,
    title: String,
    #[serde(default)]
    components_affected: Vec<WireComponent>,
    #[serde(default)]
    timeline: Vec<WireTimeline>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTimeline {
    id: String,
    status: Option<String>,
    description: Option<String>,
    created_at: Option<String>,
    #[serde(default)]
    components_affected: Vec<WireComponent>,
}

#[derive(Debug, Deserialize)]
struct WireComponent {
    name: String,
    status: Option<String>,
}

/// `page_url` is the page's base URL; an incident's page is `<page_url>/<id>`.
pub(super) fn decode(body: &[u8], page_url: &Url) -> Result<Vec<Incident>, DecodeError> {
    let config: WireConfig = serde_json::from_slice(body)?;
    Ok(config
        .incidents
        .into_iter()
        .map(|i| i.normalize(page_url))
        .collect())
}

impl WireIncident {
    fn normalize(self, page_url: &Url) -> Incident {
        // Why not the incident's own `componentsAffected` statuses: they are the
        // components' state now, all `operational` once resolved, and the resolved
        // embed must still show how severe the incident was.
        let impact = self
            .timeline
            .iter()
            .flat_map(|t| &t.components_affected)
            .filter_map(|c| c.status.as_deref().and_then(component_impact))
            .max();
        let mut seen = HashSet::new();
        let components = self
            .components_affected
            .into_iter()
            .map(|c| c.name)
            .filter(|n| seen.insert(n.clone()))
            .collect();
        let updates = self
            .timeline
            .into_iter()
            .filter_map(|t| {
                Some(IncidentUpdate {
                    created_at: parse_time(t.created_at.as_deref()?)?,
                    id: t.id,
                    state: t
                        .status
                        .as_deref()
                        .map_or(IncidentState::Unknown, IncidentState::from_wire),
                    body: t.description.unwrap_or_default(),
                })
            })
            .collect();
        Incident {
            link: join_base(page_url, &self.id).ok().map(String::from),
            id: self.id,
            name: self.title,
            impact,
            components,
            updates,
        }
    }
}

fn component_impact(status: &str) -> Option<Impact> {
    match status {
        "major_outage" => Some(Impact::Critical),
        "partial_outage" => Some(Impact::Major),
        "degraded" => Some(Impact::Minor),
        "maintenance" => Some(Impact::Maintenance),
        "operational" => Some(Impact::None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::ts;

    /// From `https://demo-status-page.statuspage.datadoghq.com/config.json`,
    /// 2026-10-17, with `componentsAffected` trimmed to `id`, `name`, `status`.
    const CONFIG: &str = r#"{
      "name": "Demo Status Page",
      "pageUrl": "https://demo-status-page.statuspage.datadoghq.com",
      "incidents": [
        {
          "id": "3db838fd-93e6-4f48-8be5-788e67121f37",
          "title": "Current issue",
          "currentStatus": "resolved",
          "description": "The issue is now resolved.",
          "publishedDate": "2026-06-29T14:30:55.653313Z",
          "lastModifiedAt": "2026-06-29T14:32:08.951454Z",
          "resolved": true,
          "resolvedDate": "2026-06-29T14:32:08.951454Z",
          "componentsAffected": [ { "id": "490e2bc5-bd93-4925-93e4-a681ca4f9573", "name": "API", "status": "operational" } ],
          "timeline": [
            {
              "id": "b95517f7-ddd0-4afb-b9c1-511f606d6008",
              "status": "resolved",
              "description": "The issue is now resolved.",
              "createdAt": "2026-06-29T14:32:08.951454Z",
              "componentsAffected": [ { "id": "490e2bc5-bd93-4925-93e4-a681ca4f9573", "name": "API", "status": "operational" } ]
            },
            {
              "id": "55e0c619-40de-4b24-b224-4a24e65ef73e",
              "status": "investigating",
              "description": "We are investigating the issue.",
              "createdAt": "2026-06-29T14:30:55.653313Z",
              "componentsAffected": [ { "id": "490e2bc5-bd93-4925-93e4-a681ca4f9573", "name": "API", "status": "degraded" } ]
            }
          ]
        }
      ],
      "maintenances": [
        {
          "id": "88b6362e-a35a-4e9e-9b1b-a5611fd6dcca",
          "title": "Dashboard will be undergoing maintenance",
          "currentStatus": "completed",
          "publishedDate": "2026-02-09T22:04:29.279573Z",
          "startDate": "2026-02-10T10:00:00Z",
          "completedDate": "2026-02-10T10:30:00Z",
          "timeline": [
            { "id": "e8ee3494-9dfd-47e2-9371-a039867150a3", "status": "completed", "description": "Maintenance is complete", "createdAt": "2026-02-10T10:30:13.397338Z" }
          ]
        }
      ]
    }"#;

    fn page_url() -> Url {
        Url::parse("https://demo-status-page.statuspage.datadoghq.com").unwrap()
    }

    fn incidents() -> Vec<Incident> {
        decode(CONFIG.as_bytes(), &page_url()).unwrap()
    }

    #[test]
    fn timeline_entries_become_updates() {
        let incident = &incidents()[0];
        assert_eq!(incident.id, "3db838fd-93e6-4f48-8be5-788e67121f37");
        assert_eq!(incident.name, "Current issue");
        assert_eq!(incident.components, ["API"]);
        let updates = &incident.updates;
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].id, "b95517f7-ddd0-4afb-b9c1-511f606d6008");
        assert_eq!(updates[0].state, IncidentState::Resolved);
        assert_eq!(updates[0].body, "The issue is now resolved.");
        assert_eq!(updates[0].created_at, ts("2026-06-29T14:32:08.951454Z"));
        assert_eq!(updates[1].state, IncidentState::Investigating);
    }

    #[test]
    fn impact_is_the_worst_component_status_across_the_timeline() {
        // Every component is `operational` now; it was `degraded` during the incident.
        assert_eq!(incidents()[0].impact, Some(Impact::Minor));

        let worst = decode(
            br#"{"incidents":[{"id":"i","title":"t","timeline":[
              {"id":"a","createdAt":"2026-06-29T14:00:00Z","componentsAffected":[{"name":"API","status":"partial_outage"}]},
              {"id":"b","createdAt":"2026-06-29T14:10:00Z","componentsAffected":[{"name":"Web","status":"major_outage"},{"name":"API","status":"operational"}]}
            ]}]}"#,
            &page_url(),
        )
        .unwrap();
        assert_eq!(worst[0].impact, Some(Impact::Critical));
    }

    #[test]
    fn incident_without_timeline_components_has_unknown_impact() {
        let decoded = decode(
            br#"{"incidents":[{"id":"i","title":"t","timeline":[{"id":"a","status":"investigating","createdAt":"2026-06-29T14:00:00Z"}]}]}"#,
            &page_url(),
        )
        .unwrap();
        assert_eq!(decoded[0].impact, None);
    }

    #[test]
    fn all_operational_components_mean_no_impact_not_unknown() {
        let decoded = decode(
            br#"{"incidents":[{"id":"i","title":"t","timeline":[{"id":"a","createdAt":"2026-06-29T14:00:00Z","componentsAffected":[{"name":"API","status":"operational"}]}]}]}"#,
            &page_url(),
        )
        .unwrap();
        assert_eq!(decoded[0].impact, Some(Impact::None));
    }

    #[test]
    fn timeline_entry_without_a_readable_time_is_dropped() {
        let decoded = decode(
            br#"{"incidents":[{"id":"i","title":"t","timeline":[
              {"id":"good","createdAt":"2026-06-29T14:00:00Z"},
              {"id":"bad","createdAt":"0001-01-01"},
              {"id":"none"}
            ]}]}"#,
            &page_url(),
        )
        .unwrap();
        let ids: Vec<&str> = decoded[0].updates.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["good"]);
    }

    #[test]
    fn maintenances_are_ignored() {
        assert_eq!(incidents().len(), 1);
    }

    #[test]
    fn link_points_at_the_incident_page() {
        assert_eq!(
            incidents()[0].link.as_deref(),
            Some(
                "https://demo-status-page.statuspage.datadoghq.com/3db838fd-93e6-4f48-8be5-788e67121f37"
            )
        );
    }

    #[test]
    fn json_without_incidents_array_is_not_datadog() {
        assert!(matches!(
            decode(br#"{"name":"x","maintenances":[]}"#, &page_url()),
            Err(DecodeError::Json(_))
        ));
    }
}
