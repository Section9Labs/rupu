//! Authenticated operator decisions for exact pending session tool calls.
//! These routes are deliberately excluded from MCP tools and self-grants.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header::CACHE_CONTROL, HeaderValue, StatusCode},
    middleware::{from_fn, Next},
    response::Response,
    routing::{get, post, put},
    Json, Router,
};
use rupu_runtime::session_approvals::{
    broker::Broker,
    store::{self, DecisionRequest, RevokeRequest},
    view::ApprovalView,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub const FEATURE: &str = "session.tool_approvals.v1";

#[cfg(test)]
#[path = "session_approvals_tests.rs"]
mod tests;

pub fn status(s: &AppState) -> Value {
    let broker = s
        .session_starter
        .as_ref()
        .and_then(|starter| starter.approval_broker());
    let ready = broker.as_ref().is_some_and(|broker| broker.is_ready());
    let supported = s
        .session_starter
        .as_ref()
        .is_some_and(|starter| starter.supports_session_approvals());
    let reason = if !s.token_set {
        "authentication_required"
    } else if broker.is_none() {
        "disabled"
    } else if !ready {
        "broker_unavailable"
    } else if !supported {
        "worker_channel_unavailable"
    } else {
        "ready"
    };
    json!({"feature":FEATURE,"enabled":available(s),"authenticated":s.token_set,"broker_ready":ready,"reason":reason,"scope":"root_and_in_process_children","default_mode_changed":false})
}

pub fn available(s: &AppState) -> bool {
    s.token_set
        && s.session_starter.as_ref().is_some_and(|starter| {
            starter.supports_session_approvals()
                && starter
                    .approval_broker()
                    .is_some_and(|broker| broker.is_ready())
        })
}
fn broker(s: &AppState, host: Option<&str>) -> ApiResult<Arc<Broker>> {
    super::fleet::require_local(host, "session tool approvals")?;
    if !available(s) {
        return Err(ApiError::not_available("Tool approvals require an authenticated CP with its private session approval channel enabled."));
    }
    s.session_starter
        .as_ref()
        .and_then(|s| s.approval_broker())
        .ok_or_else(|| ApiError::not_available("The approval broker is unavailable."))
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/:id/approvals", get(list))
        .route("/api/sessions/:id/approvals/:approval", get(detail))
        .route(
            "/api/sessions/:id/approvals/:approval/decision",
            put(decide),
        )
        .route("/api/sessions/:id/approvals/:approval/revoke", post(revoke))
        .layer(DefaultBodyLimit::max(8 * 1024))
        .layer(from_fn(no_store))
}
async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Local {
    host: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    host: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}
fn valid_id(id: &str) -> ApiResult<()> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(ApiError::bad_request("Invalid approval identity."));
    }
    Ok(())
}
fn error(e: store::Error) -> ApiError {
    match e {
        store::Error::NotFound => {
            ApiError::not_found("No approval belongs to this exact session and ID.")
        }
        store::Error::Conflict => ApiError::conflict(
            "Approval state or authority changed. Read its current status before acting.",
        ),
        store::Error::Expired => ApiError(
            StatusCode::GONE,
            "This approval expired before dispatch.".into(),
        ),
        store::Error::Invalid => ApiError::bad_request("Invalid approval request or cursor."),
        store::Error::Capacity => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Approval capacity reached; unresolved evidence is preserved.".into(),
        ),
        store::Error::Io(_) => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Approval storage is unavailable; the decision outcome may be unknown. Read this approval before taking another action.".into(),
        ),
    }
}
fn envelope(b: &Broker, id: &str) -> Value {
    json!({"schema_version":1,"cp_instance_id":b.instance_id(),"broker_epoch":b.epoch(),"host_id":"local","session_id":id,"authenticated":true,"broker_ready":b.is_ready()})
}
async fn project(b: Arc<Broker>, record: store::Record, detail: bool) -> ApiResult<ApprovalView> {
    tokio::task::spawn_blocking(move || {
        ApprovalView::from_record(&record, detail, b.live_record(&record), b.now())
    })
    .await
    .map_err(|_| ApiError::internal("Approval observation failed."))
}
async fn list(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    valid_id(&id)?;
    let b = broker(&s, q.host.as_deref())?;
    let limit = q.limit.unwrap_or(50);
    if !(1..=50).contains(&limit) || q.cursor.as_ref().is_some_and(|c| c.len() > 8192) {
        return Err(ApiError::bad_request(
            "Approval pages require limit 1–50 and a bounded issued cursor.",
        ));
    }
    let page = b.list(id.clone(), q.cursor, limit).await.map_err(error)?;
    let projection_b = b.clone();
    let records = tokio::task::spawn_blocking(move || {
        page.records
            .iter()
            .map(|r| {
                ApprovalView::from_record(r, false, projection_b.live_record(r), projection_b.now())
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|_| ApiError::internal("Approval observation failed."))?;
    let mut value = envelope(&b, &id);
    value["approvals"] = serde_json::to_value(records)
        .map_err(|_| ApiError::internal("Invalid approval projection."))?;
    value["next_cursor"] = json!(page.next_cursor);
    Ok(Json(value))
}
async fn detail(
    State(s): State<AppState>,
    Path((id, approval)): Path<(String, String)>,
    Query(q): Query<Local>,
) -> ApiResult<Json<Value>> {
    valid_id(&id)?;
    valid_id(&approval)?;
    let b = broker(&s, q.host.as_deref())?;
    let record = b.record(id.clone(), approval).await.map_err(error)?;
    let mut value = envelope(&b, &id);
    value["approval"] = serde_json::to_value(project(b, record, true).await?)
        .map_err(|_| ApiError::internal("Invalid approval projection."))?;
    Ok(Json(value))
}
async fn decide(
    State(s): State<AppState>,
    Path((id, approval)): Path<(String, String)>,
    Query(q): Query<Local>,
    Json(request): Json<DecisionRequest>,
) -> ApiResult<Json<Value>> {
    valid_id(&id)?;
    valid_id(&approval)?;
    let b = broker(&s, q.host.as_deref())?;
    let record = b
        .decide(id.clone(), approval, request)
        .await
        .map_err(error)?;
    let mut value = envelope(&b, &id);
    value["approval"] = serde_json::to_value(project(b, record, true).await?)
        .map_err(|_| ApiError::internal("Invalid approval projection."))?;
    Ok(Json(value))
}
async fn revoke(
    State(s): State<AppState>,
    Path((id, approval)): Path<(String, String)>,
    Query(q): Query<Local>,
    Json(request): Json<RevokeRequest>,
) -> ApiResult<Json<Value>> {
    valid_id(&id)?;
    valid_id(&approval)?;
    let b = broker(&s, q.host.as_deref())?;
    let record = b
        .revoke(id.clone(), approval, request)
        .await
        .map_err(error)?;
    let mut value = envelope(&b, &id);
    value["approval"] = serde_json::to_value(project(b, record, true).await?)
        .map_err(|_| ApiError::internal("Invalid approval projection."))?;
    Ok(Json(value))
}
