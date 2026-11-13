//! Read-only lead-context measurement. This deliberately does not use the
//! session accounting timeline: that series also contains dispatched children.
use super::sessions::SessionHostQuery;
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use rupu_runtime::services::session_compaction::{Compaction, CompactionResult, Outcome};
use rupu_runtime::{
    session_settings::SessionSettings,
    sessions::{parse_stored_session, SessionRecord},
};
use rupu_transcript::Event;
use serde::Serialize;
use std::{fs, io::Read, path::Path as FsPath};

pub const FEATURE: &str = "session.context.v1";

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/sessions/:id/context", get(context))
}

#[derive(Debug, Serialize)]
pub struct ContextSnapshot {
    pub schema_version: u8,
    pub session_id: String,
    pub host_id: &'static str,
    pub input_tokens: Option<u32>,
    pub context_window_tokens: Option<u32>,
    pub context_window_source: Option<String>,
    pub context_window_source_detail: Option<serde_json::Value>,
    pub compact_at_percent: Option<u8>,
    pub compact_threshold_tokens: Option<u64>,
    pub output_headroom_tokens: Option<u32>,
    pub output_headroom_source: Option<serde_json::Value>,
    pub freshness: &'static str,
    pub compacting: bool,
    pub run_id: Option<String>,
    pub call_id: Option<String>,
    pub transcript_turn: Option<u32>,
    pub usage_index: Option<u64>,
    pub observed_at: Option<String>,
    pub basis: &'static str,
}

async fn context(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SessionHostQuery>,
) -> ApiResult<Json<ContextSnapshot>> {
    super::runs::validate_id(&id)?;
    super::fleet::require_local(q.host.as_deref(), "session context snapshot")?;
    super::runs::blocking(move || read_snapshot(&s.global_dir, &id))
        .await
        .map(Json)
}

fn optional_bytes(path: &FsPath) -> ApiResult<Option<Vec<u8>>> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            const MAX_BYTES: u64 = 64 * 1024 * 1024;
            file.take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| ApiError::internal(e.to_string()))?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(ApiError::internal("context source exceeds read limit"));
            }
            Ok(Some(bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::internal(e.to_string())),
    }
}

/// Public for an embedded client/read-only acceptance without starting a server.
/// No locks, stale-marker cleanup, discovery, model calls or writes are made.
pub fn read_snapshot(global: &FsPath, id: &str) -> ApiResult<ContextSnapshot> {
    super::runs::validate_id(id)?;
    let active = global.join("sessions").join(id);
    let archived = global.join("sessions-archive").join(id);
    let dir = if active.is_dir() {
        active
    } else if archived.is_dir() {
        archived
    } else {
        return Err(ApiError::not_found("session not found"));
    };
    let record_path = dir.join("session.json");
    let bytes =
        optional_bytes(&record_path)?.ok_or_else(|| ApiError::not_found("session not found"))?;
    let stored = parse_stored_session(&bytes).map_err(|e| ApiError::internal(e.to_string()))?;
    let record = stored.record();
    if record.session_id != id {
        return Err(ApiError::conflict("session record identity mismatch"));
    }
    let marker = optional_bytes(&dir.join("compaction.json"))?;
    let result = optional_bytes(&dir.join("compaction.result.json"))?;
    let settings_bytes = optional_bytes(&dir.join("settings.json"))?;
    let mut snapshot = project(record);
    let settings = settings_bytes
        .as_deref()
        .map(serde_json::from_slice::<SessionSettings>)
        .transpose()
        .map_err(|e| ApiError::internal(e.to_string()))?
        .unwrap_or_default();
    let model_changed = settings.model.as_deref().is_some_and(|m| m != record.model)
        || settings
            .provider
            .as_deref()
            .is_some_and(|p| p != record.provider_name);
    if model_changed {
        snapshot.freshness = "model_changed";
        snapshot.context_window_tokens = None;
        snapshot.context_window_source = None;
        snapshot.context_window_source_detail = None;
        snapshot.compact_threshold_tokens = None;
        snapshot.output_headroom_tokens = None;
        snapshot.output_headroom_source = None;
    }
    if let Some(raw) = marker.as_deref() {
        match serde_json::from_slice::<Compaction>(raw) {
            Ok(marker) if rupu_launch::proc::process_alive(marker.pid, marker.pid_start) => {
                snapshot.compacting = true;
                snapshot.freshness = "compacting";
            }
            Ok(_) if !model_changed => observe(record, result.as_deref(), &mut snapshot),
            Ok(_) => {}
            Err(_) => snapshot.freshness = "unavailable",
        }
    } else if !model_changed {
        observe(record, result.as_deref(), &mut snapshot);
    }
    if optional_bytes(&record_path)?.as_deref() != Some(bytes.as_slice())
        || optional_bytes(&dir.join("compaction.json"))? != marker
        || optional_bytes(&dir.join("compaction.result.json"))? != result
        || optional_bytes(&dir.join("settings.json"))? != settings_bytes
    {
        snapshot.clear_observation("changed_during_read");
    }
    Ok(snapshot)
}

