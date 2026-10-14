//! `?q=` on `GET /api/events`: over an event row (the tagged executor
//! `Event` plus `ts`, `pos` and its run's attribution: `workflow`,
//! `project`, `run_codename`). The Live Events page evaluates the same
//! registry over its stream cards, live frames and findings included
//! (`web/src/lib/query/lists/events.ts`), reading an event card's own keys;
//! `tests/fixtures/list_query/events.matches.json` holds both to the same
//! answers.

use std::borrow::Cow;

use rupu_query::{FieldDef, FieldKind, MatchMode, RowField};
use serde_json::Value;

use super::{opt, Fields};

/// The groups a stream card answers to. A finding is a card the page makes
/// from the findings list, never an event, so no event row is in it.
pub const GROUPS: &[&str] = &["finding", "activity", "await", "warning", "error"];

/// The fields that read the event's run rather than the event: a run whose
/// record fails one of their terms has no event that passes it.
pub const RUN_KEYS: &[&str] = &["run", "project", "workflow"];

/// An event type's card group, as the page's `cardFromEvent` assigns it
/// (held in lockstep by `tests/fixtures/list_query/events.groups.json`).
pub fn group_of(event_type: &str) -> &'static str {
    match event_type {
        "run_failed" | "step_failed" => "error",
        "step_warning" => "warning",
        "step_awaiting_approval" => "await",
        _ => "activity",
    }
}

fn group(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    let t = row.get("type").and_then(Value::as_str).unwrap_or_default();
    Some(vec![Cow::Borrowed(group_of(t))])
}

fn codenames(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    let mut out = opt(row, "codename")?;
    out.extend(opt(row, "run_codename")?);
    Some(out)
}

/// What the event says in words: an error, a reason, a warning, a note.
fn message(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    let mut out = Vec::new();
    for key in ["error", "reason", "message", "note"] {
        out.extend(opt(row, key)?);
    }
    Some(out)
}

/// `ts` (unix ms) as an RFC 3339 time.
fn time(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    Some(
        row.get("ts")
            .and_then(Value::as_i64)
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| Cow::Owned(t.to_rfc3339()))
            .into_iter()
            .collect(),
    )
}

const SEVERITIES: &[&str] = &["info", "low", "medium", "high", "critical"];

pub static FIELDS: Fields = &[
    RowField {
        def: FieldDef::one_of("group", GROUPS),
        get: group,
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::new("type", FieldKind::Text),
        get: |r| opt(r, "type"),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::new("run", FieldKind::Text),
        get: |r| opt(r, "run_id"),
        mode: MatchMode::Prefix,
        text: true,
        facet: false,
    },
    RowField {
        def: FieldDef::new("project", FieldKind::Text),
        get: |r| opt(r, "project"),
        mode: MatchMode::Contains,
        text: true,
        facet: true,
    },
    RowField {
        def: FieldDef::new("workflow", FieldKind::Text),
        get: |r| opt(r, "workflow"),
        mode: MatchMode::Contains,
        text: true,
        facet: true,
    },
    RowField {
        def: FieldDef::new("agent", FieldKind::Text),
        get: |r| opt(r, "agent"),
        mode: MatchMode::Contains,
        text: true,
        facet: true,
    },
    RowField {
        def: FieldDef::new("step", FieldKind::Text),
        get: |r| opt(r, "step_id"),
        mode: MatchMode::Contains,
        text: true,
        facet: true,
    },
    RowField {
        def: FieldDef::new("codename", FieldKind::Text),
        get: codenames,
        mode: MatchMode::Contains,
        text: true,
        facet: false,
    },
    RowField {
        def: FieldDef::new("provider", FieldKind::Text),
        get: |r| opt(r, "provider"),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::new("model", FieldKind::Text),
        get: |r| opt(r, "model"),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef {
            key: "severity",
            aliases: &["sev"],
            kind: FieldKind::Severity,
            values: SEVERITIES,
        },
        get: |r| opt(r, "severity"),
        mode: MatchMode::Ordered,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::new("message", FieldKind::Text),
        get: message,
        mode: MatchMode::Contains,
        text: true,
        facet: false,
    },
    RowField {
        def: FieldDef::new("since", FieldKind::Time),
        get: time,
        mode: MatchMode::Since,
        text: false,
        facet: false,
    },
    RowField {
        def: FieldDef::new("until", FieldKind::Time),
        get: time,
        mode: MatchMode::Until,
        text: false,
        facet: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::ListQuery;
    use serde_json::json;

    fn keeps(q: &str, row: &Value) -> bool {
        ListQuery::parse(Some(q), FIELDS)
            .unwrap()
            .unwrap()
            .matches(row)
            .unwrap()
    }

    #[test]
    fn an_event_row_matches_by_its_group_attribution_and_words() {
        let failed = json!({
            "type": "step_failed", "run_id": "run_01J9", "step_id": "scan",
            "error": "connection refused", "ts": 1_759_752_000_000_i64,
            "workflow": "nightly", "project": "shop-web", "run_codename": "cobalt-harbor",
        });
        assert!(keeps("group:error", &failed));
        assert!(!keeps("-group:error", &failed));
        assert!(keeps(
            "run:run_01 workflow:night project:shop step:scan",
            &failed
        ));
        assert!(keeps("codename:cobalt", &failed));
        assert!(keeps("refused", &failed));
        assert!(keeps("since:2025-10-06 until:2025-10-06", &failed));
        assert!(!keeps("since:2025-10-07", &failed));
        assert!(!keeps("severity>=low", &failed));
        assert!(keeps("-severity:critical", &failed));
    }
}
