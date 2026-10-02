use crate::api::host_fanout::{fan_out_sessions, sort_values_newest_first};
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
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/:id", get(get_session).delete(delete_session))
        .route(
            "/api/sessions/:id/usage-timeline",
            get(get_session_usage_timeline),
        )
        .route("/api/sessions/:id/runs", get(get_session_runs))
        .route("/api/sessions/:id/send", post(send_session))
        .route("/api/sessions/:id/archive", post(archive_session))
        .route("/api/sessions/:id/restore", post(restore_session))
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
    /// Stored crew/role codename; absent on legacy sessions (derived on read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codename: Option<String>,
    /// The recorded turns — read for [`session_usage`], never serialized
    /// (the list/detail wire shape is unchanged; `/api/sessions/:id/runs`
    /// serves the turns). Parsed leniently: an unexpected shape reads as no
    /// turns, never as a session that fails to load.
    #[serde(default, skip_serializing, deserialize_with = "lenient_runs")]
    runs: Vec<SessionRunEntry>,
}

/// `runs` as recorded turns, or none when it has any other shape — usage
/// accounting must never turn a readable session into a parse error.
fn lenient_runs<'de, D>(d: D) -> Result<Vec<SessionRunEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
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

/// Price a remote session-detail body locally, but only when the transport
/// did not already price it.
///
/// An HTTP remote's `/api/sessions/:id` returns `usage` computed with *its*
/// pricing config, and we do not second-guess that. The SSH connector
/// deliberately carries no pricing at all (see `SshHostConnector::new`), so
/// its body arrives without `usage` and is priced here from the token counts
/// the remote reported — the same computation, and the same
/// [`PricingConfig`], the local branch uses.
///
/// [`PricingConfig`]: rupu_config::PricingConfig
fn ensure_usage_block(detail: &mut serde_json::Value, pricing: &rupu_config::PricingConfig) {
    let Some(map) = detail.as_object_mut() else {
        return;
    };
    if map.contains_key("usage") {
        return;
    }
    let dto: SessionDto = match serde_json::from_value(serde_json::Value::Object(map.clone())) {
        Ok(d) => d,
        // Every SessionDto field is `#[serde(default)]`, so this only fires
        // on a body that isn't an object at all — nothing to price.
        Err(_) => return,
    };
    // The remote's transcripts are not on this machine: price the token
    // totals it reported.
    if let Ok(u) = serde_json::to_value(session_usage_from_totals(&dto, pricing)) {
        map.insert("usage".to_string(), u);
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

/// A local session's recorded turns as labelled transcript paths (label =
/// the turn's run id): each turn's own transcript plus its recursive
/// dispatch sub-runs, for every turn whose transcript exists. The turns
/// whose transcript is gone are returned separately.
fn session_turn_transcripts<'a>(
    runs: &'a [SessionRunEntry],
    run_store: &rupu_orchestrator::runs::RunStore,
) -> (Vec<(String, std::path::PathBuf)>, Vec<&'a SessionRunEntry>) {
    let mut labeled = Vec::new();
    let mut missing = Vec::new();
    for r in runs {
        match r.transcript_path.as_deref().filter(|t| !t.is_empty()) {
            Some(tp) if std::path::Path::new(tp).is_file() => {
                labeled.extend(crate::usage_sources::with_dispatch_children(
                    run_store,
                    &r.run_id,
                    std::path::Path::new(tp),
                ))
            }
            _ => missing.push(r),
        }
    }
    (labeled, missing)
}

/// Token + cost summary for a local session: the live fold of every turn's
/// transcript plus its dispatch sub-runs ([`crate::usage::transcripts_usage`]),
/// so a turn still in flight counts as it streams — `session.json`'s own
/// totals only move when a turn ends. A turn whose transcript is gone (e.g.
/// archived) contributes the totals recorded for that turn, priced at the
/// session's model; a session with no recorded turns at all falls back to
/// [`session_usage_from_totals`]. `runs` stays 1 (one session).
fn session_usage(
    dto: &SessionDto,
    run_store: &rupu_orchestrator::runs::RunStore,
    pricing: &rupu_config::PricingConfig,
) -> crate::usage::UsageSummary {
    if dto.runs.is_empty() {
        return session_usage_from_totals(dto, pricing);
    }
    let (labeled, missing) = session_turn_transcripts(&dto.runs, run_store);
    let u = crate::usage::transcripts_usage(&labeled);
    let mut rows = u.rows.clone();
    rows.extend(
        missing
            .into_iter()
            .filter(|r| r.total_tokens_in + r.total_tokens_out + r.total_tokens_cached > 0)
            .map(|r| rupu_transcript::UsageRow {
                provider: dto.provider_name.clone(),
                model: dto.model.clone(),
                agent: dto.agent_name.clone(),
                input_tokens: r.total_tokens_in,
                output_tokens: r.total_tokens_out,
                cached_tokens: r.total_tokens_cached,
                runs: 1,
                ..rupu_transcript::UsageRow::default()
            }),
    );
    let mut summary = crate::usage::summarize(&rows, pricing);
    summary.partial = u.partial;
    summary.runs = 1;
    summary
}

/// Token + cost summary from a session's own token totals, priced at its
/// model: a remote session (its transcripts are not here) or one with no
/// recorded turns.
fn session_usage_from_totals(
    dto: &SessionDto,
    pricing: &rupu_config::PricingConfig,
) -> crate::usage::UsageSummary {
    let total_tokens = dto.total_tokens_in + dto.total_tokens_out;
    let cost_usd =
        rupu_config::pricing::lookup(pricing, &dto.provider_name, &dto.model, &dto.agent_name).map(
            |p| {
                p.cost_usd(
                    dto.total_tokens_in,
                    dto.total_tokens_out,
                    dto.total_tokens_cached,
                    // A session's own totals carry no cache-write count.
                    0,
                )
            },
        );
    crate::usage::UsageSummary {
        input_tokens: dto.total_tokens_in,
        output_tokens: dto.total_tokens_out,
        cached_tokens: dto.total_tokens_cached,
        // A session's own totals carry no cache-write count.
        cache_write_tokens: 0,
        total_tokens,
        priced: cost_usd.is_some(),
        cost_usd,
        runs: 1,
        partial: false,
    }
}

/// Which sessions a scan keeps, and whether it prices them.
#[derive(Clone, Copy)]
pub(crate) struct SessionScan<'a> {
    /// `Some` → each session gets a `usage` block ([`session_usage`], which
    /// folds its turns' transcripts); `None` → no usage is computed at all,
    /// for callers that only count or list sessions.
    pub(crate) pricing: Option<&'a rupu_config::PricingConfig>,
    /// `Some(w)` → only sessions whose `workspace_id == w`, filtered BEFORE
    /// any usage is folded.
    pub(crate) workspace: Option<&'a str>,
}

