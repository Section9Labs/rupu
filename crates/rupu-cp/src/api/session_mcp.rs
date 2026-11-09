//! Local configured MCP metadata and revisioned next-turn selection. No tools
//! are called by these endpoints; endpoint/auth policy stays host-owned.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_runtime::{
    session_mcp::{self, FrozenSelection, ToolRef},
    session_settings::SessionSettings,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub const FEATURE: &str = "session.mcp_tools.v1";
pub(crate) fn available(s: &AppState) -> bool {
    super::project_session_start::available(s)
        && s.session_starter
            .as_ref()
            .is_some_and(|starter| starter.supports_session_mcp())
}
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/mcp/tools", get(catalog)).route(
        "/api/sessions/:id/mcp",
        get(get_selection).put(put_selection),
    )
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolChoice {
    pub server: String,
    pub tool: String,
}
impl From<ToolChoice> for ToolRef {
    fn from(value: ToolChoice) -> Self {
        Self {
            server: value.server,
            tool: value.tool,
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalQuery {
    host: Option<String>,
}
fn gate(s: &AppState, host: Option<&str>) -> ApiResult<()> {
    super::fleet::require_local(host, "configured session MCP tools")?;
    if !available(s) {
        return Err(ApiError::not_available(
            "this session starter cannot freeze and enforce configured MCP tools",
        ));
    }
    Ok(())
}
pub(crate) fn error(error: session_mcp::Error) -> ApiError {
    match error {
        session_mcp::Error::Stale => ApiError::conflict(error.to_string()),
        session_mcp::Error::InvalidSelection => ApiError::bad_request(error.to_string()),
        _ => ApiError::bad_gateway(error.to_string()),
    }
}
pub(crate) async fn discover(s: &AppState) -> ApiResult<session_mcp::CatalogSnapshot> {
    let registry = rupu_config::mcp::Registry::load(&s.global_dir)
        .map_err(|e| ApiError::bad_gateway(e.to_string()))?;
    session_mcp::discover(&registry, &rupu_mcp_client::EnvironmentCredentials)
        .await
        .map_err(error)
}
async fn catalog(State(s): State<AppState>, Query(q): Query<LocalQuery>) -> ApiResult<Json<Value>> {
    gate(&s, q.host.as_deref())?;
    let snapshot = discover(&s).await?;
    Ok(Json(
        json!({"host_id":"local","revision":snapshot.revision,"tools":snapshot.tools,"default":"enabled_host_servers","permission":"host_effects_and_session_mode"}),
    ))
}
fn load(
    s: &AppState,
    id: &str,
) -> ApiResult<(rupu_runtime::sessions::SessionRecord, SessionSettings)> {
    super::runs::validate_id(id)?;
    super::sessions::active_session(s, id, "MCP selection")?;
    let dir = s.global_dir.join("sessions").join(id);
    let bytes =
        std::fs::read(dir.join("session.json")).map_err(|e| ApiError::internal(e.to_string()))?;
    let record = rupu_runtime::sessions::parse_session_record(&bytes)
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    if record.session_id != id || record.workspace_id.is_empty() {
        return Err(ApiError::conflict(
            "session ownership changed; reload before editing",
        ));
    }
    if let Some(selection) = &record.mcp_tools {
        selection
            .validate()
            .map_err(|e| ApiError::conflict(e.to_string()))?;
    }
    let settings = SessionSettings::read(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    settings
        .validate()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((record, settings))
}
fn identities(selection: &FrozenSelection) -> Vec<&ToolRef> {
    selection.tools.iter().map(|t| &t.identity).collect()
}
fn response(
    record: &rupu_runtime::sessions::SessionRecord,
    settings: &SessionSettings,
) -> Json<Value> {
    let default = record.mcp_tools.clone().unwrap_or_default();
    let effective = settings.mcp.as_ref().unwrap_or(&default);
    Json(
        json!({"session_id":record.session_id,"host_id":"local","workspace_id":record.workspace_id,
        "revision":settings.revision,"tools":settings.mcp.as_ref().map(identities),"defaults":identities(&default),"effective":identities(effective),
        "permission_mode":record.permission_mode,"applies":"next_turn"}),
    )
}
async fn get_selection(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LocalQuery>,
) -> ApiResult<Json<Value>> {
    gate(&s, q.host.as_deref())?;
    let (record, settings) = load(&s, &id)?;
    Ok(response(&record, &settings))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    revision: u64,
    // The field is required even though explicit null is meaningful.
    #[serde(deserialize_with = "required_tools")]
    tools: Option<Vec<ToolChoice>>,
    catalog_revision: Option<String>,
}
fn required_tools<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Vec<ToolChoice>>, D::Error> {
    Option::deserialize(d)
}
async fn put_selection(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LocalQuery>,
    Json(edit): Json<Edit>,
) -> ApiResult<Json<Value>> {
    gate(&s, q.host.as_deref())?;
    super::config::require_writable_to(&s, "changing session MCP tools")?;
    let (record, mut settings) = load(&s, &id)?;
    if settings.revision != edit.revision {
        return Err(ApiError::conflict("settings changed; reload before saving"));
    }
    settings.mcp = match edit.tools {
        None => None,
        Some(tools) if tools.is_empty() => Some(FrozenSelection::default()),
        Some(tools) => {
            let choices: Vec<ToolRef> = tools.into_iter().map(Into::into).collect();
            session_mcp::validate_request(Some(&choices), edit.catalog_revision.as_deref())
                .map_err(error)?;
            Some(
                discover(&s)
                    .await?
                    .freeze(Some(&choices), edit.catalog_revision.as_deref())
                    .map_err(error)?,
            )
        }
    };
    // Recheck active ownership after network discovery; save's shared lock/CAS
    // rejects a concurrent builtin-tools edit, including one made mid-discovery.
    super::sessions::active_session(&s, &id, "MCP selection")?;
    let saved = settings
        .save(&s.global_dir.join("sessions").join(&id))
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(response(&record, &saved))
}

#[cfg(test)]
#[path = "session_mcp/tests.rs"]
mod tests;
