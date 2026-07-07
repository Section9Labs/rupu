use crate::{
    api::fs_safety,
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    launcher::LaunchError,
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use rupu_orchestrator::Workflow;
use rupu_workspace::WorkspaceStore;
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/workflows", get(list_workflows).post(create_workflow))
        .route(
            "/api/workflows/:name",
            get(get_workflow)
                .put(write_workflow)
                .delete(delete_workflow),
        )
        .route("/api/workflows/:name/run", post(launch_run))
        // axum matches static literal segments before dynamic `:name` captures,
        // so this route is reachable and is NOT shadowed by `/api/workflows/:name`.
        .route("/api/workflows/validate", post(validate_workflow))
        .route("/api/workflows/generate", post(generate_workflow))
        .route("/api/generate/models", get(generate_models))
}

/// Directory where global workflow `.yaml` definitions live.
fn workflows_dir(s: &AppState) -> std::path::PathBuf {
    s.global_dir.join("workflows")
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

/// Scope tag for a registered project: the workspace path's basename,
/// falling back to the workspace id if the path has no basename (e.g. `/`).
fn project_scope_name(w: &rupu_workspace::Workspace) -> String {
    std::path::Path::new(&w.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| w.id.clone())
}

/// Resolve `name` to a workflow YAML path: the global layer first, then
/// (if absent there) each registered project's `<path>/.rupu/workflows/`, in
/// `store().list()` order. First match wins; a later task may thread the
/// resolved scope through to the caller for a disambiguating URL.
fn resolve_workflow_path(s: &AppState, name: &str) -> Option<std::path::PathBuf> {
    let global = workflows_dir(s).join(format!("{name}.yaml"));
    if global.exists() {
        return Some(global);
    }
    for w in store(s).list().unwrap_or_default() {
        let candidate = std::path::Path::new(&w.path)
            .join(".rupu")
            .join("workflows")
            .join(format!("{name}.yaml"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
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
pub(crate) fn scan_workflow_names(
    dir: &std::path::Path,
    scope: impl Into<String>,
) -> Vec<WorkflowDto> {
    let scope = scope.into();
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
            scope: scope.clone(),
            usage: crate::usage::UsageSummary::default(),
            run_count: 0,
            last_run: None,
        })
        .collect()
}

/// `GET /api/workflows` — global workflow definitions plus every registered
/// project's `<path>/.rupu/workflows/*.yaml`, sorted by name then scope.
///
/// Each row is tagged `scope: "global"` or the owning project's name. A
/// project def shadows a same-named GLOBAL row; two different projects
/// defining the same name both appear (distinguished by `scope`). With no
/// registered projects this is byte-for-byte the prior global-only behavior.
async fn list_workflows(State(s): State<AppState>) -> ApiResult<Json<Vec<WorkflowDto>>> {
    let mut rows = scan_workflow_names(&s.global_dir.join("workflows"), "global");

    let mut project_rows: Vec<WorkflowDto> = Vec::new();
    for w in store(&s).list().unwrap_or_default() {
        let scope = project_scope_name(&w);
        let dir = std::path::Path::new(&w.path)
            .join(".rupu")
            .join("workflows");
        project_rows.extend(scan_workflow_names(&dir, scope));
    }

    let project_names: std::collections::BTreeSet<&str> =
        project_rows.iter().map(|r| r.name.as_str()).collect();
    rows.retain(|r| !project_names.contains(r.name.as_str()));
    rows.extend(project_rows);
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.scope.cmp(&b.scope)));

    let runs = s.run_store.list().unwrap_or_default();
    let rollups = crate::usage::rollup_by(&s.run_store, &runs, &s.pricing, |r| {
        Some(r.workflow_name.clone())
    });
    for row in &mut rows {
        if let Some(roll) = rollups.get(&row.name) {
            row.usage = roll.usage.clone();
            row.run_count = roll.run_count;
            row.last_run = roll.last_active.clone();
        }
    }
    Ok(Json(rows))
}

/// Load workflow `name` and build the full detail DTO (`workflow` + raw `yaml` +
/// aggregate `usage`). Shared by GET / PUT / POST.
///
/// Project-aware: resolves `name` in the global layer first, falling back to
/// every registered project's `.rupu/workflows/` (first match) so a
/// project-only workflow's detail route doesn't 404.
fn load_detail(s: &AppState, name: &str) -> ApiResult<Json<serde_json::Value>> {
    let path = resolve_workflow_path(s, name)
        .ok_or_else(|| ApiError::not_found(format!("workflow {name} not found")))?;
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

async fn get_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    load_detail(&s, &name)
}

/// Request body for `PUT /api/workflows/:name` and `POST /api/workflows`: the
/// full raw `.yaml` to validate and persist.
#[derive(Deserialize)]
struct WorkflowWriteBody {
    raw: String,
}

/// `PUT /api/workflows/:name` — overwrite (or create) the global workflow
/// definition `:name`. The raw `.yaml` is validated by [`Workflow::parse`]
/// before any write; the parsed `name:` must equal `:name`. Returns the
/// reloaded detail DTO.
async fn write_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<WorkflowWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    fs_safety::validate_name(&name)?;
    let wf = Workflow::parse(&body.raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    if wf.name != name {
        return Err(ApiError::bad_request(
            "workflow name must equal the workflow file name",
        ));
    }
    let dir = workflows_dir(&s);
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    fs_safety::write_atomic(&dir.join(format!("{name}.yaml")), body.raw.as_bytes())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    load_detail(&s, &name)
}

/// `POST /api/workflows` — create a new global workflow. The name is taken from
/// the parsed `name:`; fails with 409 if a definition with that name already
/// exists. Returns the reloaded detail DTO.
async fn create_workflow(
    State(s): State<AppState>,
    Json(body): Json<WorkflowWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let wf = Workflow::parse(&body.raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let name = wf.name;
    fs_safety::validate_name(&name)?;
    let dir = workflows_dir(&s);
    let target = dir.join(format!("{name}.yaml"));
    if target.exists() {
        return Err(ApiError::conflict("workflow already exists"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    fs_safety::write_atomic(&target, body.raw.as_bytes())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    load_detail(&s, &name)
}

/// `POST /api/workflows/validate` — stateless parse-check of a raw workflow
/// `.yaml`. Takes no [`State`] and touches no filesystem: it only runs
/// [`Workflow::parse`] and reports `{ "ok": true }` on success or a 400 with the
/// parse error message on failure. Backs the editor's live valid/invalid badge.
async fn validate_workflow(
    Json(body): Json<WorkflowWriteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    Workflow::parse(&body.raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `DELETE /api/workflows/:name` — remove the global workflow definition `:name`.
async fn delete_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    fs_safety::validate_name(&name)?;
    let target = workflows_dir(&s).join(format!("{name}.yaml"));
    if !target.exists() {
        return Err(ApiError::not_found(format!("workflow {name} not found")));
    }
    std::fs::remove_file(&target).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "deleted": true })))
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
    /// Optional host id. Absent or `"local"` → local path (including the
    /// existing 501 when no launcher is installed). A remote id proxies via
    /// [`HostConnector::launch_run`] and returns `{ "run_id", "host_id" }`.
    #[serde(default)]
    host: Option<String>,
}

/// Start a fresh run of `:name` via the configured [`RunLauncher`] (local) or
/// by proxying to a remote host. Returns the new run id plus the owning
/// `host_id`. 501 when no launcher is installed and the target is local.
///
/// [`RunLauncher`]: crate::launcher::RunLauncher
async fn launch_run(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<LaunchBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let b = body.map(|j| j.0).unwrap_or_default();
    let host = b.host.as_deref().unwrap_or("local").to_string();

    if host != "local" {
        // Remote path: resolve the connector and proxy the launch.
        let conn = crate::api::runs::resolve_host(&s, &host)?;
        let req = crate::launcher::LaunchRequest {
            workflow: name,
            inputs: b.inputs,
            mode: b.mode,
            target: b.target,
            working_dir: b.working_dir,
        };
        let run_id = conn.launch_run(req).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(
            serde_json::json!({ "run_id": run_id, "host_id": host }),
        ));
    }

    // Local path: unchanged (including the 501 when no launcher is installed).
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
        Ok(run_id) => Ok(Json(
            serde_json::json!({ "run_id": run_id, "host_id": "local" }),
        )),
        Err(LaunchError::Invalid(m)) => Err(ApiError::bad_request(m)),
        Err(LaunchError::Spawn(m)) => Err(ApiError::internal(m)),
    }
}