/// Scan `<root>` for `<id>/session.json` entries. Assigns `scope` to
/// each successfully parsed session and pushes it onto `out`.
fn scan_session_dir(
    root: &std::path::Path,
    scope: &str,
    run_store: &rupu_orchestrator::runs::RunStore,
    scan: SessionScan<'_>,
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
        let Some(dto) = try_load_session(&dir) else {
            continue;
        };
        if scan.workspace.is_some_and(|w| dto.workspace_id != w) {
            continue;
        }
        let usage = scan
            .pricing
            .map(|pricing| session_usage(&dto, run_store, pricing));
        match serde_json::to_value(&dto) {
            Ok(mut val) => {
                if let serde_json::Value::Object(ref mut map) = val {
                    map.insert(
                        "scope".to_string(),
                        serde_json::Value::String(scope.to_string()),
                    );
                    if let Some(Ok(u)) = usage.map(|u| serde_json::to_value(&u)) {
                        map.insert("usage".to_string(), u);
                    }
                }
                crate::codename::inject_codename(&mut val, &dto.session_id, Some(&dto.agent_name));
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

/// Collect all sessions from both active and archive dirs, each priced.
/// Each entry has an injected `"scope"` key (`"active"` or `"archived"`).
/// Blocking IO (every session's turn transcripts are folded) — call from
/// `spawn_blocking`.
pub(crate) fn collect_sessions(
    global_dir: &std::path::Path,
    pricing: &rupu_config::PricingConfig,
) -> Vec<serde_json::Value> {
    collect_sessions_with(
        global_dir,
        SessionScan {
            pricing: Some(pricing),
            workspace: None,
        },
    )
}

/// [`collect_sessions`] with an explicit [`SessionScan`]: a workspace
/// filter and/or no pricing. Blocking IO — call from `spawn_blocking`.
pub(crate) fn collect_sessions_with(
    global_dir: &std::path::Path,
    scan: SessionScan<'_>,
) -> Vec<serde_json::Value> {
    // Session turns' dispatch sub-runs live in the global run store.
    let run_store = rupu_orchestrator::runs::RunStore::new(global_dir.join("runs"));
    let mut sessions = Vec::new();
    scan_session_dir(
        &global_dir.join("sessions"),
        "active",
        &run_store,
        scan,
        &mut sessions,
    );
    scan_session_dir(
        &global_dir.join("sessions-archive"),
        "archived",
        &run_store,
        scan,
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
    /// Optional RFC-3339 date-range bounds on `created_at` (perf &
    /// interaction arc, Plan 5 Task 5) — see
    /// `crate::pagination::DateRangeQuery`'s doc comment for the
    /// closed-boundary / lenient-parse contract.
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
}

impl SessionsQuery {
    fn range(&self) -> crate::pagination::DateRangeQuery {
        crate::pagination::DateRangeQuery {
            since: self.since.clone(),
            until: self.until.clone(),
        }
    }
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
        // Structured session listing — works for SSH hosts (which can't serve
        // the generic `proxy_get_json` GET) by shelling `rupu session list`.
        let mut rows = conn
            .list_sessions(q.scope.as_deref())
            .await
            .map_err(crate::api::runs::host_list_error)?;
        // Newest-first before paging: the connector returns the host's whole
        // list in CLI / mirror order, and the web's per-host merge relies on
        // each host's pages arriving ordered by `updated_at`.
        sort_values_newest_first(&mut rows, "updated_at");
        let page = crate::pagination::PageQuery {
            offset: q.offset,
            limit: q.limit,
        };
        return Ok(Json(
            crate::pagination::paginate(rows, &page)
                .into_iter()
                .map(|mut row| {
                    row["host_id"] = serde_json::json!(host);
                    crate::codename::inject_codename_row(
                        &mut row,
                        "session_id",
                        Some("agent_name"),
                    );
                    row
                })
                .collect(),
        ));
    }

    // ── Collect local sessions ─────────────────────────────────────────────────
    // Blocking IO (every session's turn transcripts are folded for usage).
    let local_sessions = {
        let global = s.global_dir.clone();
        let pricing = s.pricing.clone();
        tokio::task::spawn_blocking(move || collect_sessions(&global, &pricing))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
    };

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
        let range = q.range();
        sessions.retain(|v| range.contains_str(v.get("created_at").and_then(|x| x.as_str())));
        // `collect_sessions` walks the session dirs in `read_dir` order; page
        // newest-first by `updated_at`, the order the fan-out path uses and the
        // web's per-host merge relies on.
        sort_values_newest_first(&mut sessions, "updated_at");
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

    let mut all_values = fan_out_sessions(&s.hosts, q.scope.as_deref(), local_values).await;

    // Sort newest-first by updated_at (most recently active sessions first).
    sort_values_newest_first(&mut all_values, "updated_at");

    // Scope filter after merge.
    if let Some(scope) = q.scope.as_deref() {
        all_values.retain(|v| v.get("scope").and_then(|x| x.as_str()) == Some(scope));
    }

    // Date-range filter after merge, before paginate — same ordering as the
    // local-only branch above.
    let range = q.range();
    all_values.retain(|v| range.contains_str(v.get("created_at").and_then(|x| x.as_str())));

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
        let mut detail = conn.get_session(&id).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            other => ApiError::internal(other.to_string()),
        })?;
        ensure_usage_block(&mut detail, &s.pricing);
        crate::codename::inject_codename_row(&mut detail, "session_id", Some("agent_name"));
        return Ok(Json(detail));
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

    // Folding the turns' transcripts is blocking IO.
    let usage = {
        let dto = dto.clone();
        let store = std::sync::Arc::clone(&s.run_store);
        let pricing = s.pricing.clone();
        tokio::task::spawn_blocking(move || session_usage(&dto, &store, &pricing))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
    };
    let mut val = serde_json::to_value(&dto).map_err(|e| ApiError::internal(e.to_string()))?;
    if let serde_json::Value::Object(ref mut map) = val {
        map.insert(
            "scope".to_string(),
            serde_json::Value::String(scope.to_string()),
        );
        if let Ok(u) = serde_json::to_value(&usage) {
            map.insert("usage".to_string(), u);
        }
    }
    crate::codename::inject_codename(&mut val, &dto.session_id, Some(&dto.agent_name));
    Ok(Json(val))
}

/// Minimal projection of `session.json` for the usage-timeline endpoint:
/// just the `runs` array, each carrying its run id + transcript path.
#[derive(Deserialize)]
struct SessionRunsEnvelope {
    #[serde(default)]
    runs: Vec<SessionRunEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct SessionRunEntry {
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
    /// The turn's own recorded totals (written when it ends) — the fallback
    /// when its transcript is gone.
    #[serde(default)]
    total_tokens_in: u64,
    #[serde(default)]
    total_tokens_out: u64,
    #[serde(default)]
    total_tokens_cached: u64,
}

/// `GET /api/sessions/:id/usage-timeline[?host=<id>]` — ordered per-turn token
/// series across every run the session recorded (in order, each with its
/// dispatch sub-runs), labeled by run id — live, from the usage fold.
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
            .session_usage_timeline(&id)
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
    let (labeled, _) = session_turn_transcripts(&env.runs, &s.run_store);
    let u = crate::usage::transcripts_usage_blocking(labeled).await;
    let v = serde_json::to_value(&u.points).map_err(|e| ApiError::internal(e.to_string()))?;
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
        let runs = conn.session_runs(&id).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(runs));
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
        return Ok(Json(
            serde_json::json!({ "run_id": run_id, "host_id": host }),
        ));
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
        Ok(run_id) => Ok(Json(
            serde_json::json!({ "run_id": run_id, "host_id": "local" }),
        )),
        Err(crate::session_sender::SendError::Invalid(m)) => Err(ApiError::bad_request(m)),
        Err(crate::session_sender::SendError::Spawn(m)) => Err(ApiError::internal(m)),
    }
}

