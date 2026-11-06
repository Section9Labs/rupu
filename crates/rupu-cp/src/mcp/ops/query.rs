//! The query-help operator tools: a list's query fields, and a check of a
//! query against them.

use super::{op, OpSpec};
use crate::api::query;
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "query.fields",
            Method::GET,
            "/api/query/:list/fields",
            "List the fields a list's query (`q`) can filter on, with their kinds, aliases and values.\n\n`list` is findings, workflow_runs, agent_runs, sessions, autoflow_runs or events. Returns `{list, fields}` in suggestion order, each field with its `key`, `aliases`, `kind` (severity, enum, text, time or a custom kind such as tag) and allowed `values`; the run and session lists also say how each field matches. `HTTP 404` for an unknown list.",
        ),
        op(
            "query.check",
            Method::POST,
            "/api/query/:list/check",
            "Check a query against a list's fields without running it.\n\n`list` is as for `query.fields`; `q` is the query. Returns `{ok: true, terms}` (each term `{neg, key, op, values}`, aliases resolved and values normalized), or `{ok: false, error, token, code, start, end}`: the list's own error for that query, naming the offending token. `HTTP 404` for an unknown list.",
        )
        .body::<query::CheckBody>()
        .read_only(),
    ]
}
