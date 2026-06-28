use crate::{
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use crate::api::host_fanout::{fan_out_rows, sort_values_newest_first};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/:id", get(get_session))
        .route(
            "/api/sessions/:id/usage-timeline",
            get(get_session_usage_timeline),
        )
        .route("/api/sessions/:id/runs", get(get_session_runs))
        .route("/api/sessions/:id/send", post(send_session))
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
    last_error: Option<String>,
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

#[derive(Deserialize)]
struct SessionsQuery {
    // Flat fields, NOT `#[serde(flatten)] PageQuery` — serde_urlencoded (axum
    // `Query`) cannot deserialize integers through a flattened struct.
    offset: Option<usize>,
    limit: Option<usize>,
    scope: Option<String>,
    /// Absent or `"all"` → fan-out across all hosts (tag each row `host_id`).
    /// `"local"` → local-only.
    /// Any other value → proxy to that remote host.
    #[serde(default)]
    host: Option<String>,
}

/// Optional `?host=<id>` query param for single-session detail/runs/usage
/// endpoints.
#[derive(Deserialize, Default)]
struct SessionHostQuery {
    #[serde(default)]
    host: Option<String>,
}

async fn list_sessions(
    State(s): State<AppState>,
    Query(q): Query<SessionsQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let host = q.host.as_deref().unwrap_or("all");

    // ── Single remote host ─────────────────────────────────────────────────────
    if host != "local" && host != "all" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        let path = {
            let mut p = "/api/sessions?host=local".to_string();
            if let Some(scope) = &q.scope {
                p.push_str("&scope=");
                p.push_str(scope);
            }
            if let Some(off) = q.offset {
                p.push_str(&format!("&offset={off}"));
            }
            if let Some(lim) = q.limit {
                p.push_str(&format!("&limit={lim}"));
            }
            p
        };
        let v = conn
            .proxy_get_json(&path)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let arr = v.as_array().cloned().unwrap_or_default();
        return Ok(Json(
            arr.into_iter()
                .map(|mut row| {
                    row["host_id"] = serde_json::json!(host);
                    row
                })
                .collect(),
        ));
    }

    // ── Collect local sessions ─────────────────────────────────────────────────
    let local_sessions = collect_sessions(&s.global_dir, &s.pricing);

    let page = crate::pagination::PageQuery {
        offset: q.offset,
        limit: q.limit,
    };

    // ── Local-only path ────────────────────────────────────────────────────────
    if host == "local" {
        let mut sessions = local_sessions;
        if let Some(scope) = q.scope.as_deref() {
            sessions.retain(|v| v.get("scope").and_then(|x| x.as_str()) == Some(scope));
        }
        let paged: Vec<serde_json::Value> = crate::pagination::paginate(sessions, &page)
            .into_iter()
            .map(|mut v| {
                v["host_id"] = serde_json::json!("local");
                v
            })
            .collect();
        return Ok(Json(paged));
    }

    // ── Fan-out path (host == "all") ───────────────────────────────────────────
    let local_values: Vec<serde_json::Value> = local_sessions
        .into_iter()
        .map(|mut v| {
            v["host_id"] = serde_json::json!("local");
            v
        })
        .collect();

    let remote_path = {
        let mut p = "/api/sessions?host=local&limit=10000".to_string();
        if let Some(scope) = &q.scope {
            p.push_str("&scope=");
            p.push_str(scope);
        }
        p
    };

    let mut all_values = fan_out_rows(&s.hosts, &remote_path, local_values).await;

    // Sort newest-first by updated_at (most recently active sessions first).
    sort_values_newest_first(&mut all_values, "updated_at");

    // Scope filter after merge.
    if let Some(scope) = q.scope.as_deref() {
        all_values.retain(|v| v.get("scope").and_then(|x| x.as_str()) == Some(scope));
    }

    Ok(Json(crate::pagination::paginate(all_values, &page)))
}