#[derive(serde::Deserialize)]
struct GenerateWorkflowBody {
    description: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct GeneratedWfDto {
    raw: String,
    provider: String,
    model: String,
    attempts: u8,
}

#[derive(serde::Serialize)]
struct ProviderModelsDto {
    provider: String,
    models: Vec<String>,
    is_default: bool,
}

async fn generate_workflow(
    State(s): State<AppState>,
    Json(body): Json<GenerateWorkflowBody>,
) -> ApiResult<Json<GeneratedWfDto>> {
    use crate::definition_generator::{DefKind, GenDefError, GenerateDefRequest};
    let gen = s
        .generator
        .clone()
        .ok_or_else(|| ApiError::not_available("AI generation requires `rupu cp serve`"))?;
    let out = gen
        .generate(GenerateDefRequest {
            kind: DefKind::Workflow,
            description: body.description,
            provider: body.provider,
            model: body.model,
        })
        .await
        .map_err(|e| match e {
            GenDefError::NoCredentials => ApiError::bad_request(e.to_string()),
            GenDefError::Failed(m) => ApiError::internal(m),
        })?;
    Ok(Json(GeneratedWfDto {
        raw: out.raw,
        provider: out.provider,
        model: out.model,
        attempts: out.attempts,
    }))
}

async fn generate_models(State(s): State<AppState>) -> Json<Vec<ProviderModelsDto>> {
    let list = match &s.generator {
        Some(g) => g
            .available_models()
            .await
            .into_iter()
            .map(|p| ProviderModelsDto {
                provider: p.provider,
                models: p.models,
                is_default: p.is_default,
            })
            .collect(),
        None => Vec::new(),
    };
    Json(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::{LaunchError, LaunchRequest, RunLauncher};
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
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf())
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
            host: None,
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
            host: None,
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

    const VALID_YAML: &str = "name: demo\nsteps:\n  - id: one\n    agent: x\n    prompt: hi\n";

    fn wf_path(s: &AppState, name: &str) -> std::path::PathBuf {
        workflows_dir(s).join(format!("{name}.yaml"))
    }

    #[tokio::test]
    async fn put_valid_writes_and_reloads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let resp = write_workflow(
            State(s.clone()),
            Path("demo".into()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect("put ok");
        assert_eq!(resp.0["yaml"], serde_json::json!(VALID_YAML));
        assert_eq!(
            std::fs::read_to_string(wf_path(&s, "demo")).unwrap(),
            VALID_YAML
        );

        // Re-reading via get_workflow returns the new yaml.
        let got = get_workflow(State(s.clone()), Path("demo".into()))
            .await
            .expect("get ok");
        assert_eq!(got.0["yaml"], serde_json::json!(VALID_YAML));
    }

    #[tokio::test]
    async fn put_unparseable_is_bad_request_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let err = write_workflow(
            State(s.clone()),
            Path("demo".into()),
            Json(WorkflowWriteBody { raw: "".into() }),
        )
        .await
        .expect_err("should reject");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(!wf_path(&s, "demo").exists());
    }

    #[tokio::test]
    async fn put_name_mismatch_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let err = write_workflow(
            State(s.clone()),
            Path("other".into()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect_err("mismatch");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(!wf_path(&s, "other").exists());
    }

    #[tokio::test]
    async fn post_creates_then_conflicts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let resp = create_workflow(
            State(s.clone()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect("create ok");
        assert_eq!(resp.0["yaml"], serde_json::json!(VALID_YAML));
        assert_eq!(
            std::fs::read_to_string(wf_path(&s, "demo")).unwrap(),
            VALID_YAML
        );

        let err = create_workflow(
            State(s.clone()),
            Json(WorkflowWriteBody {
                raw: VALID_YAML.into(),
            }),
        )
        .await
        .expect_err("conflict");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    // `validate_workflow` is stateless: it takes no `State` and touches no
    // filesystem — it only parse-checks the raw YAML.
    #[tokio::test]
    async fn validate_valid_yaml_is_ok() {
        let resp = validate_workflow(Json(WorkflowWriteBody {
            raw: VALID_YAML.into(),
        }))
        .await
        .expect("valid yaml should validate");
        assert_eq!(resp.0["ok"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn validate_unparseable_is_bad_request() {
        // An empty/invalid workflow (no steps) fails `Workflow::parse`.
        let err = validate_workflow(Json(WorkflowWriteBody {
            raw: "steps: []".into(),
        }))
        .await
        .expect_err("invalid workflow should reject");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_present_then_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(wf_path(&s, "demo"), VALID_YAML).unwrap();

        let resp = delete_workflow(State(s.clone()), Path("demo".into()))
            .await
            .expect("delete ok");
        assert_eq!(resp.0["deleted"], serde_json::json!(true));
        assert!(!wf_path(&s, "demo").exists());

        let err = delete_workflow(State(s.clone()), Path("demo".into()))
            .await
            .expect_err("absent");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    /// Register a workspace record `<global_dir>/workspaces/<id>.toml` whose
    /// `path` points at `project_root`.
    fn register_workspace(tmp: &tempfile::TempDir, id: &str, project_root: &std::path::Path) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                project_root.display()
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn list_no_projects_is_global_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap();
        std::fs::write(wf_path(&s, "demo"), VALID_YAML).unwrap();

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "demo");
        assert_eq!(rows[0].scope, "global");
    }

    #[tokio::test]
    async fn list_includes_project_defs_tagged_with_project_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("proj-only.yaml"), VALID_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let Json(rows) = list_workflows(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        let expected_scope = proj
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(rows[0].name, "proj-only");
        assert_eq!(rows[0].scope, expected_scope);
    }

    #[tokio::test]
    async fn workflow_detail_resolves_project_def() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        std::fs::create_dir_all(workflows_dir(&s)).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("demo.yaml"), VALID_YAML).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // Absent from global, present only in the project — must resolve, not 404.
        let resp = get_workflow(State(s), Path("demo".into()))
            .await
            .expect("project-only workflow should resolve via detail");
        assert_eq!(resp.0["yaml"], serde_json::json!(VALID_YAML));
    }
}
