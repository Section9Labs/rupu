use crate::{
    agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher},
    api::fs_safety::{validate_name, write_atomic},
    api::repo_scope::distinct_repo_workspaces,
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    session_starter::{SessionStartError, SessionStartRequest, SessionStarter},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use rupu_agent::loader::{load_agent, load_agents, AgentLoadError};
use rupu_workspace::{RepoRegistryStore, WorkspaceStore};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agents", get(list_agents).post(create_agent))
        .route(
            "/api/agents/:name",
            get(get_agent).put(write_agent).delete(delete_agent),
        )
        .route("/api/agents/:name/run", post(run_agent))
        .route("/api/agents/:name/session", post(start_session))
        .route("/api/agents/generate", post(generate_agent))
}

/// Directory where global agent `.md` definitions live.
fn agents_dir(s: &AppState) -> PathBuf {
    s.global_dir.join("agents")
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

fn repo_store(s: &AppState) -> RepoRegistryStore {
    RepoRegistryStore {
        root: s.global_dir.join("repos"),
    }
}

/// Scope tag for a registered project: the workspace path's basename,
/// falling back to the workspace id if the path has no basename (e.g. `/`).
/// Used by [`load_detail`]'s single-name fallback lookup, which walks every
/// registered workspace (not deduped by repo — the first workspace with a
/// matching file wins, same as before).
fn project_scope_name(w: &rupu_workspace::Workspace) -> String {
    std::path::Path::new(&w.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| w.id.clone())
}

/// Pure core of `PUT /api/agents/:name`: validate the url name, parse + validate
/// the raw `.md` (no write on parse failure), enforce frontmatter-name ==
/// url-name, then atomically write to `<global_dir>/agents/<name>.md`. Returns
/// the written path.
fn save_agent_file(global_dir: &FsPath, url_name: &str, raw: &str) -> Result<PathBuf, ApiError> {
    validate_name(url_name)?;
    let spec =
        rupu_agent::AgentSpec::parse(raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    if spec.name != url_name {
        return Err(ApiError::bad_request(
            "frontmatter name must equal the agent name",
        ));
    }
    let dir = global_dir.join("agents");
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let target = dir.join(format!("{url_name}.md"));
    write_atomic(&target, raw.as_bytes()).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(target)
}

/// Pure core of `POST /api/agents`: parse the raw `.md` to derive the name,
/// validate it, refuse to clobber an existing file, then atomically write.
fn create_agent_file(global_dir: &FsPath, raw: &str) -> Result<PathBuf, ApiError> {
    let spec =
        rupu_agent::AgentSpec::parse(raw).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let name = spec.name.clone();
    validate_name(&name)?;
    let dir = global_dir.join("agents");
    let target = dir.join(format!("{name}.md"));
    if target.exists() {
        return Err(ApiError::conflict("agent already exists"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    write_atomic(&target, raw.as_bytes()).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(target)
}

/// Build the detail DTO from a loaded spec, tagged with `scope`.
fn detail_from_spec(spec: rupu_agent::spec::AgentSpec, scope: impl Into<String>) -> AgentDetailDto {
    let system_prompt = spec.system_prompt.clone();
    let raw = spec.raw.clone();
    AgentDetailDto {
        system_prompt,
        raw,
        summary: AgentDto::from_spec(spec, scope),
    }
}

/// Load agent `name` and build the full detail DTO. Shared by GET / PUT / POST.
///
/// Project-aware: resolves `name` in the global layer first, falling back to
/// every registered project's `.rupu/agents/` (first match) so a
/// project-only agent's detail route doesn't 404.
fn load_detail(s: &AppState, name: &str) -> ApiResult<AgentDetailDto> {
    match load_agent(&s.global_dir, None, name) {
        Ok(spec) => Ok(detail_from_spec(spec, "global")),
        Err(AgentLoadError::NotFound(_)) => {
            for w in store(s).list().unwrap_or_default() {
                let rupu_dir = std::path::Path::new(&w.path).join(".rupu");
                if let Ok(spec) = load_agent(&rupu_dir, None, name) {
                    return Ok(detail_from_spec(spec, project_scope_name(&w)));
                }
            }
            Err(ApiError::not_found(format!("agent {name} not found")))
        }
        Err(other) => Err(ApiError::internal(other.to_string())),
    }
}

#[derive(Serialize)]
pub(crate) struct AgentDto {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u32>,
    /// `"project"` when the spec was loaded from `<project>/.rupu/agents`,
    /// else `"global"`. Defaults to `"global"` for the global-only endpoints.
    pub(crate) scope: String,
    /// Aggregate token + cost usage across every run attributed to this agent.
    /// Defaults to empty; populated only by the list handler. Usage is
    /// grouped by agent NAME alone (the transcript rows the breakdown is
    /// computed from don't record which scope's definition ran), so only ONE
    /// canonical row per name (the `scope == "global"` row if one exists,
    /// else the first row for that name in sorted order) carries the
    /// aggregate — other same-named rows across different repos are left
    /// zeroed rather than showing duplicated combined usage. Per-scope usage
    /// attribution is a follow-up.
    pub(crate) usage: crate::usage::UsageSummary,
    /// Distinct runs attributed to this agent. Defaults to `0`.
    pub(crate) run_count: u64,
}

impl AgentDto {
    /// Map a loaded [`rupu_agent::spec::AgentSpec`] to the wire DTO, tagging
    /// it with the given scope.
    pub(crate) fn from_spec(spec: rupu_agent::spec::AgentSpec, scope: impl Into<String>) -> Self {
        AgentDto {
            name: spec.name,
            description: spec.description,
            provider: spec.provider,
            model: spec.model,
            effort: spec.effort.map(|e| format!("{e:?}")),
            max_tokens: spec.max_tokens,
            scope: scope.into(),
            usage: crate::usage::UsageSummary::default(),
            run_count: 0,
        }
    }
}

#[derive(Serialize)]
struct AgentDetailDto {
    #[serde(flatten)]
    summary: AgentDto,
    system_prompt: String,
    /// Full raw agent definition file (`.md` frontmatter + body), served so the
    /// CP can render it with syntax highlighting.
    raw: String,
}

/// `GET /api/agents` — global agent definitions plus one representative
/// workspace per distinct repo among the registered projects'
/// `<path>/.rupu/agents/*.md` (see [`distinct_repo_workspaces`]), sorted by
/// name then scope. Many registered workspaces are autoflow run-worktrees of
/// the same repo; scanning every registered workspace would otherwise emit
/// one duplicate row per worktree.
///
/// Each row is tagged `scope: "global"` or the representative workspace's
/// path basename. A project def shadows a same-named GLOBAL row; two
/// different repos defining the same name both appear (distinguished by
/// `scope`). With no registered projects this is byte-for-byte the prior
/// global-only behavior.
///
/// A malformed project agent file only drops that project's rows (logged via
/// `tracing::warn!`) rather than failing the whole list; the global scan's
/// error behavior is unchanged.
async fn list_agents(State(s): State<AppState>) -> ApiResult<Json<Vec<AgentDto>>> {
    let specs = load_agents(&s.global_dir, None).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut dtos: Vec<AgentDto> = specs
        .into_iter()
        .map(|spec| AgentDto::from_spec(spec, "global"))
        .collect();

    let workspaces = store(&s).list().unwrap_or_default();
    let repos = distinct_repo_workspaces(workspaces, &repo_store(&s));
    let mut project_dtos: Vec<AgentDto> = Vec::new();
    for r in repos {
        let scope = r.scope;
        let rupu_dir = std::path::Path::new(&r.workspace.path).join(".rupu");
        match load_agents(&rupu_dir, None) {
            Ok(specs) => project_dtos.extend(
                specs
                    .into_iter()
                    .map(|spec| AgentDto::from_spec(spec, scope.clone())),
            ),
            Err(err) => {
                tracing::warn!("agents: skipping project {scope}: {err}");
            }
        }
    }
    let project_names: std::collections::BTreeSet<&str> =
        project_dtos.iter().map(|d| d.name.as_str()).collect();
    dtos.retain(|d| !project_names.contains(d.name.as_str()));
    dtos.extend(project_dtos);
    dtos.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.scope.cmp(&b.scope)));

    // Aggregate every run's transcript, grouped by agent, to attach usage.
    // The breakdown is keyed by agent NAME alone (transcript rows don't
    // record which scope's definition ran), so — same as the workflows list
    // — attach it to only ONE canonical row per name (preferring
    // `scope == "global"`, else the first row for that name in the
    // already-sorted order) rather than duplicating it onto every same-named
    // row. See the doc comment on `AgentDto::usage`.
    let runs = s.run_store.list().unwrap_or_default();
    let mut all_paths: Vec<std::path::PathBuf> = Vec::new();
    for r in &runs {
        all_paths.extend(crate::usage::run_transcript_paths(&s.run_store, &r.id));
    }
    let rows = rupu_transcript::aggregate(&all_paths, rupu_transcript::TimeWindow::default());
    let breakdown = crate::usage::breakdown(&rows, &s.pricing, crate::usage::GroupBy::Agent);

    let mut canonical_dto_for_name: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (i, dto) in dtos.iter().enumerate() {
        canonical_dto_for_name
            .entry(dto.name.clone())
            .and_modify(|idx| {
                if dto.scope == "global" {
                    *idx = i;
                }
            })
            .or_insert(i);
    }
    for (name, idx) in canonical_dto_for_name {
        if let Some(b) = breakdown.iter().find(|b| b.agent == name) {
            let dto = &mut dtos[idx];
            dto.usage = crate::usage::UsageSummary {
                input_tokens: b.input_tokens,
                output_tokens: b.output_tokens,
                cached_tokens: b.cached_tokens,
                total_tokens: b.total_tokens,
                cost_usd: b.cost_usd,
                priced: b.priced,
                runs: b.runs,
            };
            dto.run_count = b.runs;
        }
    }

    Ok(Json(dtos))
}

async fn get_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<AgentDetailDto>> {
    Ok(Json(load_detail(&s, &name)?))
}

/// Request body for `PUT /api/agents/:name` and `POST /api/agents`: the full
/// raw `.md` (frontmatter + body) to validate and persist.
#[derive(Deserialize)]
struct AgentWriteBody {
    raw: String,
}

/// `PUT /api/agents/:name` — overwrite (or create) the global agent definition
/// `:name`. The raw `.md` is validated by [`rupu_agent::AgentSpec::parse`]
/// before any write; the frontmatter `name:` must equal `:name`. Returns the
/// reloaded detail DTO.
async fn write_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<AgentWriteBody>,
) -> ApiResult<Json<AgentDetailDto>> {
    save_agent_file(&s.global_dir, &name, &body.raw)?;
    Ok(Json(load_detail(&s, &name)?))
}

/// `POST /api/agents` — create a new global agent. The name is taken from the
/// parsed frontmatter; fails with 409 if a definition with that name already
/// exists. Returns the reloaded detail DTO.
async fn create_agent(
    State(s): State<AppState>,
    Json(body): Json<AgentWriteBody>,
) -> ApiResult<Json<AgentDetailDto>> {
    create_agent_file(&s.global_dir, &body.raw)?;
    let spec = rupu_agent::AgentSpec::parse(&body.raw)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(load_detail(&s, &spec.name)?))
}

/// `DELETE /api/agents/:name` — remove the global agent definition `:name`.
async fn delete_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_name(&name)?;
    let target = agents_dir(&s).join(format!("{name}.md"));
    if !target.exists() {
        return Err(ApiError::not_found(format!("agent {name} not found")));
    }
    std::fs::remove_file(&target).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// Request body for `POST /api/agents/:name/run`. All fields optional; a
/// bodyless POST launches the agent with no prompt in its default mode.
#[derive(Deserialize, Default)]
struct AgentRunBody {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    /// Optional host id. Absent or `"local"` → local path (including the
    /// existing 501 when no launcher is installed). A remote id proxies via
    /// [`HostConnector::launch_agent`] and returns `{ "run_id", "host_id" }`.
    #[serde(default)]
    host: Option<String>,
}

/// Testable core: map the body + a concrete launcher to a run id.
async fn run_agent_with(
    name: &str,
    body: AgentRunBody,
    launcher: Arc<dyn AgentLauncher>,
) -> Result<String, ApiError> {
    let req = AgentLaunchRequest {
        agent: name.to_string(),
        prompt: body.prompt,
        mode: body.mode,
        target: body.target,
        working_dir: body.working_dir,
    };
    launcher.launch(req).await.map_err(|e| match e {
        AgentLaunchError::Invalid(m) => ApiError::bad_request(m),
        AgentLaunchError::Spawn(m) => ApiError::internal(m),
    })
}

/// Start a fresh run of agent `:name` via the configured [`AgentLauncher`]
/// (local) or by proxying to a remote host. Returns the new run id plus the
/// owning `host_id`. 501 when no launcher is installed and the target is local.
///
/// [`AgentLauncher`]: crate::agent_launcher::AgentLauncher
async fn run_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<AgentRunBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let host = b.host.as_deref().unwrap_or("local").to_string();

    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, &host)?;
        let req = AgentLaunchRequest {
            agent: name.clone(),
            prompt: b.prompt,
            mode: b.mode,
            target: b.target,
            working_dir: b.working_dir,
        };
        let run_id = conn.launch_agent(req).await.map_err(|e| match e {
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
        .agent_launcher
        .clone()
        .ok_or_else(|| ApiError::not_available("launching agents requires `rupu cp serve`"))?;
    let run_id = run_agent_with(&name, b, launcher).await?;
    Ok(Json(
        serde_json::json!({ "run_id": run_id, "host_id": "local" }),
    ))
}

/// Request body for `POST /api/agents/:name/session`. All fields optional; a
/// bodyless POST starts the agent session with no prompt in its default mode.
#[derive(Deserialize, Default)]
struct SessionStartBody {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    /// Optional host id. Absent or `"local"` → local path (including the
    /// existing 501 when no starter is installed). A remote id proxies via
    /// [`HostConnector::start_session`] and returns `{ "session_id", "host_id" }`.
    #[serde(default)]
    host: Option<String>,
}

/// Testable core: map the body + a concrete starter to a session id.
async fn start_session_with(
    name: &str,
    body: SessionStartBody,
    starter: Arc<dyn SessionStarter>,
) -> Result<String, ApiError> {
    let req = SessionStartRequest {
        agent: name.to_string(),
        prompt: body.prompt,
        mode: body.mode,
        target: body.target,
        working_dir: body.working_dir,
    };
    starter.start(req).await.map_err(|e| match e {
        SessionStartError::Invalid(m) => ApiError::bad_request(m),
        SessionStartError::Spawn(m) => ApiError::internal(m),
    })
}

/// Start a fresh session of agent `:name` via the configured [`SessionStarter`]
/// (local) or by proxying to a remote host. Returns the new session id plus the
/// owning `host_id`. 501 when no starter is installed and the target is local.
///
/// [`SessionStarter`]: crate::session_starter::SessionStarter
async fn start_session(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<SessionStartBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let host = b.host.as_deref().unwrap_or("local").to_string();

    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, &host)?;
        let req = SessionStartRequest {
            agent: name.clone(),
            prompt: b.prompt,
            mode: b.mode,
            target: b.target,
            working_dir: b.working_dir,
        };
        let session_id = conn.start_session(req).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(
            serde_json::json!({ "session_id": session_id, "host_id": host }),
        ));
    }

    // Local path: unchanged (including the 501 when no starter is installed).
    let starter = s
        .session_starter
        .clone()
        .ok_or_else(|| ApiError::not_available("starting sessions requires `rupu cp serve`"))?;
    let session_id = start_session_with(&name, b, starter).await?;
    Ok(Json(
        serde_json::json!({ "session_id": session_id, "host_id": "local" }),
    ))
}

