//! Project-owned entry point for the bundled coding harness. No definition
//! lookup, remote proxy, or caller-supplied execution directory is involved.
use crate::{
    error::{ApiError, ApiResult},
    session_starter::{SessionStartError, SessionStartRequest},
    state::AppState,
};
use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        Path, Query, State,
    },
    Json,
};
use rupu_agent::builtin::SESSION_AGENT_NAME;
use rupu_runtime::{provider_factory, session_settings::SessionSettings};
use serde::Deserialize;
use serde_json::{json, Value};

pub const CAP_BUILTIN_HARNESS: &str = "session.builtin_harness";
pub const CAP_IDLE_HARNESS: &str = "session.idle_harness";

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartBody {
    prompt: Option<String>,
    #[serde(default)]
    idle: bool,
    provider: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    skill_preset: Option<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    preload_skills: Vec<String>,
}

#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartQuery {
    host: Option<String>,
}

pub(crate) fn available(s: &AppState) -> bool {
    s.launcher.is_some()
        && s.session_starter
            .as_ref()
            .is_some_and(|s| s.supports_builtin_harness())
}

pub(crate) fn idle_available(s: &AppState) -> bool {
    available(s)
        && s.session_starter
            .as_ref()
            .is_some_and(|s| s.supports_idle_harness())
}

/// Resolve and pin the selection before the subprocess starts. Configuration
/// comes from the selected instance and exact project, including its customer.
fn prepare(s: &AppState, id: &str, body: StartBody) -> ApiResult<SessionStartRequest> {
    if body.idle {
        if body.prompt.is_some() {
            return Err(ApiError::bad_request(
                "idle session creation cannot include a prompt",
            ));
        }
    } else if body
        .prompt
        .as_deref()
        .is_none_or(|prompt| prompt.trim().is_empty())
    {
        return Err(ApiError::bad_request(
            "prompt must contain a message unless idle is true",
        ));
    }
    if let Some(preset) = body.skill_preset.as_deref() {
        if !matches!(preset, "rupu-session-v1" | "none") {
            return Err(ApiError::bad_request(
                "skill_preset must be rupu-session-v1 or none",
            ));
        }
    }
    for entry in body.skills.iter().chain(&body.preload_skills) {
        if !matches!(
            entry.parse::<rupu_skills::SkillGrant>(),
            Ok(rupu_skills::SkillGrant::Qualified(..))
        ) {
            return Err(ApiError::bad_request("session skills require exact global:name or project:name entries; wildcards are not accepted"));
        }
    }
    let settings = SessionSettings {
        provider: body.provider,
        model: body.model,
        ..Default::default()
    };
    settings
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mode = rupu_tools::PermissionMode::parse(body.mode.as_deref().unwrap_or("readonly"))
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if mode == rupu_tools::PermissionMode::Ask {
        return Err(ApiError::bad_request(
            "ask mode is unavailable: detached session workers have no interactive approval channel; choose readonly or explicitly choose bypass",
        ));
    }
    let workspace = super::projects::load_workspace(s, id)?;
    let path = std::path::Path::new(&workspace.path)
        .canonicalize()
        .map_err(|_| {
            ApiError::bad_request("registered project directory is missing; reload the project")
        })?;
    if !path.is_dir() {
        return Err(ApiError::bad_request(
            "registered project path must name a directory",
        ));
    }
    let config_paths = rupu_workspace::config_paths(&s.global_dir, Some(&path), &path)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let config = rupu_config::layer_files_locked(config_paths.layers())
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let provider = provider_factory::resolve_provider_name(
        settings.provider.as_deref(),
        config.default_provider.as_deref(),
    );
    if !provider_factory::is_dispatchable_provider(&provider, &config.providers) {
        return Err(ApiError::bad_request(format!(
            "provider {provider} is not configured on this Rupu instance"
        )));
    }
    let model = provider_factory::resolve_model(
        settings.model.as_deref(),
        config.default_model.as_deref(),
        provider_factory::openai_compatible_params(&provider, &config.providers)
            .as_ref()
            .map(|p| p.default_model.as_str()),
    );
    Ok(SessionStartRequest {
        skill_preset: body.skill_preset,
        agent: SESSION_AGENT_NAME.into(),
        provider: Some(provider),
        model: Some(model),
        workspace_id: Some(id.into()),
        idle: body.idle,
        prompt: body.prompt,
        mode: Some(mode.as_str().into()),
        target: None,
        working_dir: Some(path.to_string_lossy().into_owned()),
        skills: body.skills,
        preload_skills: body.preload_skills,
    })
}

pub(crate) async fn start(
    State(s): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<StartQuery>, QueryRejection>,
    body: Result<Json<StartBody>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Query(query) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.body_text()))?;
    super::fleet::require_local(query.host.as_deref(), "project-owned session creation")?;
    super::config::require_writable_to(&s, "starting project-owned sessions")?;
    let starter = s
        .session_starter
        .clone()
        .filter(|s| s.supports_builtin_harness())
        .ok_or_else(|| {
            ApiError::not_available(
                "the bundled session harness requires an updated `rupu cp serve` session starter",
            )
        })?;
    if body.idle && !starter.supports_idle_harness() {
        return Err(ApiError::not_available(
            "this session starter cannot create idle sessions",
        ));
    }
    if (body.skill_preset.is_some() || !body.skills.is_empty() || !body.preload_skills.is_empty())
        && !starter.supports_session_capabilities()
    {
        return Err(ApiError::not_available(
            "this session starter cannot apply session skill selections",
        ));
    }
    let request = super::runs::blocking(move || prepare(&s, &id, body)).await?;
    let (provider, model) = (request.provider.clone(), request.model.clone());
    let session_id = starter.start(request).await.map_err(|e| match e {
        SessionStartError::Invalid(message) => ApiError::bad_request(message),
        SessionStartError::Spawn(message) => ApiError::internal(message),
    })?;
    Ok(Json(
        json!({ "session_id": session_id, "host_id": "local", "agent": SESSION_AGENT_NAME, "provider": provider, "model": model }),
    ))
}

#[cfg(test)]
#[path = "project_session_start_tests.rs"]
mod tests;
