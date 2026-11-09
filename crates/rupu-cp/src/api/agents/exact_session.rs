//! Reviewed exact global definition -> persistent conversation in a registered project.
use super::*;
use rupu_runtime::exact_agent::Binding;
use serde_json::{json, Value};
pub(crate) const FEATURE: &str = "agent.exact_global_conversation.v1";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewQuery {
    host: Option<String>,
    file_key: String,
    ws_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    host: Option<String>,
    binding: Binding,
    prompt: String,
    mode: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    tools: Option<Vec<String>>,
    tool_ceiling: Option<Vec<String>>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    preload_skills: Vec<String>,
    mcp_tools: Option<Vec<rupu_runtime::session_mcp::ToolRef>>,
    mcp_catalog_revision: Option<String>,
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agent-files/conversation-review", get(review))
        .route("/api/agent-files/conversation", post(start))
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
}
fn starter(s: &AppState) -> ApiResult<Arc<dyn SessionStarter>> {
    s.session_starter
        .clone()
        .filter(|v| v.supports_exact_global_conversation())
        .ok_or_else(|| {
            ApiError::not_available(
                "exact global conversation requires a supporting local session starter",
            )
        })
}
async fn review(State(s): State<AppState>, Query(q): Query<ReviewQuery>) -> ApiResult<Json<Value>> {
    super::super::fleet::require_local(q.host.as_deref(), "exact global conversation")?;
    starter(&s)?;
    super::super::runs::blocking(move || {
        let loaded = exact_run::load(&s, &q.file_key, &q.ws_id)?;
        Ok(Json(json!({"schema":FEATURE,"host_id":"local","definition_scope":"global","binding":loaded.review.binding,"definition_sha256":loaded.review.definition_sha256,"raw":loaded.review.raw,"provider":loaded.spec.provider,"model":loaded.spec.model,"description":loaded.spec.description,"supported_modes":["readonly","bypass"],"ask_unavailable_reason":"named_conversations_do_not_have_the_builtin_authenticated_approval_channel","skill_preset_semantics":"named_agent_skills_plus_explicit_additions; builtin_full_preset_not_applicable","source_semantics":"frozen_selected_agent_and_fallbacks; execution_project_skill_config_and_child_catalog_layers"})))
    }).await
}
async fn start(State(s): State<AppState>, Json(b): Json<Body>) -> ApiResult<Json<Value>> {
    super::super::fleet::require_local(b.host.as_deref(), "exact global conversation")?;
    let starter = starter(&s)?;
    b.binding
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if b.prompt.trim().is_empty() || b.prompt.len() > 32768 {
        return Err(ApiError::bad_request(
            "bounded nonempty first prompt required",
        ));
    }
    let mode = b.mode.unwrap_or_else(|| "readonly".into());
    if !matches!(mode.as_str(), "readonly" | "bypass") {
        return Err(ApiError::bad_request(
            "exact named conversation supports readonly or bypass only",
        ));
    }
    let provider = parse_override("provider", b.provider)?;
    let model = parse_override("model", b.model)?;
    if provider.is_some() && model.is_none() {
        return Err(ApiError::bad_request("provider override requires model"));
    }
    rupu_runtime::session_mcp::validate_request(
        b.mcp_tools.as_deref(),
        b.mcp_catalog_revision.as_deref(),
    )
    .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if (b.mcp_tools.is_some() || b.mcp_catalog_revision.is_some())
        && !starter.supports_session_mcp()
    {
        return Err(ApiError::not_available(
            "session starter cannot preserve explicit MCP selection",
        ));
    }
    rupu_runtime::session_settings::SessionSettings {
        tools: b.tools.clone(),
        ..Default::default()
    }
    .validate()
    .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if b.tools.is_some() && !starter.supports_session_tool_selection() {
        return Err(ApiError::not_available(
            "session starter cannot preserve explicit tool declaration",
        ));
    }
    if let Some(ceiling) = &b.tool_ceiling {
        if !starter.supports_session_tool_ceiling() {
            return Err(ApiError::not_available(
                "session starter cannot enforce immutable tool ceilings",
            ));
        }
        rupu_tools::tool_ceiling::validate_names(
            ceiling,
            &rupu_tools::ToolCatalog::builtin(),
            b.mcp_tools.iter().flatten().map(|tool| tool.local_name()),
        )
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    }
    let req = SessionStartRequest {
        agent: b.binding.effective_name.clone(),
        workspace_id: Some(b.binding.workspace_id.clone()),
        working_dir: Some(b.binding.project_path.clone()),
        prompt: Some(b.prompt),
        mode: Some(mode),
        provider,
        model,
        skills: parse_skill_entries("skills", b.skills)?,
        preload_skills: parse_skill_entries("preload_skills", b.preload_skills)?,
        mcp_tools: b.mcp_tools,
        mcp_catalog_revision: b.mcp_catalog_revision,
        tools: b.tools,
        tool_ceiling: b.tool_ceiling,
        effort: None,
        skill_preset: None,
        idle: false,
        target: None,
    };
    let binding = b.binding.clone();
    super::super::runs::blocking(move || {
        let loaded = exact_run::load(&s, &binding.file_key, &binding.workspace_id)?;
        if loaded.review.binding != binding {
            return Err(ApiError::conflict(
                "reviewed definition or project changed; review again",
            ));
        }
        Ok(())
    })
    .await?;
    let session_id = starter
        .start_exact_global(req, b.binding.clone())
        .await
        .map_err(|e| match e {
            SessionStartError::Invalid(m) => ApiError::bad_request(m),
            SessionStartError::Conflict(m) => ApiError::conflict(m),
            SessionStartError::Spawn(m) => ApiError::internal(m),
        })?;
    Ok(Json(
        json!({"schema":FEATURE,"host_id":"local","definition_scope":"global","binding":b.binding,"session_id":session_id,"status_semantics":"session_created_first_turn_submitted; not_completion"}),
    ))
}
#[cfg(test)]
mod tests;
