use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/:id", get(get_session))
        .route(
            "/api/sessions/:id/usage-timeline",
            get(get_session_usage_timeline),
        )
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
    #[serde(default)]
    provider_name: String,
    /// Accepts whatever enum variant the serialiser produces.
    #[serde(default)]
    status: serde_json::Value,
    #[serde(default)]
    total_turns: u32,
    #[serde(default)]
    total_tokens_in: u64,
    #[serde(default)]
    total_tokens_out: u64,
    #[serde(default)]
    total_tokens_cached: u64,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    active_run_id: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    workspace_id: String,
}

/// Try to load and parse `session.json` inside `dir`.
///
/// Returns:
/// - `Ok(None)`  — file does not exist (caller should treat as 404)
/// - `Ok(Some)`  — file exists and parsed successfully
/// - `Err(_)`    — file exists but could not be read or parsed (→ 500)
fn load_session_file(dir: &std::path::Path) -> Result<Option<SessionDto>, ApiError> {
    let path = dir.join("session.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ApiError::internal(format!(
                "failed to read {}: {e}",
                path.display()
            )));
        }
    };
    match serde_json::from_str::<SessionDto>(&text) {
        Ok(dto) => Ok(Some(dto)),
        Err(e) => Err(ApiError::internal(format!(
            "failed to parse {}: {e}",
            path.display()
        ))),
    }
}

/// Try to load and parse `session.json` inside `dir` for list scanning.
/// Returns `None` when the file is absent or fails to parse (with a warning).
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

/// Token + cost summary for a session, derived from its on-disk token totals
/// (sessions record their own totals; no transcript aggregation needed).
fn session_usage(
    dto: &SessionDto,
    pricing: &rupu_config::PricingConfig,
) -> crate::usage::UsageSummary {
    let total_tokens = dto.total_tokens_in + dto.total_tokens_out;
    let cost_usd =
        rupu_config::pricing::lookup(pricing, &dto.provider_name, &dto.model, &dto.agent_name)
            .map(|p| p.cost_usd(dto.total_tokens_in, dto.total_tokens_out, dto.total_tokens_cached));
    crate::usage::UsageSummary {
        input_tokens: dto.total_tokens_in,
        output_tokens: dto.total_tokens_out,
        cached_tokens: dto.total_tokens_cached,
        total_tokens,
        priced: cost_usd.is_some(),
        cost_usd,
        runs: 1,
    }
}

/// Scan `<root>` for `<id>/session.json` entries. Assigns `scope` to
/// each successfully parsed session and pushes it onto `out`.
fn scan_session_dir(
    root: &std::path::Path,
    scope: &str,
    pricing: &rupu_config::PricingConfig,
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
            let usage = session_usage(&dto, pricing);
            match serde_json::to_value(&dto) {
                Ok(mut val) => {
                    if let serde_json::Value::Object(ref mut map) = val {
                        map.insert(
                            "scope".to_string(),
                            serde_json::Value::String(scope.to_string()),
                        );
                        if let Ok(u) = serde_json::to_value(&usage) {
                            map.insert("usage".to_string(), u);
                        }
                    }
                    out.push(val);
                }
                Err(e) => {
                    tracing::warn!(
                        session_dir = %dir.display(),
                        error = %e,
                        "failed to serialize session dto; skipping"
                    );
                }
            }
        }
    }
}

/// Collect all sessions from both active and archive dirs. Each entry has an
/// injected `"scope"` key (`"active"` or `"archived"`). Exposed as
/// `pub(crate)` so that the dashboard aggregate can reuse the scan without
/// duplicating logic.
pub(crate) fn collect_sessions(
    global_dir: &std::path::Path,
    pricing: &rupu_config::PricingConfig,
) -> Vec<serde_json::Value> {
    let mut sessions = Vec::new();
    scan_session_dir(
        &global_dir.join("sessions"),
        "active",
        pricing,
        &mut sessions,
    );
    scan_session_dir(
        &global_dir.join("sessions-archive"),
        "archived",
        pricing,
        &mut sessions,
    );
    sessions
}

async fn list_sessions(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let sessions = collect_sessions(&s.global_dir, &s.pricing);
    Ok(Json(crate::pagination::paginate(sessions, &page)))
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

    // load_session_file distinguishes missing (Ok(None)→404) from IO/parse
    // errors on an existing file (Err→500).
    let dto = match load_session_file(&dir)? {
        Some(dto) => dto,
        None => return Err(ApiError::not_found(format!("session {id} not found"))),
    };

    let usage = session_usage(&dto, &s.pricing);
    let mut val =
        serde_json::to_value(&dto).map_err(|e| ApiError::internal(e.to_string()))?;
    if let serde_json::Value::Object(ref mut map) = val {
        map.insert(
            "scope".to_string(),
            serde_json::Value::String(scope.to_string()),
        );
        if let Ok(u) = serde_json::to_value(&usage) {
            map.insert("usage".to_string(), u);
        }
    }
    Ok(Json(val))
}

/// Minimal projection of `session.json` for the usage-timeline endpoint:
/// just the `runs` array, each carrying its run id + transcript path.
#[derive(Deserialize)]
struct SessionRunsEnvelope {
    #[serde(default)]
    runs: Vec<SessionRunEntry>,
}

#[derive(Deserialize)]
struct SessionRunEntry {
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
}

/// `GET /api/sessions/:id/usage-timeline` — ordered per-turn token series across
/// every run the session recorded (in order), labeled by run id.
async fn get_session_usage_timeline(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<crate::usage::TurnPoint>>> {
    let active = s.global_dir.join("sessions").join(&id);
    let archive = s.global_dir.join("sessions-archive").join(&id);
    let dir = if active.is_dir() {
        active
    } else if archive.is_dir() {
        archive
    } else {
        return Err(ApiError::not_found(format!("session {id} not found")));
    };
    let text = std::fs::read_to_string(dir.join("session.json"))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let env: SessionRunsEnvelope =
        serde_json::from_str(&text).unwrap_or(SessionRunsEnvelope { runs: vec![] });
    let mut labeled: Vec<(String, std::path::PathBuf)> = Vec::new();
    for r in &env.runs {
        if let Some(tp) = &r.transcript_path {
            labeled.push((r.run_id.clone(), std::path::PathBuf::from(tp)));
        }
    }
    Ok(Json(crate::usage::turn_series(&labeled)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_usage_from_dto_prices_known_model() {
        let dto = SessionDto {
            session_id: "s1".into(),
            agent_name: "a".into(),
            model: "claude-sonnet-4-6".into(),
            provider_name: "anthropic".into(),
            status: serde_json::Value::String("active".into()),
            total_turns: 3,
            total_tokens_in: 1_000_000,
            total_tokens_out: 0,
            total_tokens_cached: 0,
            created_at: String::new(),
            updated_at: String::new(),
            active_run_id: None,
            target: None,
            workspace_id: "w".into(),
        };
        let u = session_usage(&dto, &rupu_config::PricingConfig::default());
        assert_eq!(u.input_tokens, 1_000_000);
        assert!(u.priced);
        assert!((u.cost_usd.unwrap() - 3.0).abs() < 1e-9);
    }
}