#[derive(Deserialize)]
struct GenerateAgentBody {
    description: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct GeneratedDefDto {
    raw: String,
    provider: String,
    model: String,
    attempts: u8,
}

async fn generate_agent(
    State(s): State<AppState>,
    Json(body): Json<GenerateAgentBody>,
) -> ApiResult<Json<GeneratedDefDto>> {
    use crate::definition_generator::{DefKind, GenDefError, GenerateDefRequest};
    let gen = s
        .generator
        .clone()
        .ok_or_else(|| ApiError::not_available("AI generation requires `rupu cp serve`"))?;
    let out = gen
        .generate(GenerateDefRequest {
            kind: DefKind::Agent,
            description: body.description,
            provider: body.provider,
            model: body.model,
        })
        .await
        .map_err(|e| match e {
            GenDefError::NoCredentials => ApiError::bad_request(e.to_string()),
            GenDefError::Failed(m) => ApiError::internal(m),
        })?;
    Ok(Json(GeneratedDefDto {
        raw: out.raw,
        provider: out.provider,
        model: out.model,
        attempts: out.attempts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
    use crate::session_starter::{SessionStartError, SessionStartRequest, SessionStarter};
    use std::sync::{Arc, Mutex};

    struct MockAgent {
        last: Mutex<Option<AgentLaunchRequest>>,
    }

    #[async_trait::async_trait]
    impl AgentLauncher for MockAgent {
        async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
            *self.last.lock().unwrap() = Some(req);
            Ok("run_A".into())
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
    async fn run_agent_forwards_request() {
        let mock = Arc::new(MockAgent {
            last: Mutex::new(None),
        });
        let body = AgentRunBody {
            prompt: Some("do it".into()),
            mode: Some("bypass".into()),
            target: None,
            working_dir: Some("/tmp/p".into()),
            host: None,
        };
        let run_id = run_agent_with("triage", body, mock.clone())
            .await
            .expect("ok");
        assert_eq!(run_id, "run_A");
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.agent, "triage");
        assert_eq!(got.prompt.as_deref(), Some("do it"));
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/p"));
    }

    #[tokio::test]
    async fn missing_launcher_is_not_available() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // agent_launcher: None

        let err = run_agent(State(s), Path("triage".into()), None)
            .await
            .expect_err("no launcher should error");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    const VALID_MD: &str = "---\nname: code-reviewer\nmodel: opus\n---\nReview code carefully.\n";

    #[test]
    fn save_writes_exact_bytes_at_named_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = save_agent_file(tmp.path(), "code-reviewer", VALID_MD).expect("save ok");
        assert_eq!(path, tmp.path().join("agents").join("code-reviewer.md"));
        let back = std::fs::read_to_string(&path).unwrap();
        assert_eq!(back, VALID_MD);
        // No leftover temp file.
        assert!(!path.with_extension("md.tmp").exists());
    }

    #[test]
    fn save_unparseable_is_bad_request_and_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = save_agent_file(tmp.path(), "code-reviewer", "no frontmatter here")
            .expect_err("should reject");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(!tmp.path().join("agents").join("code-reviewer.md").exists());
    }

    #[test]
    fn save_name_mismatch_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = save_agent_file(tmp.path(), "other-name", VALID_MD).expect_err("mismatch");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(!tmp.path().join("agents").join("other-name.md").exists());
    }

    #[test]
    fn create_conflicts_then_writes_using_frontmatter_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Absent → writes at the frontmatter-derived name.
        let path = create_agent_file(tmp.path(), VALID_MD).expect("create ok");
        assert_eq!(path, tmp.path().join("agents").join("code-reviewer.md"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), VALID_MD);
        // Present → conflict.
        let err = create_agent_file(tmp.path(), VALID_MD).expect_err("conflict");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[test]
    fn delete_present_then_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        save_agent_file(&s.global_dir, "code-reviewer", VALID_MD).expect("seed");

        let body = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(delete_agent(State(s.clone()), Path("code-reviewer".into())))
            .expect("delete ok");
        assert_eq!(body.0["deleted"], serde_json::json!(true));
        assert!(!agents_dir(&s).join("code-reviewer.md").exists());

        // Second delete → not found.
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(delete_agent(State(s.clone()), Path("code-reviewer".into())))
            .expect_err("absent");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    struct MockStarter {
        last: Mutex<Option<SessionStartRequest>>,
    }

    #[async_trait::async_trait]
    impl SessionStarter for MockStarter {
        async fn start(&self, req: SessionStartRequest) -> Result<String, SessionStartError> {
            *self.last.lock().unwrap() = Some(req);
            Ok("ses_TEST".into())
        }
    }

    #[tokio::test]
    async fn start_session_forwards_request() {
        let mock = Arc::new(MockStarter {
            last: Mutex::new(None),
        });
        let body = SessionStartBody {
            prompt: Some("hi".into()),
            mode: Some("ask".into()),
            target: None,
            working_dir: Some("/tmp/p".into()),
            host: None,
        };
        let id = start_session_with("triage", body, mock.clone())
            .await
            .expect("ok");
        assert_eq!(id, "ses_TEST");
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.agent, "triage");
        assert_eq!(got.prompt.as_deref(), Some("hi"));
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/p"));
    }

