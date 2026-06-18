use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/:id", get(get_session))
}

/// Minimal projection of the on-disk `session.json`. All fields are
/// `#[serde(default)]` so that unknown / missing fields don't cause
/// parse failures as the schema evolves. The `message_history` field
/// is deliberately excluded — it can be very large.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionDto {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    agent_name: String,
    #[serde(default)]
    model: String,
    /// Accepts whatever enum variant the serialiser produces.
    #[serde(default)]
    status: serde_json::Value,
    #[serde(default)]
    total_turns: u32,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    active_run_id: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

/// Try to load and parse `session.json` inside `dir`. Returns `None`
/// when the file is absent or fails to parse (with a warning).
fn try_load_session(dir: &std::path::Path) -> Option<SessionDto> {
    let path = dir.join("session.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "skipping unreadable session.json");
            return None;
        }
    };
    match serde_json::from_str::<SessionDto>(&text) {
        Ok(dto) => Some(dto),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "skipping unparseable session.json");
            None
        }
    }
}

/// Scan `<root>` for `<id>/session.json` entries. Assigns `scope` to
/// each successfully parsed session and pushes it onto `out`.
fn scan_session_dir(
    root: &std::path::Path,
    scope: &str,
    out: &mut Vec<serde_json::Value>,
) {
    if !root.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(dir = %root.display(), error = %e, "failed to read session directory");
            return;
        }
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if let Some(dto) = try_load_session(&dir) {
            let mut val = serde_json::to_value(dto).unwrap_or(serde_json::Value::Null);
            if let serde_json::Value::Object(ref mut map) = val {
                map.insert("scope".to_string(), serde_json::Value::String(scope.to_string()));
            }
            out.push(val);
        }
    }
}

async fn list_sessions(
    State(s): State<AppState>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let mut sessions = Vec::new();
    scan_session_dir(&s.global_dir.join("sessions"), "active", &mut sessions);
    scan_session_dir(
        &s.global_dir.join("sessions-archive"),
        "archived",
        &mut sessions,
    );
    Ok(Json(sessions))
}

async fn get_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Try active first, then archive.
    let active_dir = s.global_dir.join("sessions").join(&id);
    let archive_dir = s.global_dir.join("sessions-archive").join(&id);

    let (dir, scope) = if active_dir.is_dir() {
        (active_dir, "active")
    } else if archive_dir.is_dir() {
        (archive_dir, "archived")
    } else {
        return Err(ApiError::not_found(format!("session {id} not found")));
    };

    let dto = try_load_session(&dir)
        .ok_or_else(|| ApiError::not_found(format!("session {id} session.json missing or unparseable")))?;

    let mut val =
        serde_json::to_value(dto).map_err(|e| ApiError::internal(e.to_string()))?;
    if let serde_json::Value::Object(ref mut map) = val {
        map.insert(
            "scope".to_string(),
            serde_json::Value::String(scope.to_string()),
        );
    }
    Ok(Json(val))
}
