use crate::{error::ApiResult, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rupu_workspace::WorkspaceStore;

#[derive(serde::Serialize)]
pub struct ProjectRow {
    pub ws_id: String,
    pub name: String,
    pub path: String,
    pub repo_remote: Option<String>,
    pub branch: Option<String>,
    pub created_at: String,
    pub last_run_at: Option<String>,
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/projects", get(list_projects))
}

async fn list_projects(State(s): State<AppState>) -> ApiResult<Json<Vec<ProjectRow>>> {
    let mut rows: Vec<ProjectRow> = store(&s)
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|w| ProjectRow {
            name: std::path::Path::new(&w.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| w.path.clone()),
            ws_id: w.id,
            path: w.path,
            repo_remote: w.repo_remote,
            branch: w.initial_branch,
            created_at: w.created_at,
            last_run_at: w.last_run_at,
        })
        .collect();
    // Newest activity first; `None` sorts last (None < Some(_) in Rust's
    // default Ord, so reversing puts Some(_) before None).
    rows.sort_by(|a, b| b.last_run_at.cmp(&a.last_run_at));
    Ok(Json(rows))
}
