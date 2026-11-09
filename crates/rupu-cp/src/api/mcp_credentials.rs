//! Authenticated operator HTTP only. Tokens are write-only and never echoed.
use super::mcp_servers::store;
use crate::{
    auth::VerifiedControlPlaneAuth,
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, Query, State},
    http::{header::CACHE_CONTROL, HeaderValue, StatusCode},
    routing::{delete, get},
    Extension, Json, Router,
};
use rupu_auth::mcp_credentials::{Binding, Error as StoreError, Receipt, Store};
use serde::Deserialize;
use serde_json::{json, Value};

enum CredentialFailure {
    Api(ApiError),
    Store(StoreError),
}
impl From<ApiError> for CredentialFailure {
    fn from(value: ApiError) -> Self {
        Self::Api(value)
    }
}
impl axum::response::IntoResponse for CredentialFailure {
    fn into_response(self) -> axum::response::Response {
        let (status, message, code, outcome) = match self {
            Self::Store(StoreError::OperationExists) => (
                StatusCode::CONFLICT,
                StoreError::OperationExists.to_string(),
                "operation_id_used",
                "unknown",
            ),
            Self::Store(StoreError::Conflict) => (
                StatusCode::CONFLICT,
                StoreError::Conflict.to_string(),
                "precondition_failed",
                "not_applied",
            ),
            Self::Store(
                error @ (StoreError::Invalid | StoreError::NotFound | StoreError::Capacity),
            ) => {
                let status = match error {
                    StoreError::NotFound => StatusCode::NOT_FOUND,
                    StoreError::Capacity => StatusCode::INSUFFICIENT_STORAGE,
                    _ => StatusCode::BAD_REQUEST,
                };
                (status, error.to_string(), "request_rejected", "not_applied")
            }
            Self::Store(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
                "storage_unavailable",
                "unknown",
            ),
            Self::Api(error) if error.0 == StatusCode::CONFLICT => {
                (error.0, error.1, "precondition_failed", "not_applied")
            }
            Self::Api(error)
                if error.0.is_client_error() || error.0 == StatusCode::NOT_IMPLEMENTED =>
            {
                (error.0, error.1, "request_rejected", "not_applied")
            }
            Self::Api(error) => (error.0, error.1, "storage_unavailable", "unknown"),
        };
        (
            status,
            Json(json!({"error":message,"code":code,"mutation_outcome":outcome})),
        )
            .into_response()
    }
}
type CredentialResult<T> = Result<T, CredentialFailure>;
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> CredentialResult<T> + Send + 'static,
) -> CredentialResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| CredentialFailure::Api(ApiError::internal("MCP credential task failed")))?
}

pub const FEATURE: &str = "mcp.credential_enrollment.v1";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/mcp/servers/:id/credential",
            get(status).put(enroll).delete(remove),
        )
        .route("/api/mcp/credential-operations/:id", get(operation))
        .route("/api/mcp/credential-orphans", get(orphans))
        .route(
            "/api/mcp/credential-orphans/:generation",
            delete(remove_orphan),
        )
        .layer(DefaultBodyLimit::max(24 * 1024))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response
                    .headers_mut()
                    .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response
            },
        ))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Local {
    host: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    host: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}
