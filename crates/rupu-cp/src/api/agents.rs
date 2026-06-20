use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use rupu_agent::loader::{load_agent, load_agents};
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agents", get(list_agents))
        .route("/api/agents/:name", get(get_agent))
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
        }
    }
}

#[derive(Serialize)]
struct AgentDetailDto {
    #[serde(flatten)]
    summary: AgentDto,
    system_prompt: String,
}

async fn list_agents(State(s): State<AppState>) -> ApiResult<Json<Vec<AgentDto>>> {
    let specs = load_agents(&s.global_dir, None).map_err(|e| ApiError::internal(e.to_string()))?;
    let dtos = specs
        .into_iter()
        .map(|spec| AgentDto::from_spec(spec, "global"))
        .collect();
    Ok(Json(dtos))
}

async fn get_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<AgentDetailDto>> {
    let spec = load_agent(&s.global_dir, None, &name).map_err(|e| match e {
        rupu_agent::loader::AgentLoadError::NotFound(_) => {
            ApiError::not_found(format!("agent {name} not found"))
        }
        other => ApiError::internal(other.to_string()),
    })?;
    let system_prompt = spec.system_prompt.clone();
    Ok(Json(AgentDetailDto {
        system_prompt,
        summary: AgentDto::from_spec(spec, "global"),
    }))
}
