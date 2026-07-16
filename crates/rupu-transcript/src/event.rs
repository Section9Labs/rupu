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
    AssistantDelta {
        content: String,
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
        /// Optional structured payload emitted alongside the human/LLM-facing
        /// `output` string (e.g. ast_grep match + metavariable data). Additive
        /// and backward compatible — absent on legacy transcripts.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        structured: Option<Value>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        served_model: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_structured_roundtrips_and_is_omitted_when_none() {
        // Present:
        let e = Event::ToolResult {
            call_id: "c1".into(),
            output: "ok".into(),
            error: None,
            duration_ms: 5,
            structured: Some(serde_json::json!({"tool":"ast_grep","matchCount":2})),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"structured\""));
        let back: Event = serde_json::from_str(&s).unwrap();
        match back {
            Event::ToolResult {
                structured: Some(v),
                ..
            } => {
                assert_eq!(v["matchCount"], 2);
            }
            _ => panic!("expected ToolResult with structured=Some"),
        }

        // Absent -> omitted from JSON, and old JSON without the field still parses.
        let e2 = Event::ToolResult {
            call_id: "c2".into(),
            output: "ok".into(),
            error: None,
            duration_ms: 1,
            structured: None,
        };
        let s2 = serde_json::to_string(&e2).unwrap();
        assert!(!s2.contains("structured"));
        let legacy =
            r#"{"type":"tool_result","data":{"call_id":"c3","output":"x","duration_ms":0}}"#;
        let parsed: Event = serde_json::from_str(legacy).unwrap();
        assert!(matches!(
            parsed,
            Event::ToolResult {
                structured: None,
                ..
            }
        ));
    }
}