// Do not derive Debug: Enroll contains a secret.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enroll {
    operation_id: String,
    registry_revision: String,
    binding_generation: String,
    credential_revision: String,
    token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    operation_id: String,
    registry_revision: String,
    binding_generation: String,
    credential_revision: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrphanRemove {
    operation_id: String,
    registry_revision: String,
    binding: Binding,
    credential_revision: String,
}
fn gate(
    s: &AppState,
    host: Option<&str>,
    auth: &Option<Extension<VerifiedControlPlaneAuth>>,
    write: bool,
) -> ApiResult<()> {
    // This marker is installed only after the real bearer/cookie middleware
    // verifies the request. token_set/writable alone cannot manufacture it.
    if auth.is_none() {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "MCP credential management requires authenticated control-plane access".into(),
        ));
    }
    super::fleet::require_local(host, "host-owned MCP credentials")?;
    if write {
        super::config::require_writable_to(s, "enrolling MCP credentials")?;
    }
    Ok(())
}
fn map_error(error: StoreError) -> ApiError {
    match error {
        StoreError::Invalid => ApiError::bad_request(error.to_string()),
        StoreError::Conflict | StoreError::OperationExists => ApiError::conflict(error.to_string()),
        StoreError::NotFound => ApiError::not_found(error.to_string()),
        _ => ApiError::internal(error.to_string()),
    }
}
fn binding(snapshot: &store::Snapshot, id: &str) -> ApiResult<Binding> {
    let server = snapshot
        .registry
        .servers
        .get(id)
        .ok_or_else(|| ApiError::not_found("configured MCP server not found"))?;
    let generation = server.bearer_store.clone().ok_or_else(|| {
        ApiError::conflict("select host-owned token storage for this MCP server first")
    })?;
    let url = url::Url::parse(&server.url)
        .map_err(|_| ApiError::conflict("invalid MCP endpoint"))?
        .to_string();
    let binding = Binding {
        server_id: id.into(),
        url,
        generation,
    };
    binding.validate().map_err(map_error)?;
    Ok(binding)
}
fn referenced(snapshot: &store::Snapshot, value: &Binding) -> bool {
    binding(snapshot, &value.server_id).is_ok_and(|current| current == *value)
}
fn receipt(value: Receipt) -> Json<Value> {
    Json(json!({"version":1,"host_id":"local","scope":"trusted_host","receipt":value}))
}
async fn status(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Local>,
    auth: Option<Extension<VerifiedControlPlaneAuth>>,
) -> CredentialResult<Json<Value>> {
    gate(&s, q.host.as_deref(), &auth, false)?;
    blocking(move || {
        let snapshot = store::read(&s.global_dir)?;
        let owner = binding(&snapshot, &id)?;
        let status = Store::for_global(&s.global_dir).status(&owner).map_err(CredentialFailure::Store)?;
        Ok(Json(json!({"version":1,"host_id":"local","scope":"trusted_host","server_id":id,"registry_revision":snapshot.revision,"binding":owner,"credential":{"present":status.present,"revision":status.revision},"writable":s.launcher.is_some()})))
    }).await
}
async fn enroll(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Local>,
    auth: Option<Extension<VerifiedControlPlaneAuth>>,
    body: Result<Json<Enroll>, JsonRejection>,
) -> CredentialResult<Json<Value>> {
    gate(&s, q.host.as_deref(), &auth, true)?;
    let Json(body) =
        body.map_err(|_| ApiError::bad_request("invalid MCP credential enrollment request"))?;
    blocking(move || {
        let result =
            store::with_snapshot_locked(&s.global_dir, &body.registry_revision, |snapshot| {
                let owner = binding(snapshot, &id)?;
                if owner.generation != body.binding_generation {
                    return Err(ApiError::conflict("MCP credential binding changed"));
                }
                Ok(Store::for_global(&s.global_dir).mutate(
                    &owner,
                    &body.credential_revision,
                    &body.operation_id,
                    Some(&body.token),
                ))
            })?;
        result.map(receipt).map_err(CredentialFailure::Store)
    })
    .await
}
async fn remove(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Local>,
    auth: Option<Extension<VerifiedControlPlaneAuth>>,
    body: Result<Json<Remove>, JsonRejection>,
) -> CredentialResult<Json<Value>> {
    gate(&s, q.host.as_deref(), &auth, true)?;
    let Json(body) =
        body.map_err(|_| ApiError::bad_request("invalid MCP credential removal request"))?;
    blocking(move || {
        let result =
            store::with_snapshot_locked(&s.global_dir, &body.registry_revision, |snapshot| {
                let owner = binding(snapshot, &id)?;
                if owner.generation != body.binding_generation {
                    return Err(ApiError::conflict("MCP credential binding changed"));
                }
                Ok(Store::for_global(&s.global_dir).mutate(
                    &owner,
                    &body.credential_revision,
                    &body.operation_id,
                    None,
                ))
            })?;
        result.map(receipt).map_err(CredentialFailure::Store)
    })
    .await
}
async fn operation(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Local>,
    auth: Option<Extension<VerifiedControlPlaneAuth>>,
) -> CredentialResult<Json<Value>> {
    gate(&s, q.host.as_deref(), &auth, false)?;
    blocking(move || {
        Store::for_global(&s.global_dir)
            .receipt(&id)
            .map(receipt)
            .map_err(CredentialFailure::Store)
    })
    .await
}
async fn orphans(
    State(s): State<AppState>,
    Query(q): Query<Page>,
    auth: Option<Extension<VerifiedControlPlaneAuth>>,
) -> CredentialResult<Json<Value>> {
    gate(&s, q.host.as_deref(), &auth, false)?;
    let limit = q.limit.unwrap_or(50);
    let offset = q.offset.unwrap_or(0);
    if !(1..=100).contains(&limit) || offset > usize::MAX - limit {
        return Err(ApiError::bad_request("invalid MCP credential page").into());
    }
    blocking(move || {
        let snapshot = store::read(&s.global_dir)?;
        let rows: Vec<_> = Store::for_global(&s.global_dir).statuses().map_err(CredentialFailure::Store)?.into_iter().filter(|row| row.present && !referenced(&snapshot, &row.binding)).collect();
        let next = (offset.saturating_add(limit) < rows.len()).then_some(offset + limit);
        Ok(Json(json!({"version":1,"host_id":"local","scope":"trusted_host","registry_revision":snapshot.revision,"offset":offset,"limit":limit,"next_offset":next,"orphans":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>()})))
    }).await
}
async fn remove_orphan(
    State(s): State<AppState>,
    Path(generation): Path<String>,
    Query(q): Query<Local>,
    auth: Option<Extension<VerifiedControlPlaneAuth>>,
    body: Result<Json<OrphanRemove>, JsonRejection>,
) -> CredentialResult<Json<Value>> {
    gate(&s, q.host.as_deref(), &auth, true)?;
    let Json(body) =
        body.map_err(|_| ApiError::bad_request("invalid orphan credential removal request"))?;
    if body.binding.generation != generation {
        return Err(ApiError::bad_request(
            "credential binding does not match the requested generation",
        )
        .into());
    }
    blocking(move || {
        let result = store::with_snapshot_locked(&s.global_dir, &body.registry_revision, |snapshot| {
        if referenced(snapshot, &body.binding) { return Err(ApiError::conflict("credential still belongs to a configured server; use its credential removal action")); }
        let store = Store::for_global(&s.global_dir);
        let current = store.status(&body.binding).map_err(map_error)?;
        if !current.present { return Err(ApiError::not_found("retained orphan credential not found")); }
        Ok(store.mutate(&body.binding, &body.credential_revision, &body.operation_id, None))
        })?;
        result.map(receipt).map_err(CredentialFailure::Store)
    }).await
}
#[cfg(test)]
mod tests;
