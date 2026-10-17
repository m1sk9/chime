use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{DecodeError, Incident, IncidentState, IncidentUpdate, parse_time};
use crate::config::{CustomMapping, Impact, JsonPointer};

#[derive(Debug, thiserror::Error)]
pub enum MappingError {
    #[error("nothing at JSON pointer `{0}`")]
    Missing(String),
    #[error("value at JSON pointer `{0}` is not a string or number")]
    NotScalar(String),
    #[error("value at JSON pointer `{0}` is a fractional number; serve it as a string")]
    Fractional(String),
    #[error("value at JSON pointer `{0}` is empty")]
    Empty(String),
    #[error("value at JSON pointer `{0}` is not an array")]
    NotArray(String),
    #[error("value at JSON pointer `{0}` is not an RFC 3339 timestamp")]
    Timestamp(String),
}

/// Pure. Each element of the incident array is one incident with one update.
///
/// Why one bad element fails the whole body rather than being skipped: the
/// mapping is the user's own config, and a pointer that misses has to show up as
/// a `warn` on every poll, as a `json` watch's pointer does, not as silence.
pub(super) fn decode(
    mapping: &CustomMapping,
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<Vec<Incident>, DecodeError> {
    let doc: Value = serde_json::from_slice(body)?;
    // `""` addresses the whole document.
    let pointer = mapping.incidents.as_ref().map_or("", JsonPointer::as_str);
    let items = doc
        .pointer(pointer)
        .ok_or_else(|| MappingError::Missing(pointer.to_string()))?
        .as_array()
        .ok_or_else(|| MappingError::NotArray(pointer.to_string()))?;
    Ok(items
        .iter()
        .map(|item| incident(mapping, item, now))
        .collect::<Result<_, _>>()?)
}

fn incident(
    mapping: &CustomMapping,
    item: &Value,
    now: DateTime<Utc>,
) -> Result<Incident, MappingError> {
    let id = required(item, &mapping.id)?;
    let title = required(item, &mapping.title)?;
    let status = optional(item, mapping.status.as_ref())?;
    let updated_at = optional(item, mapping.updated_at.as_ref())?;
    let created_at = match (&updated_at, &mapping.updated_at) {
        (Some(raw), Some(pointer)) => {
            parse_time(raw).ok_or_else(|| MappingError::Timestamp(pointer.as_str().to_string()))?
        }
        _ => now,
    };
    // Without `updated_at`, a change of status is the only change chime can see;
    // without either, the incident is reported once, when it first appears.
    let update_id = updated_at.or_else(|| status.clone()).unwrap_or_default();
    Ok(Incident {
        id,
        name: title,
        impact: optional(item, mapping.impact.as_ref())?
            .and_then(|i| Impact::from_wire(&i.to_ascii_lowercase())),
        link: optional(item, mapping.link.as_ref())?,
        components: Vec::new(),
        updates: vec![IncidentUpdate {
            id: update_id,
            state: status
                .as_deref()
                .map_or(IncidentState::Unknown, IncidentState::from_wire),
            body: optional(item, mapping.body.as_ref())?.unwrap_or_default(),
            created_at,
        }],
    })
}

fn required(item: &Value, pointer: &JsonPointer) -> Result<String, MappingError> {
    let pointer = pointer.as_str();
    match scalar(item, pointer)? {
        None => Err(MappingError::Missing(pointer.to_string())),
        Some(s) if s.is_empty() => Err(MappingError::Empty(pointer.to_string())),
        Some(s) => Ok(s),
    }
}

/// An unset pointer, a missing value, `null` and `""` all read as absent: an
/// incident that has not been resolved yet may well have no `resolved_at`.
fn optional(item: &Value, pointer: Option<&JsonPointer>) -> Result<Option<String>, MappingError> {
    let Some(pointer) = pointer else {
        return Ok(None);
    };
    Ok(scalar(item, pointer.as_str())?.filter(|s| !s.is_empty()))
}

/// Same rule as a `json` watch: strings and integers only, because a fractional
/// number goes through `f64` and `1.10` would read back as `1.1`.
fn scalar(item: &Value, pointer: &str) -> Result<Option<String>, MappingError> {
    match item.pointer(pointer) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => Ok(Some(n.to_string())),
        Some(Value::Number(_)) => Err(MappingError::Fractional(pointer.to_string())),
        Some(_) => Err(MappingError::NotScalar(pointer.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::ts;

    fn mapping(toml_inline: &str) -> CustomMapping {
        #[derive(serde::Deserialize)]
        struct Wrap {
            mapping: CustomMapping,
        }
        let wrap: Wrap = toml::from_str(&format!("mapping = {toml_inline}")).unwrap();
        wrap.mapping
    }

    /// The mapping shown in the README.
    fn documented() -> CustomMapping {
        mapping(
            r#"{ incidents = "/incidents", id = "/id", title = "/title", status = "/state", body = "/message", updated_at = "/updated_at", link = "/url", impact = "/severity" }"#,
        )
    }

    fn now() -> DateTime<Utc> {
        ts("2026-10-17T14:00:00Z")
    }

    fn decode_with(m: &CustomMapping, json: &str) -> Result<Vec<Incident>, DecodeError> {
        decode(m, json.as_bytes(), now())
    }

    fn mapping_error(r: Result<Vec<Incident>, DecodeError>) -> MappingError {
        match r {
            Err(DecodeError::Mapping(e)) => e,
            other => panic!("expected a mapping error, got {other:?}"),
        }
    }

    #[test]
    fn maps_the_documented_example() {
        let incidents = decode_with(
            &documented(),
            r#"{"incidents":[{
              "id":"inc-1","title":"API errors","state":"Investigating",
              "message":"Looking into it.","updated_at":"2026-10-17T13:00:00Z",
              "url":"https://status.example.com/inc-1","severity":"major"
            }]}"#,
        )
        .unwrap();
        assert_eq!(
            incidents,
            [Incident {
                id: "inc-1".to_string(),
                name: "API errors".to_string(),
                impact: Some(Impact::Major),
                link: Some("https://status.example.com/inc-1".to_string()),
                components: Vec::new(),
                updates: vec![IncidentUpdate {
                    id: "2026-10-17T13:00:00Z".to_string(),
                    state: IncidentState::Investigating,
                    body: "Looking into it.".to_string(),
                    created_at: ts("2026-10-17T13:00:00Z"),
                }],
            }]
        );
    }

    #[test]
    fn root_array_when_incidents_pointer_is_absent() {
        let m = mapping(r#"{ id = "/id", title = "/title" }"#);
        let incidents =
            decode_with(&m, r#"[{"id":"a","title":"A"},{"id":"b","title":"B"}]"#).unwrap();
        assert_eq!(incidents.len(), 2);
        assert_eq!(incidents[1].name, "B");
    }

    #[test]
    fn incidents_pointer_must_reach_an_array() {
        let m = documented();
        assert!(matches!(
            mapping_error(decode_with(&m, r#"{"incidents":{"id":"a"}}"#)),
            MappingError::NotArray(p) if p == "/incidents"
        ));
        assert!(matches!(
            mapping_error(decode_with(&m, r#"{"data":[]}"#)),
            MappingError::Missing(p) if p == "/incidents"
        ));
    }

    #[test]
    fn missing_id_fails_the_whole_decode() {
        let r = decode_with(
            &documented(),
            r#"{"incidents":[{"id":"a","title":"A"},{"title":"no id"}]}"#,
        );
        assert!(matches!(mapping_error(r), MappingError::Missing(p) if p == "/id"));
    }

    #[test]
    fn empty_or_non_scalar_title_fails_the_decode() {
        let m = documented();
        assert!(matches!(
            mapping_error(decode_with(&m, r#"{"incidents":[{"id":"a","title":"  "}]}"#)),
            MappingError::Empty(p) if p == "/title"
        ));
        assert!(matches!(
            mapping_error(decode_with(&m, r#"{"incidents":[{"id":"a","title":["x"]}]}"#)),
            MappingError::NotScalar(p) if p == "/title"
        ));
    }

    #[test]
    fn integer_id_is_accepted_fractional_is_not() {
        let m = documented();
        let incidents = decode_with(&m, r#"{"incidents":[{"id":42,"title":"A"}]}"#).unwrap();
        assert_eq!(incidents[0].id, "42");
        assert!(matches!(
            mapping_error(decode_with(&m, r#"{"incidents":[{"id":4.2,"title":"A"}]}"#)),
            MappingError::Fractional(p) if p == "/id"
        ));
    }

    #[test]
    fn status_and_impact_are_matched_case_insensitively() {
        let incidents = decode_with(
            &documented(),
            r#"{"incidents":[
              {"id":"a","title":"A","state":"RESOLVED","severity":"Critical"},
              {"id":"b","title":"B","state":"degraded","severity":"sev1"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(incidents[0].updates[0].state, IncidentState::Resolved);
        assert_eq!(incidents[0].impact, Some(Impact::Critical));
        assert_eq!(incidents[1].updates[0].state, IncidentState::Unknown);
        assert_eq!(
            incidents[1].impact, None,
            "an unknown severity is unknown, not dropped"
        );
    }

    #[test]
    fn updated_at_is_the_dedup_key_and_timestamp() {
        let incidents = decode_with(
            &documented(),
            r#"{"incidents":[{"id":"a","title":"A","state":"identified","updated_at":"2026-10-17T12:30:00+09:00"}]}"#,
        )
        .unwrap();
        let update = &incidents[0].updates[0];
        assert_eq!(update.id, "2026-10-17T12:30:00+09:00");
        assert_eq!(update.created_at, ts("2026-10-17T03:30:00Z"));
    }

    #[test]
    fn absent_updated_at_falls_back_to_status_then_to_a_constant_key() {
        let incidents = decode_with(
            &documented(),
            r#"{"incidents":[
              {"id":"a","title":"A","state":"monitoring"},
              {"id":"b","title":"B","updated_at":null}
            ]}"#,
        )
        .unwrap();
        assert_eq!(incidents[0].updates[0].id, "monitoring");
        assert_eq!(incidents[1].updates[0].id, "");
    }

    #[test]
    fn absent_updated_at_uses_detection_time() {
        let m = mapping(r#"{ id = "/id", title = "/title" }"#);
        let incidents = decode_with(
            &m,
            r#"[{"id":"a","title":"A","updated_at":"2020-01-01T00:00:00Z"}]"#,
        )
        .unwrap();
        assert_eq!(
            incidents[0].updates[0].created_at,
            now(),
            "a field the mapping does not name is not read"
        );
    }

    #[test]
    fn unparsable_updated_at_is_an_error() {
        let r = decode_with(
            &documented(),
            r#"{"incidents":[{"id":"a","title":"A","updated_at":"yesterday"}]}"#,
        );
        assert!(matches!(mapping_error(r), MappingError::Timestamp(p) if p == "/updated_at"));
    }

    #[test]
    fn nested_pointer_into_the_element_works() {
        let m = mapping(
            r#"{ incidents = "/data/items", id = "/meta/0/id", title = "/attributes/name", body = "/attributes/updates/0/text" }"#,
        );
        let incidents = decode_with(
            &m,
            r#"{"data":{"items":[{"meta":[{"id":"deep"}],"attributes":{"name":"N","updates":[{"text":"latest"}]}}]}}"#,
        )
        .unwrap();
        assert_eq!(incidents[0].id, "deep");
        assert_eq!(incidents[0].name, "N");
        assert_eq!(incidents[0].updates[0].body, "latest");
    }

    #[test]
    fn non_json_body_is_a_json_error() {
        assert!(matches!(
            decode_with(&documented(), "<html>"),
            Err(DecodeError::Json(_))
        ));
    }
}