impl ContextSnapshot {
    fn clear_observation(&mut self, freshness: &'static str) {
        self.input_tokens = None;
        self.transcript_turn = None;
        self.usage_index = None;
        self.freshness = freshness;
    }
}

fn project(record: &SessionRecord) -> ContextSnapshot {
    let window = record.effective_context_window().filter(|n| *n > 0);
    let source = record
        .model_limits
        .as_ref()
        .filter(|l| l.input.tokens.is_some())
        .and_then(|l| serde_json::to_value(&l.input.source).ok())
        .or_else(|| window.map(|_| serde_json::json!({"kind":"agent"})))
        .filter(|_| window.is_some());
    // Match the same stored/pinned output precedence as compaction_threshold.
    let output = match record.model_limits.as_ref() {
        Some(limits) if !limits.is_unresolved() => limits
            .output_shares_context
            .then_some(limits.output.tokens)
            .flatten(),
        stored => {
            if stored.is_none_or(|l| l.output_shares_context) {
                record.max_tokens.filter(|n| *n > 0)
            } else {
                None
            }
        }
    };
    let output = output.filter(|out| *out > 0 && window.is_some_and(|input| *out < input));
    let output_source = output.and_then(|_| {
        record
            .model_limits
            .as_ref()
            .filter(|l| !l.is_unresolved())
            .and_then(|l| serde_json::to_value(&l.output.source).ok())
            .or_else(|| Some(serde_json::json!({"kind":"agent"})))
    });
    ContextSnapshot {
        schema_version: 1,
        session_id: record.session_id.clone(),
        host_id: "local",
        input_tokens: None,
        context_window_tokens: window,
        context_window_source: source
            .as_ref()
            .and_then(|s| s["kind"].as_str().map(str::to_owned)),
        context_window_source_detail: source,
        compact_at_percent: Some(record.effective_compact_at_percent().clamp(10, 95)),
        compact_threshold_tokens: window.and_then(|_| record.compaction_threshold(None)),
        output_headroom_tokens: output,
        output_headroom_source: output_source,
        freshness: "awaiting_usage",
        compacting: false,
        run_id: lead_run_id(record).map(str::to_owned),
        call_id: None,
        transcript_turn: None,
        usage_index: None,
        observed_at: None,
        basis: "latest_recorded_lead_input",
    }
}

fn lead_run_id(record: &SessionRecord) -> Option<&str> {
    record
        .active_run_id
        .as_deref()
        .or(record.last_run_id.as_deref())
        .or_else(|| record.runs.last().map(|run| run.run_id.as_str()))
}