async fn get_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    // ── Remote proxy ───────────────────────────────────────────────────────────
    if let Some(host) = q.host.as_deref().filter(|h| *h != "local") {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        let v = conn
            .proxy_get_json(&format!("/api/sessions/{id}"))
            .await
            .map_err(|e| match e {
                HostConnectorError::NotFound(m) => ApiError::not_found(m),
                other => ApiError::internal(other.to_string()),
            })?;
        return Ok(Json(v));
    }

    // ── Local path (unchanged) ─────────────────────────────────────────────────
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

/// `GET /api/sessions/:id/usage-timeline[?host=<id>]` — ordered per-turn token
/// series across every run the session recorded (in order), labeled by run id.
///
/// With `?host=<remote-id>`: proxies to the owning host and forwards the
/// response verbatim. Local/absent: today's on-disk logic unchanged.
async fn get_session_usage_timeline(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    // ── Remote proxy ───────────────────────────────────────────────────────────
    if let Some(host) = q.host.as_deref().filter(|h| *h != "local") {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        let v = conn
            .proxy_get_json(&format!("/api/sessions/{id}/usage-timeline"))
            .await
            .map_err(|e| match e {
                HostConnectorError::NotFound(m) => ApiError::not_found(m),
                other => ApiError::internal(other.to_string()),
            })?;
        return Ok(Json(v));
    }

    // ── Local path (unchanged logic, boxed into Value) ─────────────────────────
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
    let points = crate::usage::turn_series(&labeled);
    let v = serde_json::to_value(points).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(v))
}

// ---------------------------------------------------------------------------
// Session runs (per-turn chat view) — /api/sessions/:id/runs
// ---------------------------------------------------------------------------

/// Minimal projection of `session.json` capturing just the `runs` array, used
/// for the per-turn chat view. Mirrors `run_streams::SessionForRunsDto` but
/// additionally retains each turn's `prompt`.
#[derive(Deserialize)]
struct SessionRunsChatEnvelope {
    #[serde(default)]
    runs: Vec<SessionRunChatRecord>,
}

/// One entry in `session.json`'s `runs` array, matching the on-disk field
/// names written by the CLI's `SessionRunRecord`. All fields are
/// `#[serde(default)]` so partial / evolving records still parse. `status` is
/// kept permissive (a `serde_json::Value`) since the CLI serialises it as the
/// snake_case strings `"ok"` / `"error"` / `"aborted"`.
#[derive(Deserialize)]
struct SessionRunChatRecord {
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    transcript_path: String,
    #[serde(default)]
    status: serde_json::Value,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
    #[serde(default)]
    total_tokens_in: u64,
    #[serde(default)]
    total_tokens_out: u64,
    #[serde(default)]
    total_tokens_cached: u64,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    error: Option<String>,
}

/// One turn in a session's chat view: its user prompt, transcript path, and
/// per-turn token/status metadata.
#[derive(Debug, Serialize)]
struct SessionRunRow {
    run_id: String,
    prompt: String,
    transcript_path: String,
    status: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    tokens_in: u64,
    tokens_out: u64,
    tokens_cached: u64,
    duration_ms: u64,
    error: Option<String>,
}

impl From<SessionRunChatRecord> for SessionRunRow {
    fn from(r: SessionRunChatRecord) -> Self {
        let status = match r.status {
            serde_json::Value::String(s) => Some(s.to_lowercase()),
            serde_json::Value::Null => None,
            other => Some(other.to_string().to_lowercase()),
        };
        Self {
            run_id: r.run_id,
            prompt: r.prompt,
            transcript_path: r.transcript_path,
            status,
            started_at: r.started_at,
            completed_at: r.completed_at,
            tokens_in: r.total_tokens_in,
            tokens_out: r.total_tokens_out,
            tokens_cached: r.total_tokens_cached,
            duration_ms: r.duration_ms,
            error: r.error,
        }
    }
}