    #[tokio::test]
    async fn start_session_without_starter_is_not_available() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // session_starter: None
        let err = start_session(State(s), Path("triage".into()), None)
            .await
            .expect_err("no starter");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    use crate::definition_generator::{
        DefKind, DefinitionGenerator, GenDefError, GenerateDefRequest, GeneratedDef, ProviderModels,
    };

    struct StubGen;
    #[async_trait::async_trait]
    impl DefinitionGenerator for StubGen {
        async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError> {
            assert_eq!(req.kind, DefKind::Agent);
            Ok(GeneratedDef {
                raw: VALID_MD.to_string(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                attempts: 1,
            })
        }
        async fn available_models(&self) -> Vec<ProviderModels> {
            vec![ProviderModels {
                provider: "anthropic".into(),
                models: vec!["claude-sonnet-4-6".into()],
                is_default: true,
            }]
        }
    }

    #[tokio::test]
    async fn generate_agent_returns_content_without_writing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp).with_generator(Some(std::sync::Arc::new(StubGen)));
        let body = GenerateAgentBody {
            description: "x".into(),
            provider: None,
            model: None,
        };
        let Json(out) = generate_agent(State(state), Json(body)).await.expect("ok");
        assert!(out.raw.contains("name:"));
        // Nothing persisted by generate.
        assert!(
            !tmp.path().join("agents").exists()
                || std::fs::read_dir(tmp.path().join("agents"))
                    .unwrap()
                    .next()
                    .is_none()
        );
    }

    #[tokio::test]
    async fn generate_agent_without_adapter_is_not_available() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp); // generator = None
        let body = GenerateAgentBody {
            description: "x".into(),
            provider: None,
            model: None,
        };
        let err = generate_agent(State(state), Json(body)).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    /// Register a workspace record `<global_dir>/workspaces/<id>.toml` whose
    /// `path` points at `project_root`.
    fn register_workspace(tmp: &tempfile::TempDir, id: &str, project_root: &std::path::Path) {
        register_workspace_with_remote(tmp, id, project_root, None);
    }

    /// Same as [`register_workspace`], optionally tagging the record with a
    /// `repo_remote` (simulating autoflow run-worktrees of the same repo).
    fn register_workspace_with_remote(
        tmp: &tempfile::TempDir,
        id: &str,
        project_root: &std::path::Path,
        repo_remote: Option<&str>,
    ) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        let remote_line = repo_remote
            .map(|u| format!("repo_remote = \"{u}\"\n"))
            .unwrap_or_default();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\n{remote_line}created_at = \"2026-01-01T00:00:00Z\"\n",
                project_root.display()
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn list_no_projects_is_global_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        save_agent_file(&s.global_dir, "code-reviewer", VALID_MD).expect("seed");

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "code-reviewer");
        assert_eq!(rows[0].scope, "global");
    }

    #[tokio::test]
    async fn list_includes_project_defs_tagged_with_project_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        let expected_scope = proj
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(rows[0].name, "code-reviewer");
        assert_eq!(rows[0].scope, expected_scope);
    }

    #[tokio::test]
    async fn agent_detail_resolves_project_def() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        // Absent from global, present only in the project — must resolve, not 404.
        let resp = get_agent(State(s), Path("code-reviewer".into()))
            .await
            .expect("project-only agent should resolve via detail");
        assert_eq!(resp.0.summary.name, "code-reviewer");
        let expected_scope = proj
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(resp.0.summary.scope, expected_scope);
    }

    #[tokio::test]
    async fn same_repo_worktrees_dedupe_to_one_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        // Three registered workspaces = run-worktrees of the SAME repo, each
        // carrying its own copy of `.rupu/agents/code-reviewer.md`.
        let remote = "git@github.com:acme/widgets.git";
        for (id, name) in [
            ("ws_a", "worktree-a"),
            ("ws_b", "worktree-b"),
            ("ws_c", "worktree-c"),
        ] {
            let root = tmp.path().join(name);
            let proj_agents = root.join(".rupu").join("agents");
            std::fs::create_dir_all(&proj_agents).unwrap();
            std::fs::write(proj_agents.join("code-reviewer.md"), VALID_MD).unwrap();
            register_workspace_with_remote(&tmp, id, &root, Some(remote));
        }

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(
            rows.len(),
            1,
            "code-reviewer must appear exactly once despite 3 worktrees of the same repo"
        );
        assert_eq!(rows[0].name, "code-reviewer");
        // No tracked-repo record was seeded, so the tie-break is the
        // deterministic path sort: "worktree-a" sorts first.
        assert_eq!(rows[0].scope, "worktree-a");
    }

    #[tokio::test]
    async fn different_repos_same_def_name_both_appear() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj_x = tmp.path().join("proj-x");
        let agents_x = proj_x.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_x).unwrap();
        std::fs::write(agents_x.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let agents_y = proj_y.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_y).unwrap();
        std::fs::write(agents_y.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 2, "different repos are distinct groups");
        let scopes: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(
            scopes,
            std::collections::BTreeSet::from(["proj-x", "proj-y"])
        );
        assert!(rows.iter().all(|r| r.name == "code-reviewer"));
    }

    #[tokio::test]
    async fn no_repo_remote_scans_every_standalone_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        const OTHER_MD: &str = "---\nname: doc-writer\nmodel: opus\n---\nWrite docs.\n";

        let proj_a = tmp.path().join("standalone-a");
        let agents_a = proj_a.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_a).unwrap();
        std::fs::write(agents_a.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace(&tmp, "ws_a", &proj_a);

        let proj_b = tmp.path().join("standalone-b");
        let agents_b = proj_b.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_b).unwrap();
        std::fs::write(agents_b.join("doc-writer.md"), OTHER_MD).unwrap();
        register_workspace(&tmp, "ws_b", &proj_b);

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(
            rows.len(),
            2,
            "both standalone (no repo_remote) dirs are scanned"
        );
        let names: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            std::collections::BTreeSet::from(["code-reviewer", "doc-writer"])
        );
    }

    /// Write a single-event transcript (`RunStart` only, no `Usage` events) so
    /// `rupu_transcript::aggregate` emits exactly one zero-token row for
    /// `agent`, bumping that row's `runs` by 1.
    fn write_agent_transcript(path: &std::path::Path, agent: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let ev = rupu_transcript::Event::RunStart {
            run_id: "r1".into(),
            workspace_id: "ws1".into(),
            agent: agent.into(),
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            started_at: chrono::Utc::now(),
            mode: rupu_transcript::RunMode::Ask,
        };
        let mut line = serde_json::to_vec(&ev).unwrap();
        line.push(b'\n');
        std::fs::write(path, &line).unwrap();
    }

    /// Register a run of `workflow_name` bound to `workspace_id`, with one
    /// completed step whose transcript attributes usage to `agent`.
    fn seed_run_with_agent_usage(
        s: &AppState,
        run_id: &str,
        workspace_id: &str,
        agent: &str,
        transcript_path: &std::path::Path,
    ) {
        let record = rupu_orchestrator::RunRecord {
            id: run_id.into(),
            workflow_name: "wf".into(),
            status: rupu_orchestrator::RunStatus::Completed,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: workspace_id.into(),
            workspace_path: PathBuf::from("/tmp/proj"),
            transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            final_output: None,
        };
        s.run_store.create(record, "name: wf\n").unwrap();
        write_agent_transcript(transcript_path, agent);
        s.run_store
            .append_step_result(
                run_id,
                &rupu_orchestrator::runs::StepResultRecord {
                    step_id: "s1".into(),
                    run_id: run_id.into(),
                    transcript_path: transcript_path.to_path_buf(),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: rupu_orchestrator::runs::StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                },
            )
            .unwrap();
    }

    #[tokio::test]
    async fn same_named_rows_from_different_repos_do_not_both_show_combined_usage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj_x = tmp.path().join("proj-x");
        let agents_x = proj_x.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_x).unwrap();
        std::fs::write(agents_x.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let agents_y = proj_y.join(".rupu").join("agents");
        std::fs::create_dir_all(&agents_y).unwrap();
        std::fs::write(agents_y.join("code-reviewer.md"), VALID_MD).unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        // Two runs, both attributing usage to agent "code-reviewer" — as far
        // as the transcript breakdown is concerned they're indistinguishable
        // by scope (grouped by agent name alone).
        seed_run_with_agent_usage(
            &s,
            "run_1",
            "ws_x",
            "code-reviewer",
            &tmp.path().join("t1.jsonl"),
        );
        seed_run_with_agent_usage(
            &s,
            "run_2",
            "ws_y",
            "code-reviewer",
            &tmp.path().join("t2.jsonl"),
        );

        let Json(rows) = list_agents(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.name == "code-reviewer"));

        let run_counts: Vec<u64> = rows.iter().map(|r| r.run_count).collect();
        assert_eq!(
            run_counts.iter().sum::<u64>(),
            2,
            "the 2 runs are counted exactly once between the two rows combined"
        );
        assert_eq!(
            run_counts.iter().filter(|&&c| c == 2).count(),
            1,
            "exactly one row carries the combined run_count"
        );
        assert_eq!(
            run_counts.iter().filter(|&&c| c == 0).count(),
            1,
            "the other same-named row stays zeroed rather than duplicating usage"
        );
    }
}