fn observe(record: &SessionRecord, result: Option<&[u8]>, snapshot: &mut ContextSnapshot) {
    // Never fall back from a running/newest turn to an older turn, nor scan the
    // dispatch metadata referenced by tools. Only the session's own run path.
    let Some(run_id) = lead_run_id(record) else {
        return;
    };
    snapshot.run_id = Some(run_id.to_owned());
    let own = record.runs.iter().rev().find(|r| r.run_id == run_id);
    let path = if record.active_run_id.as_deref() == Some(run_id) {
        record
            .active_transcript_path
            .as_deref()
            .or_else(|| own.map(|r| r.transcript_path.as_path()))
    } else {
        own.map(|r| r.transcript_path.as_path())
            .or(record.last_transcript_path.as_deref())
    };
    let Some(path) = path else {
        snapshot.freshness = "unavailable";
        return;
    };
    let Ok(file) = fs::File::open(path) else {
        snapshot.freshness = "unavailable";
        return;
    };
    let Ok(metadata) = file.metadata() else {
        snapshot.freshness = "unavailable";
        return;
    };
    const MAX_BYTES: u64 = 64 * 1024 * 1024;
    if metadata.len() > MAX_BYTES {
        snapshot.freshness = "unavailable";
        return;
    }
    // Take enforces the bound even if the transcript grows while being read.
    let mut bytes = Vec::new();
    if file.take(MAX_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_BYTES {
        snapshot.freshness = "unavailable";
        return;
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        snapshot.freshness = "unavailable";
        return;
    };
    let events = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<Event>);
    let completed_compaction = match result {
        Some(raw) => match serde_json::from_slice::<CompactionResult>(raw) {
            Ok(r) if r.outcome == Outcome::Compacted => {
                match DateTime::parse_from_rfc3339(&r.finished_at) {
                    Ok(at) => Some(at.with_timezone(&Utc)),
                    Err(_) => {
                        snapshot.freshness = "unavailable";
                        return;
                    }
                }
            }
            Ok(_) => None,
            Err(_) => {
                snapshot.freshness = "unavailable";
                return;
            }
        },
        None => None,
    };
    let mut owned = false;
    let mut turn = None;
    let mut usage_index = 0;
    for (index, event) in events.enumerate() {
        if index >= 250_000 {
            snapshot.clear_observation("unavailable");
            return;
        }
        let Ok(event) = event else {
            snapshot.clear_observation("unavailable");
            return;
        };
        match event {
            Event::RunStart {
                run_id: actual,
                agent,
                workspace_id,
                provider,
                model,
                started_at,
                ..
            } => {
                if owned
                    || actual != run_id
                    || agent != record.agent_name
                    || workspace_id != record.workspace_id
                    || provider != record.provider_name
                    || model != record.model
                {
                    snapshot.clear_observation("unavailable");
                    return;
                }
                if completed_compaction.is_some_and(|at| started_at <= at) {
                    snapshot.clear_observation("after_compaction");
                    return;
                }
                owned = true;
            }
            Event::TurnStart { turn_idx } if owned => turn = Some(turn_idx),
            Event::Compaction { .. } if owned => snapshot.clear_observation("after_compaction"),
            Event::Usage {
                input_tokens,
                purpose: None,
                provider,
                model,
                ..
            } if owned => {
                usage_index += 1;
                if provider != record.provider_name || model != record.model {
                    snapshot.clear_observation("model_changed");
                    continue;
                }
                if input_tokens == 0 {
                    snapshot.clear_observation("awaiting_usage");
                    continue;
                }
                snapshot.input_tokens = Some(input_tokens);
                snapshot.transcript_turn = turn;
                snapshot.usage_index = Some(usage_index);
                snapshot.freshness = "recorded";
            }
            _ => {}
        }
    }
    if !owned {
        snapshot.clear_observation("unavailable");
    }
    if fs::metadata(path)
        .map(|now| !same_file(&now, &metadata))
        .unwrap_or(true)
    {
        snapshot.clear_observation("changed_during_read");
    }
}

#[cfg(test)]
#[path = "session_context_tests.rs"]
mod tests;

fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return false;
        }
    }
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}