/// Pure mapping from `session.json` text → ordered chat rows. Factored out so
/// it's unit-testable without spinning up the axum handler. A parse error is
/// surfaced to the caller (the handler maps it to a 500).
fn session_runs_from_json(text: &str) -> Result<Vec<SessionRunRow>, serde_json::Error> {
    let env: SessionRunsChatEnvelope = serde_json::from_str(text)?;
    Ok(env.runs.into_iter().map(SessionRunRow::from).collect())
}

/// `GET /api/sessions/:id/runs[?host=<id>]` — the session's ordered turns,
/// each with its user prompt, transcript path, status, and per-turn token
/// totals. Backs the web chat view.
///
/// With `?host=<remote-id>`: proxies to the owning host and forwards the
/// response verbatim. Local/absent: resolves active dir first, then archive;
/// 404 when neither exists or `session.json` is missing; 500 on a parse error.
async fn get_session_runs(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    // ── Remote proxy ───────────────────────────────────────────────────────────
    if let Some(host) = q.host.as_deref().filter(|h| *h != "local") {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        let v = conn
            .proxy_get_json(&format!("/api/sessions/{id}/runs"))
            .await
            .map_err(|e| match e {
                HostConnectorError::NotFound(m) => ApiError::not_found(m),
                other => ApiError::internal(other.to_string()),
            })?;
        return Ok(Json(v));
    }

    // ── Local path (unchanged logic) ───────────────────────────────────────────
    let active = s.global_dir.join("sessions").join(&id);
    let archive = s.global_dir.join("sessions-archive").join(&id);
    let dir = if active.is_dir() {
        active
    } else if archive.is_dir() {
        archive
    } else {
        return Err(ApiError::not_found(format!("session {id} not found")));
    };

    let path = dir.join("session.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found(format!("session {id} not found")));
        }
        Err(e) => {
            return Err(ApiError::internal(format!(
                "failed to read {}: {e}",
                path.display()
            )));
        }
    };
    let rows = session_runs_from_json(&text)
        .map_err(|e| ApiError::internal(format!("failed to parse {}: {e}", path.display())))?;
    let v = serde_json::to_value(rows).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(v))
}

/// Request body for `POST /api/sessions/:id/send`.
#[derive(Deserialize)]
struct SendBody {
    prompt: String,
}

/// Optional `?host=<id>` query param for `POST /api/sessions/:id/send`.
/// Absent or `"local"` → local path; a remote id proxies via
/// [`HostConnector::send_session_turn`].
#[derive(Deserialize, Default)]
struct SendQuery {
    #[serde(default)]
    host: Option<String>,
}

