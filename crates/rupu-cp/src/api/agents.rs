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
    let spec = load_agent(&s.global_dir, None, &name).map_err(|e| match e {
        rupu_agent::loader::AgentLoadError::NotFound(_) => {
            ApiError::not_found(format!("agent {name} not found"))
        }
        other => ApiError::internal(other.to_string()),
    })?;
    let system_prompt = spec.system_prompt.clone();
    let raw = spec.raw.clone();
    Ok(Json(AgentDetailDto {
        system_prompt,
        raw,
        summary: AgentDto::from_spec(spec, "global"),
    }))
}
