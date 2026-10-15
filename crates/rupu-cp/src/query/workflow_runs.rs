//! `?q=` on `GET /api/runs/workflows` and `GET /api/projects/:id/runs`:
//! over a [`crate::api::runs::RunListRow`] tagged with `host_id` (and
//! `archived: true` on a row from the archive). Each field is a named const
//! so [`super::autoflow_runs`] lists the same entries, never a copy.

use std::borrow::Cow;

use rupu_query::{FieldDef, FieldKind, MatchMode, RowField};
use serde_json::Value;

use super::shared::{CODENAME, CUSTOMER, HOST};
use super::{opt, Fields};

/// `RunStatus::as_str`, every value.
pub const STATUSES: &[&str] = &[
    "pending",
    "running",
    "awaiting_approval",
    "paused",
    "completed",
    "failed",
    "rejected",
    "cancelled",
];

fn archived(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    Some(match row.get("archived") {
        Some(Value::Bool(true)) => vec![Cow::Borrowed("archived")],
        _ => Vec::new(),
    })
}

pub const STATUS: RowField<Value> = RowField {
    def: FieldDef::one_of("status", STATUSES),
    get: |r| opt(r, "status"),
    mode: MatchMode::Exact,
    text: false,
    facet: true,
};

pub const WORKFLOW: RowField<Value> = RowField {
    def: FieldDef::new("workflow", FieldKind::Text),
    get: |r| opt(r, "workflow_name"),
    mode: MatchMode::Contains,
    text: true,
    facet: true,
};

pub const ID: RowField<Value> = RowField {
    def: FieldDef::new("id", FieldKind::Text),
    get: |r| opt(r, "id"),
    mode: MatchMode::Prefix,
    text: true,
    facet: false,
};

pub const TRIGGER: RowField<Value> = RowField {
    def: FieldDef::one_of("trigger", &["manual", "cron", "event"]),
    get: |r| opt(r, "trigger"),
    mode: MatchMode::Exact,
    text: false,
    facet: true,
};

pub const SINCE: RowField<Value> = RowField {
    def: FieldDef::new("since", FieldKind::Time),
    get: |r| opt(r, "started_at"),
    mode: MatchMode::Since,
    text: false,
    facet: false,
};

pub const UNTIL: RowField<Value> = RowField {
    def: FieldDef::new("until", FieldKind::Time),
    get: |r| opt(r, "started_at"),
    mode: MatchMode::Until,
    text: false,
    facet: false,
};

pub const IS: RowField<Value> = RowField {
    def: FieldDef::one_of("is", &["archived"]),
    get: archived,
    mode: MatchMode::Exact,
    text: false,
    facet: false,
};

pub static FIELDS: Fields = &[
    STATUS, WORKFLOW, ID, CODENAME, TRIGGER, HOST, CUSTOMER, SINCE, UNTIL, IS,
];