// ---------------------------------------------------------------------------
// Session archive / restore / delete — /api/sessions/:id/{archive,restore}
// and DELETE /api/sessions/:id
// ---------------------------------------------------------------------------

async fn mutate_session(
    s: &AppState,
    id: &str,
    action: crate::session_mutator::SessionAction,
) -> ApiResult<Json<serde_json::Value>> {
    use crate::session_mutator::SessionMutateError as E;
    let m = s.session_mutator.clone().ok_or_else(|| {
        ApiError::not_available("session archive/delete requires `rupu cp serve`")
    })?;
    m.mutate(id, action).await.map_err(|e| match e {
        E::NotFound(_) => ApiError::not_found(format!("session {id} not found")),
        E::Invalid(msg) => ApiError::conflict(msg),
        E::Failed { message, .. } => ApiError::internal(message),
    })?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id })))
}

/// Map a [`HostConnectorError`] from a proxied session archive/restore/
/// delete call to an [`ApiError`], mirroring the local branch's
/// [`crate::session_mutator::SessionMutateError`] mapping in [`mutate_session`]
/// (`NotFound` → 404, `Invalid`-shaped → 409) plus `Unsupported` → 501 for a
/// transport that genuinely can't do this (never a silent no-op).
fn map_host_session_mutate_err(e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Invalid(m) => ApiError::conflict(m),
        // A remote CP-of-CP hop (HttpHostConnector) already mapped ITS OWN
        // local refusal to 409 before it reached us — preserve that status
        // rather than flattening it into a 500 below.
        HostConnectorError::Remote(409, m) => ApiError::conflict(m),
        HostConnectorError::Unsupported(m) => ApiError::not_available(m),
        other => ApiError::internal(other.to_string()),
    }
}

