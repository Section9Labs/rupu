//! Explicit archive policy and exact reviewed network-ledger retention.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Instant};
mod disk;
mod journal;
mod netflow;
mod plan;
#[cfg(test)]
mod tests;

pub const NETFLOW_FEATURE: &str = "maintenance.retention.netflow_exact.v1";
pub const FEATURE: &str = "maintenance.retention.policy.v1";
pub const KINDS: &[&str] = &["sessions", "transcripts", "cleanup"];
const MAX_GROUPS: usize = 256;
const MAX_ARTIFACTS: usize = 4096;
const MAX_BYTES: usize = 8 * 1024 * 1024;
fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(e.to_string())
}
fn large(message: &str) -> ApiError {
    ApiError(axum::http::StatusCode::PAYLOAD_TOO_LARGE, message.into())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_id(id: &str) -> ApiResult<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ApiError::bad_request("invalid retention identifier"));
    }
    Ok(())
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/maintenance/retention/preview", post(preview))
        .route(
            "/api/maintenance/retention/operations/:id/execute",
            post(execute),
        )
        .route("/api/maintenance/retention/operations/:id", get(operation))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scopes {
    pub global: bool,
    pub project_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewBody {
    pub kind: String,
    pub scopes: Scopes,
    pub older_than: Option<String>,
    #[serde(default)]
    pub authorization_mode: Option<String>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteBody {
    pub preview_id: String,
    pub policy_digest: String,
    pub authorization_mode: String,
    #[serde(default)]
    pub preview_set_digest: Option<String>,
}
#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadQuery {
    pub host: Option<String>,
    pub section: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Root {
    id: String,
    path: PathBuf,
    identity: disk::Identity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Policy {
    owner: Root,
    roots: Vec<Root>,
    kind: String,
    scopes: Scopes,
    project_registry: Option<(PathBuf, String)>,
    sessions_before: String,
    transcripts_before: String,
    observed_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    authorization_mode: String,
    auxiliary_artifacts: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Artifact {
    root: usize,
    relative: PathBuf,
    stamp: disk::Stamp,
    directory: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Check {
    path: PathBuf,
    digest: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Group {
    kind: String,
    id: String,
    artifacts: Vec<Artifact>,
    checks: Vec<Check>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    kind: String,
    id: String,
    reason: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Plan {
    groups: Vec<Group>,
    kept: Vec<Kept>,
    entries_scanned: usize,
    metadata_bytes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Prepared {
    schema_version: u32,
    operation_id: String,
    preview_id: String,
    policy_digest: String,
    policy: Policy,
    preview: Plan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preview_set_digest: Option<String>,
}

fn lease(global: &std::path::Path) -> ApiResult<rupu_runtime::retention_lease::RetentionLease> {
    rupu_runtime::retention_lease::RetentionLease::try_acquire(global).map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            ApiError(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "retention lifecycle is busy".into(),
            )
        } else {
            internal(e)
        }
    })
}
async fn preview(
    State(s): State<AppState>,
    Query(q): Query<crate::admin_ops::HostQuery>,
    Json(body): Json<PreviewBody>,
) -> ApiResult<Json<Value>> {
    crate::admin_ops::local_only(q.host.as_deref(), "retention policy preview")?;
    super::config::require_writable_to(&s, "durable retention policy preview")?;
    super::runs::blocking(move || prepare(&s, body).map(Json)).await
}
fn prepare(s: &AppState, body: PreviewBody) -> ApiResult<Value> {
    if !(KINDS.contains(&body.kind.as_str()) && body.authorization_mode.is_none()
        || body.kind == "netflow"
            && body.authorization_mode.as_deref() == Some("exact_preview_set"))
    {
        return Err(ApiError::bad_request("archive policy kinds are sessions, transcripts and cleanup; netflow requires explicit exact_preview_set authorization"));
    }
    if !body.scopes.global
        && (body.scopes.project_id.is_none()
            || !["transcripts", "netflow"].contains(&body.kind.as_str()))
    {
        return Err(ApiError::bad_request(
            "explicit global scope is required for session cleanup",
        ));
    }
    if body.older_than.as_ref().is_some_and(|v| v.len() > 64) {
        return Err(ApiError::bad_request("retention age exceeds 64 bytes"));
    }
    let global = s.global_dir.canonicalize().map_err(internal)?;
    let _lease = lease(&global)?;
    let policy = plan::policy(s, body)?;
    if policy.owner.path != global {
        return Err(ApiError::conflict("retention configured root changed"));
    }
    let preview = plan::select(s, &policy)?;
    let id = format!("ret_{}", ulid::Ulid::new());
    let preview_set_digest = if policy.kind == "netflow" {
        Some(digest(&serde_json::to_vec(&preview).map_err(internal)?))
    } else {
        None
    };
    let prepared = Prepared {
        schema_version: 1,
        operation_id: id.clone(),
        preview_id: id,
        policy_digest: digest(&serde_json::to_vec(&policy).map_err(internal)?),
        policy,
        preview_set_digest,
        preview,
    };
    journal::create(&prepared)?;
    journal::answer(&prepared, ReadQuery::default(), false)
}
async fn execute(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<crate::admin_ops::HostQuery>,
    Json(body): Json<ExecuteBody>,
) -> ApiResult<Json<Value>> {
    crate::admin_ops::local_only(q.host.as_deref(), "retention execution")?;
    super::config::require_writable_to(&s, "retention execution")?;
    // spawn_blocking owns the admitted task even if the transport disconnects.
    super::runs::blocking(move || execute_once(&s, &id, body, &mut journal::NoFault).map(Json))
        .await
}
fn execute_once(
    s: &AppState,
    id: &str,
    body: ExecuteBody,
    fault: &mut impl journal::Fault,
) -> ApiResult<Value> {
    let global = s.global_dir.canonicalize().map_err(internal)?;
    let _lease = lease(&global)?;
    let prepared = journal::load(&global, id)?;
    if body.preview_id != prepared.preview_id
        || body.policy_digest != prepared.policy_digest
        || body.authorization_mode != prepared.policy.authorization_mode
        || body.preview_set_digest != prepared.preview_set_digest
    {
        return Err(ApiError::conflict("retention operation intent mismatch"));
    }
    if !journal::events(&prepared)?.is_empty() {
        return journal::answer(&prepared, ReadQuery::default(), true);
    }
    if Utc::now() > prepared.policy.expires_at {
        return Err(ApiError::conflict(
            "retention preview expired; create a new policy preview",
        ));
    }
    let mut writer = journal::Writer::new(&prepared, fault)?;
    writer.append(json!({"phase":"accepted"}))?;
    let selected = if prepared.policy.kind == "netflow" {
        plan::verify_policy(&prepared.policy).map(|()| prepared.preview.clone())
    } else {
        plan::select(s, &prepared.policy)
    };
    match selected {
        Ok(selected) => {
            journal::save_plan(&prepared, &selected)?;
            writer.append(json!({"phase":"executing","execution_groups":selected.groups.len(),"kept":selected.kept}))?;
            journal::apply(s, &prepared, &selected, &mut writer)?;
        }
        Err(e) => {
            writer.append(json!({"phase":"refused","error":e.1,"zero_effects":true}))?;
        }
    }
    journal::answer(&prepared, ReadQuery::default(), true)
}
async fn operation(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ReadQuery>,
) -> ApiResult<Json<Value>> {
    crate::admin_ops::local_only(q.host.as_deref(), "retention receipts")?;
    super::runs::blocking(move || {
        let prepared = journal::load(&s.global_dir, &id)?;
        let guard = lease(&s.global_dir).ok();
        journal::answer(&prepared, q, guard.is_some()).map(Json)
    })
    .await
}
