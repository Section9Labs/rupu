//! `?q=` on `GET /api/runs/agents`: over its agent-run rows (standalone runs
//! and session turns) tagged with `host_id`.

use std::borrow::Cow;

use rupu_query::{FieldDef, FieldKind, Getter, MatchMode, RowField};
use serde_json::Value;

use super::shared::{CODENAME, CUSTOMER, HOST};
use super::{opt, Fields};

const fn text(
    key: &'static str,
    get: Getter<Value>,
    mode: MatchMode,
    text: bool,
    facet: bool,
) -> RowField<Value> {
    RowField {
        def: FieldDef::new(key, FieldKind::Text),
        get,
        mode,
        text,
        facet,
    }
}

pub static FIELDS: Fields = &[
    // Free-form: a standalone run's transcript status or a session turn's.
    text(
        "status",
        |r| opt(r, "status"),
        MatchMode::Exact,
        false,
        true,
    ),
    text(
        "agent",
        |r| opt(r, "agent"),
        MatchMode::Contains,
        true,
        true,
    ),
    text(
        "provider",
        |r| opt(r, "provider"),
        MatchMode::Exact,
        false,
        true,
    ),
    text(
        "model",
        |r| opt(r, "model"),
        MatchMode::Contains,
        false,
        true,
    ),
    RowField {
        def: FieldDef::one_of("is", &["running", "completed", "failed"]),
        get: lifecycle,
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::one_of("source", &["standalone", "session"]),
        get: |r| opt(r, "source"),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    text(
        "session",
        |r| opt(r, "session_id"),
        MatchMode::Prefix,
        false,
        false,
    ),
    text(
        "trigger",
        |r| opt(r, "trigger_source"),
        MatchMode::Exact,
        false,
        true,
    ),
    CODENAME,
    text("id", |r| opt(r, "run_id"), MatchMode::Prefix, true, false),
    HOST,
    CUSTOMER,
    RowField {
        def: FieldDef::new("since", FieldKind::Time),
        get: |r| opt(r, "started_at"),
        mode: MatchMode::Since,
        text: false,
        facet: false,
    },
    RowField {
        def: FieldDef::new("until", FieldKind::Time),
        get: |r| opt(r, "started_at"),
        mode: MatchMode::Until,
        text: false,
        facet: false,
    },
];

/// The lifecycle group a row's status falls in, as the old Running /
/// Completed / Failed tabs grouped it: a finished standalone run records no
/// status, so it reads as completed.
fn lifecycle(r: &Value) -> Option<Vec<Cow<'_, str>>> {
    let group = match r.get("status").and_then(Value::as_str) {
        Some("running" | "awaiting_approval" | "pending") => "running",
        Some("error" | "failed" | "rejected" | "aborted") => "failed",
        _ => "completed",
    };
    Some(vec![Cow::Borrowed(group)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn group(row: Value) -> String {
        lifecycle(&row).unwrap()[0].to_string()
    }

    #[test]
    fn lifecycle_groups_statuses_as_the_old_tabs_did() {
        assert_eq!(group(json!({"status": "running"})), "running");
        assert_eq!(group(json!({"status": "awaiting_approval"})), "running");
        assert_eq!(group(json!({"status": "aborted"})), "failed");
        assert_eq!(group(json!({"status": "error"})), "failed");
        assert_eq!(group(json!({"status": "ok"})), "completed");
        assert_eq!(group(json!({})), "completed");
        assert_eq!(group(json!({"status": null})), "completed");
    }
}