/// `POST /api/sessions/:id/archive[?host=<id>]`.
///
/// Without `?host=` (or `?host=local`): unchanged — dispatches through the
/// local [`crate::session_mutator::SessionMutator`] port (`mutate_session`).
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::archive_session`]
/// and returns `{ "ok": true, "id", "host_id": "<id>" }`.
async fn archive_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    crate::api::runs::validate_id(&id)?;
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        conn.archive_session(&id)
            .await
            .map_err(map_host_session_mutate_err)?;
        return Ok(Json(serde_json::json!({ "ok": true, "id": id, "host_id": host })));
    }
    mutate_session(&s, &id, crate::session_mutator::SessionAction::Archive).await
}

/// `POST /api/sessions/:id/restore[?host=<id>]`. See [`archive_session`]'s doc.
async fn restore_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    crate::api::runs::validate_id(&id)?;
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        conn.restore_session(&id)
            .await
            .map_err(map_host_session_mutate_err)?;
        return Ok(Json(serde_json::json!({ "ok": true, "id": id, "host_id": host })));
    }
    mutate_session(&s, &id, crate::session_mutator::SessionAction::Restore).await
}

/// `DELETE /api/sessions/:id[?host=<id>]`. See [`archive_session`]'s doc.
async fn delete_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    crate::api::runs::validate_id(&id)?;
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        conn.delete_session(&id)
            .await
            .map_err(map_host_session_mutate_err)?;
        return Ok(Json(serde_json::json!({ "ok": true, "id": id, "host_id": host })));
    }
    mutate_session(&s, &id, crate::session_mutator::SessionAction::Delete).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn single_remote_host_session_list_reports_unsupported_as_501() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = crate::api::runs::tests::state_with_fake_host(&tmp, serde_json::json!({}));
        let err = list_sessions(
            State(s),
            Query(SessionsQuery {
                offset: None,
                limit: None,
                scope: None,
                host: Some("host_fake".into()),
                since: None,
                until: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    /// SSH bodies arrive unpriced (that connector has no pricing config);
    /// the CP prices them from the reported token counts.
    #[test]
    fn ensure_usage_block_prices_a_body_that_arrived_without_usage() {
        let mut detail = serde_json::json!({
            "session_id": "ses_1",
            "agent_name": "scout",
            "provider_name": "anthropic",
            "model": "opus",
            "total_tokens_in": 10,
            "total_tokens_out": 20,
        });

        ensure_usage_block(&mut detail, &rupu_config::PricingConfig::default());

        let usage = detail.get("usage").expect("unpriced body must be priced");
        assert_eq!(usage["total_tokens"], 30, "priced from in + out");
    }

    /// An HTTP remote already priced its own session with its own config.
    /// Re-pricing it here would silently overwrite that with ours.
    #[test]
    fn ensure_usage_block_leaves_an_already_priced_body_alone() {
        let mut detail = serde_json::json!({
            "session_id": "ses_1",
            "total_tokens_in": 10,
            "total_tokens_out": 20,
            "usage": { "total_tokens": 999, "cost_usd": 1.5 },
        });

        ensure_usage_block(&mut detail, &rupu_config::PricingConfig::default());

        assert_eq!(
            detail["usage"]["total_tokens"], 999,
            "the remote's own usage block must survive untouched"
        );
    }

    #[test]
    fn session_rows_codename_stored_else_derived() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        for (id, codename) in [
            ("ses_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None),
            ("ses_named", Some("cobalt-harbor/heron")),
        ] {
            let dir = root.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            let mut j = serde_json::json!({"session_id": id, "agent_name": "triage"});
            if let Some(c) = codename {
                j["codename"] = c.into();
            }
            std::fs::write(dir.join("session.json"), j.to_string()).unwrap();
        }
        let rows = collect_sessions(tmp.path(), &rupu_config::PricingConfig::default());
        let by = |id: &str| {
            rows.iter()
                .find(|r| r["session_id"] == id)
                .cloned()
                .unwrap()
        };
        let legacy = by("ses_01J9ZQ3K4M5N6P7Q8R9S0T1V2W");
        assert_eq!(
            legacy["codename"],
            rupu_codename::derive_legacy("ses_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", Some("triage"))
        );
        assert_eq!(legacy["codename_derived"], true);
        let named = by("ses_named");
        assert_eq!(named["codename"], "cobalt-harbor/heron");
        assert_eq!(named["codename_derived"], false);
    }

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
            codename: None,
            runs: Vec::new(),
        };
        let u = session_usage_from_totals(&dto, &rupu_config::PricingConfig::default());
        assert_eq!(u.input_tokens, 1_000_000);
        assert!(u.priced);
        assert!((u.cost_usd.unwrap() - 3.0).abs() < 1e-9);
    }

    /// A `runs` field of an unexpected shape never fails the session parse
    /// (it would turn a readable session into a 500, or drop a remote
    /// body's usage block).
    #[test]
    fn an_odd_runs_shape_reads_as_no_turns() {
        let dto: SessionDto =
            serde_json::from_str(r#"{"session_id":"s1","runs":3}"#).expect("parses");
        assert!(dto.runs.is_empty());
        let dto: SessionDto =
            serde_json::from_str(r#"{"session_id":"s1","runs":[{"run_id":7}]}"#).expect("parses");
        assert!(dto.runs.is_empty());
    }

    /// A live turn counts from its transcript; a turn whose transcript is
    /// gone still counts its own recorded totals — never silently zero.
    #[test]
    fn session_usage_folds_live_turns_and_keeps_recorded_totals_of_missing_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().join("run_live.jsonl");
        let mut body = Vec::new();
        for ev in [
            rupu_transcript::Event::RunStart {
                codename: None,
                run_id: "run_live".into(),
                workspace_id: "w".into(),
                agent: "a".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                started_at: chrono::Utc::now(),
                mode: rupu_transcript::RunMode::Ask,
                schema: None,
                system_prompt: None,
            },
            rupu_transcript::Event::Usage {
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                served_model: None,
                input_tokens: 70,
                output_tokens: 7,
                cached_tokens: 0,
                cache_write_tokens: 0,
                purpose: None,
            },
        ] {
            body.extend(serde_json::to_vec(&ev).unwrap());
            body.push(b'\n');
        }
        std::fs::write(&live, body).unwrap();
        let entry = |run_id: &str, tp: &std::path::Path, input: u64| SessionRunEntry {
            run_id: run_id.into(),
            transcript_path: Some(tp.to_string_lossy().into_owned()),
            total_tokens_in: input,
            total_tokens_out: 0,
            total_tokens_cached: 0,
        };
        let dto = SessionDto {
            codename: None,
            session_id: "s1".into(),
            agent_name: "a".into(),
            model: "claude-sonnet-4-6".into(),
            provider_name: "anthropic".into(),
            status: serde_json::Value::Null,
            total_turns: 2,
            // Stale: only the finished turn is in the session totals.
            total_tokens_in: 900,
            total_tokens_out: 0,
            total_tokens_cached: 0,
            created_at: String::new(),
            updated_at: String::new(),
            active_run_id: Some("run_live".into()),
            last_error: None,
            target: None,
            workspace_id: "w".into(),
            runs: vec![
                entry("run_gone", &tmp.path().join("archived.jsonl"), 900),
                entry("run_live", &live, 0),
            ],
        };
        let store = rupu_orchestrator::runs::RunStore::new(tmp.path().join("runs"));
        let u = session_usage(&dto, &store, &rupu_config::PricingConfig::default());
        assert_eq!(u.input_tokens, 970);
        assert_eq!(u.output_tokens, 7);
        assert_eq!(u.runs, 1);
        assert!(!u.partial);
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
        assert_eq!(
            rows[0].completed_at.as_deref(),
            Some("2026-06-26T00:01:00Z")
        );
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

    use crate::session_mutator::{SessionAction, SessionMutateError, SessionMutator};

    struct StubMutator;
    #[async_trait::async_trait]
    impl SessionMutator for StubMutator {
        async fn mutate(&self, id: &str, action: SessionAction) -> Result<(), SessionMutateError> {
            if id == "missing" {
                return Err(SessionMutateError::NotFound(id.into()));
            }
            if action == SessionAction::Archive && id == "active-running" {
                return Err(SessionMutateError::Invalid("session is running".into()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn archive_session_ok_and_not_found_and_409() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default())
            .with_session_mutator(Some(std::sync::Arc::new(StubMutator)));
        let _ = archive_session(
            State(state.clone()),
            Path("s1".to_string()),
            Query(SessionHostQuery::default()),
        )
        .await
        .expect("ok");
        let nf = archive_session(
            State(state.clone()),
            Path("missing".to_string()),
            Query(SessionHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(nf.0, axum::http::StatusCode::NOT_FOUND);
        let conflict = archive_session(
            State(state.clone()),
            Path("active-running".to_string()),
            Query(SessionHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(conflict.0, axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn archive_session_without_adapter_is_501() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default()); // no mutator
        let err = archive_session(
            State(state),
            Path("s1".to_string()),
            Query(SessionHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    /// Traversal ids must be rejected with 400 BEFORE the mutator is reached.
    /// The no-mutator state (→ 501) is never hit when the id is invalid, which
    /// proves the guard fires first.
    #[tokio::test]
    async fn archive_session_traversal_id_is_bad_request() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let err = archive_session(
            State(state),
            Path("../../etc".to_string()),
            Query(SessionHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn restore_session_traversal_id_is_bad_request() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let err = restore_session(
            State(state),
            Path("../../etc".to_string()),
            Query(SessionHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_session_traversal_id_is_bad_request() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let err = delete_session(
            State(state),
            Path("../../etc".to_string()),
            Query(SessionHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn archive_session_absent_host_matches_explicit_host_local() {
        // Back-compat proof, mirroring the runs.rs equivalent: an absent
        // `?host=` and an explicit `?host=local` must both dispatch through
        // the local `SessionMutator` port and produce the same response
        // shape (no `host_id` key — unlike the remote branch).
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default())
            .with_session_mutator(Some(std::sync::Arc::new(StubMutator)));

        let absent = archive_session(
            State(state.clone()),
            Path("s1".to_string()),
            Query(SessionHostQuery::default()),
        )
        .await
        .expect("absent host should archive locally")
        .0;
        let explicit_local = archive_session(
            State(state.clone()),
            Path("s1".to_string()),
            Query(SessionHostQuery {
                host: Some("local".into()),
            }),
        )
        .await
        .expect("host=local should archive locally")
        .0;

        assert_eq!(absent, serde_json::json!({ "ok": true, "id": "s1" }));
        assert_eq!(explicit_local, serde_json::json!({ "ok": true, "id": "s1" }));
    }

    #[tokio::test]
    async fn archive_session_unknown_host_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let err = archive_session(
            State(state),
            Path("s1".to_string()),
            Query(SessionHostQuery {
                host: Some("host_nonexistent".into()),
            }),
        )
        .await
        .unwrap_err();
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

    // Minor finding: a remote CP-of-CP hop's own 409 refusal must surface as
    // 409 here too, not fall through to the generic 500 `other` arm.
    #[test]
    fn map_host_session_mutate_err_preserves_remote_409_as_conflict() {
        let err =
            map_host_session_mutate_err(HostConnectorError::Remote(409, "still running".into()));
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    // ── Date-range filtering (perf & interaction arc, Plan 5 Task 5) ─────────

    fn write_session_with_created_at(root: &std::path::Path, session_id: &str, created_at: &str) {
        let dir = root.join("sessions").join(session_id);
        std::fs::create_dir_all(&dir).unwrap();
        let session = serde_json::json!({
            "session_id": session_id,
            "agent_name": "agent",
            "created_at": created_at,
            "updated_at": created_at,
        });
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_string(&session).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn list_sessions_since_until_narrows_before_pagination() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_session_with_created_at(tmp.path(), "sess_early", "2026-08-01T12:00:00Z");
        write_session_with_created_at(tmp.path(), "sess_mid", "2026-08-10T12:00:00Z");
        write_session_with_created_at(tmp.path(), "sess_late", "2026-08-20T12:00:00Z");

        let s = crate::state::AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );

        let Json(rows) = list_sessions(
            State(s),
            Query(SessionsQuery {
                offset: None,
                limit: None,
                scope: None,
                host: Some("local".into()),
                since: Some("2026-08-05T00:00:00Z".into()),
                until: Some("2026-08-15T00:00:00Z".into()),
            }),
        )
        .await
        .expect("ok");

        assert_eq!(rows.len(), 1, "only sess_mid falls in [Aug 5, Aug 15]");
        assert_eq!(rows[0]["session_id"], serde_json::json!("sess_mid"));
    }

    // ── Per-host time order (the web's per-host merge relies on it) ──────────

    fn write_session_updated_at(root: &std::path::Path, session_id: &str, updated_at: &str) {
        let dir = root.join("sessions").join(session_id);
        std::fs::create_dir_all(&dir).unwrap();
        let session = serde_json::json!({
            "session_id": session_id,
            "agent_name": "agent",
            "created_at": "2026-08-01T00:00:00Z",
            "updated_at": updated_at,
        });
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_string(&session).unwrap(),
        )
        .unwrap();
    }

    fn session_ids(rows: &[serde_json::Value]) -> Vec<&str> {
        rows.iter()
            .map(|r| r["session_id"].as_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn local_session_list_pages_newest_first_by_updated_at() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Names and times deliberately disagree, so `read_dir` order (by name
        // or by anything else) is not the time order.
        for (id, at) in [
            ("sess_a", "2026-08-03T00:00:00Z"),
            ("sess_b", "2026-08-01T00:00:00Z"),
            ("sess_c", "2026-08-05T00:00:00Z"),
            ("sess_d", "2026-08-02T00:00:00Z"),
            ("sess_e", "2026-08-04T00:00:00Z"),
        ] {
            write_session_updated_at(tmp.path(), id, at);
        }
        let s = crate::state::AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        let page = |offset, limit| {
            list_sessions(
                State(s.clone()),
                Query(SessionsQuery {
                    offset: Some(offset),
                    limit: Some(limit),
                    scope: None,
                    host: Some("local".into()),
                    since: None,
                    until: None,
                }),
            )
        };
        let Json(all) = page(0, 20).await.expect("ok");
        assert_eq!(
            session_ids(&all),
            ["sess_c", "sess_e", "sess_a", "sess_d", "sess_b"]
        );
        // Sorted BEFORE paging: page 0 is the newest two, page 1 the next.
        let Json(p0) = page(0, 2).await.expect("ok");
        assert_eq!(session_ids(&p0), ["sess_c", "sess_e"]);
        let Json(p1) = page(2, 2).await.expect("ok");
        assert_eq!(session_ids(&p1), ["sess_a", "sess_d"]);
    }

    #[tokio::test]
    async fn single_remote_session_list_pages_newest_first_by_updated_at() {
        let tmp = tempfile::TempDir::new().unwrap();
        // The connector hands back the host's whole list unsorted.
        let s = crate::api::runs::tests::state_with_fake_host(
            &tmp,
            serde_json::json!({ "sessions": [
                {"session_id": "sess_a", "agent_name": "x", "updated_at": "2026-08-03T00:00:00Z"},
                {"session_id": "sess_b", "agent_name": "x", "updated_at": "2026-08-01T00:00:00Z"},
                {"session_id": "sess_c", "agent_name": "x", "updated_at": "2026-08-05T00:00:00Z"},
                {"session_id": "sess_d", "agent_name": "x", "updated_at": "2026-08-02T00:00:00Z"},
            ]}),
        );
        let page = |offset, limit| {
            list_sessions(
                State(s.clone()),
                Query(SessionsQuery {
                    offset: Some(offset),
                    limit: Some(limit),
                    scope: None,
                    host: Some("host_fake".into()),
                    since: None,
                    until: None,
                }),
            )
        };
        let Json(p0) = page(0, 2).await.expect("ok");
        assert_eq!(session_ids(&p0), ["sess_c", "sess_a"]);
        let Json(p1) = page(2, 2).await.expect("ok");
        assert_eq!(session_ids(&p1), ["sess_d", "sess_b"]);
        assert!(p0.iter().all(|r| r["host_id"] == "host_fake"));
    }

    #[tokio::test]
    async fn list_sessions_bad_until_degrades_to_unfiltered_not_500() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_session_with_created_at(tmp.path(), "sess_a", "2026-08-01T12:00:00Z");

        let s = crate::state::AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );

        let Json(rows) = list_sessions(
            State(s),
            Query(SessionsQuery {
                offset: None,
                limit: None,
                scope: None,
                host: Some("local".into()),
                since: None,
                until: Some("not-a-timestamp".into()),
            }),
        )
        .await
        .expect("a malformed until must degrade, never error");
        assert_eq!(rows.len(), 1);
    }
}