/// `POST /api/sessions/:id/send[?host=<id>]` — send a message to a live session.
///
/// Without `?host=` (or `?host=local`): uses the configured [`SessionSender`].
/// Returns the new run id plus `host_id: "local"`. 501 when no sender is
/// installed; 400 on an empty prompt; 404 when the session is missing; 409
/// when the session is stopped.
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::send_session_turn`]
/// and returns `{ "run_id", "host_id" }`. The local session-existence
/// pre-check is skipped for remote sends (the remote CP performs it).
///
/// [`SessionSender`]: crate::session_sender::SessionSender
async fn send_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SendQuery>,
    Json(body): Json<SendBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let prompt = body.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(ApiError::bad_request("prompt is empty"));
    }

    let host = q.host.as_deref().unwrap_or("local").to_string();

    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, &host)?;
        let req = crate::session_sender::SendMessageRequest {
            session_id: id,
            prompt,
        };
        let run_id = conn.send_session_turn(req).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(serde_json::json!({ "run_id": run_id, "host_id": host })));
    }

    // Local path: unchanged.
    let sender = s
        .session_sender
        .as_ref()
        .ok_or_else(|| ApiError::not_available("sending requires `rupu cp serve`"))?;

    // Best-effort pre-check: 404 for a missing session, 409 for a stopped one.
    let active_dir = s.global_dir.join("sessions").join(&id);
    let archive_dir = s.global_dir.join("sessions-archive").join(&id);
    let dir = if active_dir.is_dir() {
        Some(active_dir)
    } else if archive_dir.is_dir() {
        Some(archive_dir)
    } else {
        None
    };
    if let Some(dir) = dir {
        match load_session_file(&dir)? {
            Some(dto) => {
                if dto.status.as_str() == Some("stopped") {
                    return Err(ApiError::conflict(format!("session {id} is stopped")));
                }
            }
            None => return Err(ApiError::not_found(format!("session {id} not found"))),
        }
    } else {
        return Err(ApiError::not_found(format!("session {id} not found")));
    }

    let req = crate::session_sender::SendMessageRequest {
        session_id: id,
        prompt,
    };
    match sender.send(req).await {
        Ok(run_id) => Ok(Json(serde_json::json!({ "run_id": run_id, "host_id": "local" }))),
        Err(crate::session_sender::SendError::Invalid(m)) => Err(ApiError::bad_request(m)),
        Err(crate::session_sender::SendError::Spawn(m)) => Err(ApiError::internal(m)),
    }
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
            last_error: None,
            target: None,
            workspace_id: "w".into(),
        };
        let u = session_usage(&dto, &rupu_config::PricingConfig::default());
        assert_eq!(u.input_tokens, 1_000_000);
        assert!(u.priced);
        assert!((u.cost_usd.unwrap() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn session_runs_from_json_maps_turns_in_order() {
        let json = r#"{
            "session_id": "s1",
            "runs": [
                {
                    "run_id": "run_1",
                    "prompt": "first prompt",
                    "transcript_path": "/t/run_1.jsonl",
                    "status": "ok",
                    "started_at": "2026-06-26T00:00:00Z",
                    "completed_at": "2026-06-26T00:01:00Z",
                    "total_tokens_in": 100,
                    "total_tokens_out": 200,
                    "total_tokens_cached": 50,
                    "duration_ms": 1234
                },
                {
                    "run_id": "run_2",
                    "prompt": "second prompt",
                    "transcript_path": "/t/run_2.jsonl",
                    "status": "error",
                    "total_tokens_in": 1,
                    "total_tokens_out": 2,
                    "total_tokens_cached": 3,
                    "duration_ms": 9
                },
                {
                    "run_id": "run_3",
                    "prompt": "third prompt",
                    "transcript_path": "/t/run_3.jsonl",
                    "status": "error",
                    "error": "provider: API error 401",
                    "total_tokens_in": 0,
                    "total_tokens_out": 0,
                    "total_tokens_cached": 0,
                    "duration_ms": 0
                }
            ]
        }"#;
        let rows = session_runs_from_json(json).expect("parse");
        assert_eq!(rows.len(), 3);
        // Order preserved.
        assert_eq!(rows[0].run_id, "run_1");
        assert_eq!(rows[1].run_id, "run_2");
        assert_eq!(rows[2].run_id, "run_3");
        // Prompt + transcript preserved.
        assert_eq!(rows[0].prompt, "first prompt");
        assert_eq!(rows[0].transcript_path, "/t/run_1.jsonl");
        assert_eq!(rows[1].prompt, "second prompt");
        assert_eq!(rows[1].transcript_path, "/t/run_2.jsonl");
        // total_tokens_* mapped to tokens_*.
        assert_eq!(rows[0].tokens_in, 100);
        assert_eq!(rows[0].tokens_out, 200);
        assert_eq!(rows[0].tokens_cached, 50);
        assert_eq!(rows[0].duration_ms, 1234);
        // status lowercased; timestamps surfaced.
        assert_eq!(rows[0].status.as_deref(), Some("ok"));
        assert_eq!(rows[0].started_at.as_deref(), Some("2026-06-26T00:00:00Z"));
        assert_eq!(rows[0].completed_at.as_deref(), Some("2026-06-26T00:01:00Z"));
        assert_eq!(rows[1].status.as_deref(), Some("error"));
        assert_eq!(rows[1].started_at, None);
        // Per-run error is surfaced.
        assert_eq!(rows[0].error, None);
        assert_eq!(rows[2].error.as_deref(), Some("provider: API error 401"));
    }

    #[test]
    fn session_runs_from_json_no_runs_key_is_empty() {
        let rows = session_runs_from_json(r#"{"session_id":"s1"}"#).expect("parse");
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn get_session_runs_reads_active_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("sessions").join("sessX");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            r#"{"session_id":"sessX","runs":[{"run_id":"r1","prompt":"hello","transcript_path":"/t/r1.jsonl"}]}"#,
        )
        .unwrap();
        let s = crate::state::AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        let resp = get_session_runs(
            State(s),
            Path("sessX".into()),
            Query(SessionHostQuery::default()),
        )
        .await
        .expect("runs should load");
        let arr = resp.0.as_array().expect("array response");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["prompt"], "hello");
    }

    #[tokio::test]
    async fn get_session_runs_missing_session_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = crate::state::AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        let err = get_session_runs(
            State(s),
            Path("nope".into()),
            Query(SessionHostQuery::default()),
        )
        .await
        .expect_err("missing session should 404");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    use crate::session_sender::{SendError, SendMessageRequest, SessionSender};
    use std::sync::{Arc, Mutex};

    /// Captures the last `SendMessageRequest` and returns a canned run id.
    struct MockSender {
        last: Mutex<Option<SendMessageRequest>>,
        run_id: String,
    }

    #[async_trait::async_trait]
    impl SessionSender for MockSender {
        async fn send(&self, req: SendMessageRequest) -> Result<String, SendError> {
            *self.last.lock().unwrap() = Some(req);
            Ok(self.run_id.clone())
        }
    }

    /// Write a minimal active `session.json` for `id` under `global_dir`.
    fn write_active_session(global_dir: &std::path::Path, id: &str, status: &str) {
        let dir = global_dir.join("sessions").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            format!(r#"{{"session_id":"{id}","status":"{status}"}}"#),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn send_session_invokes_sender_and_returns_run_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_active_session(tmp.path(), "sess1", "active");
        let mock = Arc::new(MockSender {
            last: Mutex::new(None),
            run_id: "run_abc".into(),
        });
        let s = crate::state::AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_session_sender(Some(mock.clone()));

        let resp = send_session(
            State(s),
            Path("sess1".into()),
            Query(SendQuery { host: None }),
            Json(SendBody {
                prompt: "hi".into(),
            }),
        )
        .await
        .expect("send should succeed");
        assert_eq!(resp.0["run_id"], "run_abc");

        let captured = mock.last.lock().unwrap().clone().expect("request captured");
        assert_eq!(captured.session_id, "sess1");
        assert_eq!(captured.prompt, "hi");
    }

    #[tokio::test]
    async fn send_session_without_sender_is_not_implemented() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = crate::state::AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        ); // session_sender: None

        let err = send_session(
            State(s),
            Path("sess1".into()),
            Query(SendQuery { host: None }),
            Json(SendBody {
                prompt: "hi".into(),
            }),
        )
        .await
        .expect_err("no sender should error");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn send_session_empty_prompt_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mock = Arc::new(MockSender {
            last: Mutex::new(None),
            run_id: "run_abc".into(),
        });
        let s = crate::state::AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_session_sender(Some(mock));

        let err = send_session(
            State(s),
            Path("sess1".into()),
            Query(SendQuery { host: None }),
            Json(SendBody {
                prompt: "   ".into(),
            }),
        )
        .await
        .expect_err("empty prompt should error");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }
}
