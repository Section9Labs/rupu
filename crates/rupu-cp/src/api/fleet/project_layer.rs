//! Read-only, owner-qualified reasons for project fleet unavailability.
//! `missing` is positive evidence at observation time, not an init reservation.
use crate::{error::ApiResult, state::AppState};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::path::{Path, PathBuf};

pub(super) struct Refusal {
    code: &'static str,
    workspace_id: String,
    project_path: String,
    project_root: PathBuf,
}
impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let message = match self.code {
            "project_layer_missing" => "The registered project has no project-local .rupu directory.",
            "project_layer_global_alias" => "The registered project's .rupu directory is the global root; project-local setup is unavailable.",
            _ => "The registered project's local layer could not be distinguished safely from the global root.",
        };
        (StatusCode::CONFLICT, Json(serde_json::json!({
            "error": message, "code": self.code, "schema_version": 1,
            "scope": {"host_id": "local", "workspace_id": self.workspace_id, "project_path": self.project_path},
            "project_root": self.project_root,
            "consistency": "observed_not_locked",
        }))).into_response()
    }
}

pub(super) fn inspect(s: &AppState, workspace_id: &str) -> ApiResult<Result<PathBuf, Refusal>> {
    let workspace = crate::api::code::load_workspace(s, workspace_id)?;
    let project = Path::new(&workspace.path);
    let layer = project.join(".rupu");
    let refuse = |code| {
        Err(Refusal {
            code,
            workspace_id: workspace_id.to_owned(),
            project_path: workspace.path.clone(),
            project_root: layer.clone(),
        })
    };
    // Canonicalize both existing anchors. A lexical spelling difference is not
    // proof of distinct ownership (e.g. a registered symlink to the home dir).
    let (project_anchor, global_anchor) =
        match (project.canonicalize(), s.global_dir.canonicalize()) {
            (Ok(project_anchor), Ok(global_anchor))
                if project_anchor.is_dir() && global_anchor.is_dir() =>
            {
                (project_anchor, global_anchor)
            }
            _ => return Ok(refuse("project_layer_unavailable")),
        };
    match std::fs::symlink_metadata(&layer) {
        Ok(_) => match layer.canonicalize() {
            Ok(actual) if actual == global_anchor => Ok(refuse("project_layer_global_alias")),
            Ok(actual) if actual.is_dir() => Ok(Ok(layer)),
            _ => Ok(refuse("project_layer_unavailable")),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // symlink_metadata distinguishes a dangling .rupu symlink from a
            // genuinely absent final component; only the latter is "missing".
            if project_anchor.join(".rupu") == global_anchor {
                Ok(refuse("project_layer_global_alias"))
            } else {
                Ok(refuse("project_layer_missing"))
            }
        }
        Err(_) => Ok(refuse("project_layer_unavailable")),
    }
}
