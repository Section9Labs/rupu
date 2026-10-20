//! Query-language help: each list's field registry, and a check of a query
//! against it without running it. The registries are the ones the lists
//! filter with — findings' `rupu_coverage::FIELDS` and the CP's
//! [`crate::query`] row registries — so the answer is what the list would
//! do.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::Path;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/query/:list/fields", get(fields))
        .route("/api/query/:list/check", post(check))
}

/// The lists that take a `q`, by the name `:list` uses.
const LISTS: &[&str] = &[
    "findings",
    "workflow_runs",
    "agent_runs",
    "sessions",
    "autoflow_runs",
    "events",
];

/// A list's registry: findings' own field definitions, or a row registry.
enum Registry {
    Findings,
    Rows(crate::query::Fields),
}

fn registry(list: &str) -> ApiResult<Registry> {
    use crate::query::{agent_runs, autoflow_runs, events, sessions, workflow_runs};
    Ok(match list {
        "findings" => Registry::Findings,
        "workflow_runs" => Registry::Rows(workflow_runs::FIELDS),
        "agent_runs" => Registry::Rows(agent_runs::FIELDS),
        "sessions" => Registry::Rows(sessions::FIELDS),
        "autoflow_runs" => Registry::Rows(autoflow_runs::FIELDS),
        "events" => Registry::Rows(events::FIELDS),
        other => {
            return Err(ApiError::not_found(format!(
                "no query list `{other}`; the lists are {}",
                LISTS.join(", ")
            )))
        }
    })
}

/// `GET /api/query/:list/fields` — `{list, fields}`: the list's registry in
/// suggestion order, in the shape `tests/fixtures/list_query/` and
/// findings' `fields.json` pin. `404` for an unknown list.
async fn fields(Path(list): Path<String>) -> ApiResult<Json<Value>> {
    let fields = match registry(&list)? {
        Registry::Findings => serde_json::to_value(rupu_coverage::FIELDS),
        Registry::Rows(fields) => serde_json::to_value(fields),
    }
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "list": list, "fields": fields })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckBody {
    /// The query to check, as the list's `q` would take it.
    q: String,
}

/// `POST /api/query/:list/check` — parse `q` with the list's registry.
/// `200 {ok: true, terms}` (each `{neg, key, op, values}`, the key an alias
/// resolves to, the values normalized) or `200 {ok: false, error, token,
/// code, start, end}`, the body of the list's own 400: the check itself
/// succeeded. `404` for an unknown list.
async fn check(Path(list): Path<String>, Json(body): Json<CheckBody>) -> ApiResult<Json<Value>> {
    let terms = match registry(&list)? {
        Registry::Findings => {
            rupu_coverage::parse_query(&body.q).map(|p| serde_json::to_value(p.terms))
        }
        Registry::Rows(fields) => rupu_query::parse_query(&body.q, &rupu_query::defs(fields))
            .map(|p| serde_json::to_value(p.terms)),
    };
    Ok(Json(match terms {
        Ok(terms) => {
            let terms = terms.map_err(|e| ApiError::internal(e.to_string()))?;
            json!({ "ok": true, "terms": terms })
        }
        Err(e) => {
            let mut v = crate::api::findings::query_error_body(&e);
            v["ok"] = json!(false);
            v
        }
    }))
}
