//! Exact run-start workflow source for operator inspection. No current-definition
//! resolution, host probing, restoration, or execution is performed.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::runs::RunRecord;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path as FsPath};

const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_RECORD_BYTES: usize = 256 * 1024;

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/workflow-source", get(source))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceQuery {
    host: Option<String>,
    ws_id: String,
}

async fn source(
    State(s): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<SourceQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Json<serde_json::Value>> {
    let Query(q) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    super::fleet::require_local(q.host.as_deref(), "recorded workflow source")?;
    super::runs::validate_id(&id)?;
    super::runs::validate_id(&q.ws_id)?;
    super::runs::blocking(move || read_source(&s, &id, &q.ws_id).map(Json)).await
}

fn read_bounded(path: &FsPath, max: usize) -> ApiResult<Vec<u8>> {
    let file = super::fs_open::open_anchored_regular_file(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found("recorded workflow artifact unavailable")
        } else {
            ApiError::conflict("recorded workflow artifact is not a safe regular file")
        }
    })?;
    let mut bytes = Vec::new();
    file.take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ApiError::internal("unable to read recorded workflow artifact"))?;
    if bytes.len() > max {
        return Err(ApiError(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "recorded workflow artifact exceeds read limit".into(),
        ));
    }
    Ok(bytes)
}

fn read_source(s: &AppState, id: &str, workspace: &str) -> ApiResult<serde_json::Value> {
    // Same active/archive precedence as local_recorded_store and project history.
    // Descriptor-relative reads also bound run.json and refuse symlink components.
    for (root, archived) in [
        (s.run_store.root.clone(), false),
        (s.run_store.archive_root(), true),
    ] {
        let root = match root.canonicalize() {
            Ok(root) => root,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(ApiError::internal("unable to resolve recorded run store")),
        };
        let directory = root.join(id);
        let record_bytes = match read_bounded(&directory.join("run.json"), MAX_RECORD_BYTES) {
            Ok(bytes) => bytes,
            Err(e) if e.0 == axum::http::StatusCode::NOT_FOUND => continue,
            Err(e) => return Err(e),
        };
        let run: RunRecord = serde_json::from_slice(&record_bytes)
            .map_err(|_| ApiError::conflict("recorded workflow identity is malformed"))?;
        if run.id != id || run.workspace_id != workspace {
            return Err(ApiError::not_found(
                "workflow run not found in this project",
            ));
        }
        if run.workflow_name.is_empty() || run.workflow_name.starts_with("agent:") {
            return Err(ApiError::bad_request("run is not a recorded workflow"));
        }
        let bytes = read_bounded(&directory.join("workflow.yaml"), MAX_SOURCE_BYTES)?;
        let yaml = std::str::from_utf8(&bytes)
            .map_err(|_| ApiError::conflict("recorded source is not UTF-8"))?;
        if yaml.trim().is_empty() {
            return Err(ApiError::not_found("recorded workflow source unavailable"));
        }
        // An archive/delete/replace race must not publish source under stale owners.
        let after = read_bounded(&directory.join("run.json"), MAX_RECORD_BYTES)?;
        let after: RunRecord = serde_json::from_slice(&after)
            .map_err(|_| ApiError::conflict("recorded workflow identity changed"))?;
        if after.id != run.id
            || after.workspace_id != run.workspace_id
            || after.workflow_name != run.workflow_name
            || after.started_at != run.started_at
        {
            return Err(ApiError::conflict("recorded workflow identity changed"));
        }
        return Ok(
            serde_json::json!({"run_id":run.id,"workspace_id":run.workspace_id,"host_id":"local",
            "workflow_name":run.workflow_name,"started_at":run.started_at,"archived":archived,
            "source_kind":"run_snapshot","snapshot_revision":format!("sha256:{:x}", Sha256::digest(&bytes)),"yaml":yaml}),
        );
    }
    Err(ApiError::not_found(
        "recorded workflow run unavailable on local host",
    ))
}

#[cfg(test)]
#[path = "recorded_workflow_tests.rs"]
mod tests;
