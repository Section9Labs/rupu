//! `?q=` on `GET /api/runs/agents`: over its agent-run rows (standalone runs
//! and session turns) tagged with `host_id`.

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
