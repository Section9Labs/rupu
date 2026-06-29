use crate::{
    agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher},
    api::fs_safety::{validate_name, write_atomic},
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
use rupu_agent::loader::{load_agent, load_agents};
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

/// Load agent `name` and build the full detail DTO. Shared by GET / PUT / POST.
fn load_detail(s: &AppState, name: &str) -> ApiResult<AgentDetailDto> {
    let spec = load_agent(&s.global_dir, None, name).map_err(|e| match e {
        rupu_agent::loader::AgentLoadError::NotFound(_) => {
            ApiError::not_found(format!("agent {name} not found"))
        }
        other => ApiError::internal(other.to_string()),
    })?;
    let system_prompt = spec.system_prompt.clone();
    let raw = spec.raw.clone();
    Ok(AgentDetailDto {
        system_prompt,
        raw,
        summary: AgentDto::from_spec(spec, "global"),
    })
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
    pub(crate) scope: &'static str,
    /// Aggregate token + cost usage across every run attributed to this agent.
    /// Defaults to empty; populated only by the list handler.
    pub(crate) usage: crate::usage::UsageSummary,
    /// Distinct runs attributed to this agent. Defaults to `0`.
    pub(crate) run_count: u64,
}

impl AgentDto {
    /// Map a loaded [`rupu_agent::spec::AgentSpec`] to the wire DTO, tagging
    /// it with the given scope.
    pub(crate) fn from_spec(spec: rupu_agent::spec::AgentSpec, scope: &'static str) -> Self {
        AgentDto {
            name: spec.name,
            description: spec.description,
            provider: spec.provider,
            model: spec.model,
            effort: spec.effort.map(|e| format!("{e:?}")),
            max_tokens: spec.max_tokens,
            scope,
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

async fn list_agents(State(s): State<AppState>) -> ApiResult<Json<Vec<AgentDto>>> {
    let specs = load_agents(&s.global_dir, None).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut dtos: Vec<AgentDto> = specs
        .into_iter()
        .map(|spec| AgentDto::from_spec(spec, "global"))
        .collect();

    // Aggregate every run's transcript, grouped by agent, to attach usage.
    let runs = s.run_store.list().unwrap_or_default();
    let mut all_paths: Vec<std::path::PathBuf> = Vec::new();
    for r in &runs {
        all_paths.extend(crate::usage::run_transcript_paths(&s.run_store, &r.id));
    }
    let rows = rupu_transcript::aggregate(&all_paths, rupu_transcript::TimeWindow::default());
    let breakdown = crate::usage::breakdown(&rows, &s.pricing, crate::usage::GroupBy::Agent);
    for dto in &mut dtos {
        if let Some(b) = breakdown.iter().find(|b| b.agent == dto.name) {
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
}
