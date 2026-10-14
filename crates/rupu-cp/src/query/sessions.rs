//! `?q=` on `GET /api/sessions` and `GET /api/projects/:id/sessions`: over
//! a session row (`session.json` + `scope` + attribution) tagged with
//! `host_id`.

use rupu_query::{FieldDef, FieldKind, MatchMode, RowField};

use super::shared::{CODENAME, CUSTOMER, HOST};
use super::{opt, Fields};

pub static FIELDS: Fields = &[
    RowField {
        def: FieldDef::one_of("status", &["idle", "running", "failed", "stopped"]),
        get: |r| opt(r, "status"),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::new("agent", FieldKind::Text),
        get: |r| opt(r, "agent_name"),
        mode: MatchMode::Contains,
        text: true,
        facet: true,
    },
    RowField {
        def: FieldDef::new("model", FieldKind::Text),
        get: |r| opt(r, "model"),
        mode: MatchMode::Contains,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::new("provider", FieldKind::Text),
        get: |r| opt(r, "provider_name"),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    CODENAME,
    RowField {
        def: FieldDef::new("id", FieldKind::Text),
        get: |r| opt(r, "session_id"),
        mode: MatchMode::Prefix,
        text: true,
        facet: false,
    },
    HOST,
    CUSTOMER,
    RowField {
        def: FieldDef::new("project", FieldKind::Text).aliased(&["workspace"]),
        get: |r| opt(r, "workspace_id"),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::one_of("is", &["active", "archived"]),
        get: |r| r.get("scope").map(|_| opt(r, "scope").unwrap_or_default()),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::new("since", FieldKind::Time),
        get: |r| opt(r, "created_at"),
        mode: MatchMode::Since,
        text: false,
        facet: false,
    },
    RowField {
        def: FieldDef::new("until", FieldKind::Time),
        get: |r| opt(r, "created_at"),
        mode: MatchMode::Until,
        text: false,
        facet: false,
    },
];
