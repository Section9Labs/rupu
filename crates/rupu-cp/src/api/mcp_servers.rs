//! Trusted-host MCP registry settings. Saving configuration does not connect
//! to servers, resolve credentials, discover tools, or alter running clients.
pub(crate) mod store;
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, Query, State},
    routing::{get, put},
    Json, Router,
};
use rupu_config::mcp::{Registry, ServerConfig, ToolEffect};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const FEATURE: &str = "mcp.server_configuration.v1";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/mcp/servers", get(list))
        .route("/api/mcp/servers/:id", put(upsert).delete(delete))
        .layer(DefaultBodyLimit::max(256 * 1024))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalQuery {
    host: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum CredentialEdit {
    Preserve,
    Clear,
    UseHostStore,
    SetReference { reference: String },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    revision: String,
    label: String,
    url: String,
    enabled: bool,
    credential: CredentialEdit,
    tool_effects: BTreeMap<String, ToolEffect>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    revision: String,
}
fn validate_id(id: &str) -> ApiResult<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(ApiError::bad_request(
            "invalid configured MCP server identifier",
        ));
    }
    Ok(())
}
fn inventory(
    registry: &Registry,
    revision: &str,
    writable: bool,
    mutation: Option<(&str, &str)>,
) -> Json<Value> {
    let servers: Vec<_> = registry
        .servers
        .iter()
        .map(|(id, server)| {
            json!({
                "id": id,
                "label": server.label,
                "url": server.url,
                "enabled": server.enabled,
                "credential": {
                    "kind": if server.bearer_store.is_some() { "host_store" } else if server.bearer_env.is_some() { "bearer_env" } else { "none" },
                    "configured": server.bearer_env.is_some() || server.bearer_store.is_some()
                },
                "tool_effects": server.tool_effects,
                "status": "configured_unverified"
            })
        })
        .collect();
    let mut result = json!({
        "host_id": "local",
        "scope": "trusted_host",
        "revision": revision,
        "writable": writable,
        "servers": servers,
        "applies": "new_sessions_and_next_turn_validation"
    });
    if let Some((action, id)) = mutation {
        result["mutation"] = json!({"action":action,"server_id":id});
    }
    Json(result)
}
async fn list(State(s): State<AppState>, Query(q): Query<LocalQuery>) -> ApiResult<Json<Value>> {
    super::fleet::require_local(q.host.as_deref(), "trusted host MCP configuration")?;
    super::runs::blocking(move || {
        let current = store::read(&s.global_dir)?;
        Ok(inventory(
            &current.registry,
            &current.revision,
            s.launcher.is_some(),
            None,
        ))
    })
    .await
}
async fn upsert(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LocalQuery>,
    body: Result<Json<Update>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    super::fleet::require_local(q.host.as_deref(), "trusted host MCP configuration")?;
    super::config::require_writable_to(&s, "configuring MCP servers")?;
    validate_id(&id)?;
    let Json(update) =
        body.map_err(|_| ApiError::bad_request("invalid MCP server configuration request"))?;
    super::runs::blocking(move || {
        let saved = store::mutate(&s.global_dir, &update.revision, |registry| {
            let previous = registry.servers.get(&id);
            let same_endpoint = previous.is_some_and(|server| {
                match (url::Url::parse(&server.url), url::Url::parse(&update.url)) {
                    (Ok(old), Ok(new)) => old == new,
                    _ => false,
                }
            });
            let host_binding = || {
                if same_endpoint {
                    previous.and_then(|server| server.bearer_store.clone())
                } else {
                    None
                }
                .unwrap_or_else(|| ulid::Ulid::new().to_string())
            };
            let (bearer_env, bearer_store) = match update.credential {
                CredentialEdit::Preserve => (
                    previous.and_then(|server| server.bearer_env.clone()),
                    previous
                        .filter(|server| server.bearer_store.is_some())
                        .map(|_| host_binding()),
                ),
                CredentialEdit::Clear => (None, None),
                CredentialEdit::SetReference { reference } => (Some(reference), None),
                CredentialEdit::UseHostStore => (None, Some(host_binding())),
            };
            let server = ServerConfig {
                label: update.label,
                url: update.url,
                enabled: update.enabled,
                bearer_env,
                bearer_store,
                tool_effects: update.tool_effects,
            };
            server
                .validate()
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            registry.servers.insert(id.clone(), server);
            Ok(())
        })?;
        Ok(inventory(
            &saved.registry,
            &saved.revision,
            true,
            Some(("upsert", &id)),
        ))
    })
    .await
}
async fn delete(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LocalQuery>,
    body: Result<Json<Delete>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    super::fleet::require_local(q.host.as_deref(), "trusted host MCP configuration")?;
    super::config::require_writable_to(&s, "removing MCP servers")?;
    validate_id(&id)?;
    let Json(update) =
        body.map_err(|_| ApiError::bad_request("invalid MCP server configuration request"))?;
    super::runs::blocking(move || {
        let saved = store::mutate(&s.global_dir, &update.revision, |registry| {
            registry
                .servers
                .remove(&id)
                .ok_or_else(|| ApiError::not_found("configured MCP server not found"))?;
            Ok(())
        })?;
        Ok(inventory(
            &saved.registry,
            &saved.revision,
            true,
            Some(("delete", &id)),
        ))
    })
    .await
}
#[cfg(test)]
mod tests;
