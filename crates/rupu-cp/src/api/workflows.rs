use crate::{
    error::{ApiError, ApiResult},
    launcher::LaunchError,
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use rupu_orchestrator::Workflow;
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/workflows", get(list_workflows))
        .route("/api/workflows/:name", get(get_workflow))
        .route("/api/workflows/:name/run", post(launch_run))
}

#[derive(Serialize)]
pub(crate) struct WorkflowDto {
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) usage: crate::usage::UsageSummary,
    pub(crate) run_count: u64,
    pub(crate) last_run: Option<String>,
}

/// Scan `<dir>/*.yaml` and return one [`WorkflowDto`] per file stem, tagged
/// with `scope`, sorted by name. A missing/unreadable directory yields an
/// empty vec (tolerated, not an error) so the caller can merge layers freely.
pub(crate) fn scan_workflow_names(dir: &std::path::Path, scope: &'static str) -> Vec<WorkflowDto> {
    if !dir.is_dir() {
        return vec![];
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("workflows: could not read {}: {err}", dir.display());
            return vec![];
        }
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("yaml"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| WorkflowDto {
            name,
            scope: scope.to_string(),
            usage: crate::usage::UsageSummary::default(),
            run_count: 0,
            last_run: None,
        })
        .collect()
}

async fn list_workflows(State(s): State<AppState>) -> ApiResult<Json<Vec<WorkflowDto>>> {
    let mut rows = scan_workflow_names(&s.global_dir.join("workflows"), "global");
    let runs = s.run_store.list().unwrap_or_default();
    let rollups =
        crate::usage::rollup_by(&s.run_store, &runs, &s.pricing, |r| Some(r.workflow_name.clone()));
    for row in &mut rows {
        if let Some(roll) = rollups.get(&row.name) {
            row.usage = roll.usage.clone();
            row.run_count = roll.run_count;
            row.last_run = roll.last_active.clone();
        }
    }
    Ok(Json(rows))
}

async fn get_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = s.global_dir.join("workflows").join(format!("{name}.yaml"));
    if !path.exists() {
        return Err(ApiError::not_found(format!("workflow {name} not found")));
    }
    let yaml = std::fs::read_to_string(&path).map_err(|e| ApiError::internal(e.to_string()))?;
    let workflow = Workflow::parse(&yaml).map_err(|e| ApiError::internal(e.to_string()))?;

    let runs = s.run_store.list().unwrap_or_default();
    let usage = crate::usage::rollup(
        runs.iter()
            .filter(|r| r.workflow_name == name)
            .map(|r| crate::usage::summarize_run(&s.run_store, &r.id, &s.pricing)),
    );

    Ok(Json(
        serde_json::json!({ "workflow": workflow, "yaml": yaml, "usage": usage }),
    ))
}

/// Request body for `POST /api/workflows/:name/run`. All fields optional; a
/// bodyless POST launches the workflow with no inputs in its default mode.
#[derive(Deserialize, Default)]
struct LaunchBody {
    #[serde(default)]
    inputs: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
}

/// Start a fresh run of `:name` via the configured [`RunLauncher`]. Returns the
/// new run id. 501 when no launcher is installed (read-only deploy).
///
/// [`RunLauncher`]: crate::launcher::RunLauncher
async fn launch_run(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<LaunchBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let b = body.map(|j| j.0).unwrap_or_default();
    let launcher = s
        .launcher
        .as_ref()
        .ok_or_else(|| ApiError::not_available("launching runs requires `rupu cp serve`"))?;
    let req = crate::launcher::LaunchRequest {
        workflow: name,
        inputs: b.inputs,
        mode: b.mode,
        target: b.target,
        working_dir: b.working_dir,
    };
    match launcher.launch(req).await {
        Ok(run_id) => Ok(Json(serde_json::json!({ "run_id": run_id }))),
        Err(LaunchError::Invalid(m)) => Err(ApiError::bad_request(m)),
        Err(LaunchError::Spawn(m)) => Err(ApiError::internal(m)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::{LaunchError, LaunchRequest, RunLauncher};
    use rupu_orchestrator::RunStore;
    use std::sync::{Arc, Mutex};

    /// Captures the last `LaunchRequest` and returns a canned run id.
    struct MockLauncher {
        last: Mutex<Option<LaunchRequest>>,
        run_id: String,
    }

    #[async_trait::async_trait]
    impl RunLauncher for MockLauncher {
        async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError> {
            *self.last.lock().unwrap() = Some(req);
            Ok(self.run_id.clone())
        }
    }

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        let store = RunStore::new(tmp.path().join("runs"));
        AppState {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: tmp.path().to_path_buf(),
            run_store: Arc::new(store),
            pricing: rupu_config::PricingConfig::default(),
            launcher: None,
            session_sender: None,
            repos: None,
            agent_launcher: None,
        }
    }

    #[tokio::test]
    async fn launch_run_invokes_launcher_and_returns_run_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "run_xyz".into(),
        });
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));

        let mut inputs = std::collections::BTreeMap::new();
        inputs.insert("repo".to_string(), "acme/widgets".to_string());
        let body = LaunchBody {
            inputs: inputs.clone(),
            mode: Some("bypass".into()),
            target: None,
            working_dir: None,
        };

        let resp = launch_run(State(s), Path("nightly".into()), Some(Json(body)))
            .await
            .expect("launch should succeed");
        assert_eq!(resp.0["run_id"], "run_xyz");

        let captured = mock.last.lock().unwrap().clone().expect("request captured");
        assert_eq!(captured.workflow, "nightly");
        assert_eq!(captured.inputs, inputs);
        assert_eq!(captured.mode.as_deref(), Some("bypass"));
        assert_eq!(captured.target, None);
    }

    #[tokio::test]
    async fn launch_forwards_working_dir() {
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "run_X".into(),
        });
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));
        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: Some("/tmp/projX".into()),
        };
        let _ = launch_run(State(s), Path("nightly".into()), Some(Json(body))).await;
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/projX"));
    }

    #[tokio::test]
    async fn launch_run_without_launcher_is_not_implemented() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // launcher: None

        let err = launch_run(State(s), Path("nightly".into()), None)
            .await
            .expect_err("no launcher should error");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
}
