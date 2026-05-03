//! Event schema for rupu transcripts. See the Slice A spec for details.
//!
//! All events are tagged JSON objects with a `type` discriminator and a
//! `data` payload. JSONL on disk is one event per line.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    RunStart {
        run_id: String,
        workspace_id: String,
        agent: String,
        provider: String,
        model: String,
        started_at: DateTime<Utc>,
        mode: RunMode,
    },
    TurnStart {
        turn_idx: u32,
    },
    AssistantMessage {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        thinking: Option<String>,
    },
    ToolCall {
        call_id: String,
        tool: String,
        input: Value,
    },
    ToolResult {
        call_id: String,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        error: Option<String>,
        duration_ms: u64,
    },
    FileEdit {
        path: String,
        kind: FileEditKind,
        diff: String,
    },
    CommandRun {
        argv: Vec<String>,
        cwd: String,
        exit_code: i32,
        stdout_bytes: u64,
        stderr_bytes: u64,
    },
    ActionEmitted {
        kind: String,
        payload: Value,
        allowed: bool,
        applied: bool,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        reason: Option<String>,
    },
    GateRequested {
        gate_id: String,
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        decision: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        decided_by: Option<String>,
    },
    TurnEnd {
        turn_idx: u32,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        tokens_in: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        tokens_out: Option<u64>,
    },
    Usage {
        provider: String,
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        #[serde(default)]
        cached_tokens: u32,
    },
    RunComplete {
        run_id: String,
        status: RunStatus,
        total_tokens: u64,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    Error,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Ask,
    Bypass,
    Readonly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileEditKind {
    Create,
    Modify,
    Delete,
}
