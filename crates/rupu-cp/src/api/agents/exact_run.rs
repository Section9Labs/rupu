//! Exact installed global agent -> separately registered local execution project.
use super::*;
use rupu_runtime::exact_agent::{self, Binding, Loaded};
use serde_json::{json, Value};
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
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    engagement_profiles: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    preload_skills: Vec<String>,
    #[serde(default)]
    findings_profile: Option<String>,
    #[serde(default)]
    messaging: LaunchMessaging,
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agent-files/run-review", get(review))
        .route("/api/agent-files/run", post(launch))
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
}
fn launcher(s: &AppState) -> ApiResult<Arc<dyn AgentLauncher>> {
    s.agent_launcher
        .clone()
        .filter(|l| l.supports_exact_global_agent())
        .ok_or_else(|| {
            ApiError::not_available("exact global launch requires a supporting local launcher")
        })
}
pub(super) fn load(s: &AppState, key: &str, ws: &str) -> ApiResult<Loaded> {
    let loaded = exact_agent::review(&s.global_dir, key, ws).map_err(|e| {
        ApiError::conflict(format!(
            "exact definition/project could not be reviewed: {e}"
        ))
    })?;
    let _: rupu_workspace::Workspace = toml::from_str(&loaded.registration_raw)
        .map_err(|_| ApiError::conflict("execution project registration is malformed"))?;
    rupu_agent::loader::validate_loaded_agent_in(
        &loaded.spec,
        &s.global_dir,
        Some(&FsPath::new(&loaded.review.binding.project_path).join(".rupu")),
        &[],
    )
    .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(loaded)
}
async fn review(State(s): State<AppState>, Query(q): Query<ReviewQuery>) -> ApiResult<Json<Value>> {
    super::super::fleet::require_local(q.host.as_deref(), "exact global agent launch")?;
    launcher(&s)?;
    super::super::runs::blocking(move||{
        let loaded=load(&s,&q.file_key,&q.ws_id)?;
        Ok(Json(json!({"schema":exact_agent::FEATURE,"host_id":"local","definition_scope":"global","binding":loaded.review.binding,
            "definition_sha256":loaded.review.definition_sha256,"raw":loaded.review.raw,"provider":loaded.spec.provider,"model":loaded.spec.model,"description":loaded.spec.description,
            "source_semantics":"original_global_file_bytes; execution_project_skill_config_and_child_catalog_layers"})))
    }).await
}
async fn launch(State(s): State<AppState>, Json(b): Json<Body>) -> ApiResult<Json<Value>> {
    super::super::fleet::require_local(b.host.as_deref(), "exact global agent launch")?;
    let launcher = launcher(&s)?;
    if b.prompt.trim().is_empty() || b.prompt.len() > 32768 {
        return Err(ApiError::bad_request(
            "a bounded nonempty prompt is required",
        ));
    }
    let mode = b.mode.as_deref().unwrap_or("readonly");
    if !["readonly", "bypass"].contains(&mode) {
        return Err(ApiError::bad_request(
            "exact detached agent launch supports readonly or bypass only",
        ));
    }
    b.binding
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let req = AgentLaunchRequest {
        agent: b.binding.effective_name.clone(),
        prompt: Some(b.prompt),
        mode: Some(mode.into()),
        target: None,
        working_dir: Some(b.binding.project_path.clone()),
        run_id: None,
        codename: None,
        continue_from: None,
        model: parse_override("model", b.model)?,
        provider: parse_override("provider", b.provider)?,
        findings_profile: parse_findings_profile(b.findings_profile.as_deref())?,
        engagement_profiles: parse_engagement_profiles(b.engagement_profiles)?,
        skills: parse_skill_entries("skills", b.skills)?,
        preload_skills: parse_skill_entries("preload_skills", b.preload_skills)?,
        skill_bundle: None,
        messaging: parse_messaging(b.messaging)?,
    };
    let check = s.clone();
    let binding = b.binding.clone();
    super::super::runs::blocking(move || {
        let loaded = load(&check, &binding.file_key, &binding.workspace_id)?;
        if loaded.review.binding != binding {
            return Err(ApiError::conflict(
                "reviewed definition or project changed; review again",
            ));
        }
        Ok(())
    })
    .await?;
    let run = launcher
        .launch_exact_global(req, b.binding.clone())
        .await
        .map_err(|e| match e {
            AgentLaunchError::Invalid(m) => ApiError::bad_request(m),
            AgentLaunchError::Spawn(m) => ApiError::internal(m),
        })?;
    Ok(Json(
        json!({"schema":exact_agent::FEATURE,"host_id":"local","run_id":run,"definition_scope":"global","binding":b.binding}),
    ))
}

#[cfg(test)]
mod tests;
