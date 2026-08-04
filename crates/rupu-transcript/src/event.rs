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
    /// Per-catalog-tool-call audit trail (step `actions:` enforcement,
    /// 2026-07-26 design). Emitted from two choke points: the agent
    /// runtime's `on_tool_call` hook (wrapped in
    /// `rupu-orchestrator::step_factory`) for agent-driven MCP calls, and
    /// `execute_action_step` for `action:`-node calls. NEVER confuse this
    /// with `ActionEmitted` (the older, `action:`-step-only envelope) —
    /// `ToolAudit` is the general catalog-call audit line and is emitted
    /// *alongside* `ActionEmitted` for action steps, not instead of it.
    ToolAudit {
        /// The MCP catalog tool name (e.g. `issues.create`).
        tool: String,
        /// Whether `tool` appears in the step's `actions:` allowlist.
        /// `false` both when `actions:` is empty/absent (the step is
        /// unrestricted — NOT a violation) and when `actions:` is
        /// non-empty but doesn't name this tool. Use `restricted` to
        /// tell those two apart.
        declared: bool,
        /// Whether `tool` is covered by the agent's `tools:` grant
        /// (wildcard-expanded, evaluated BEFORE any `actions:`
        /// narrowing — see `rupu-orchestrator::step_factory::
        /// narrow_agent_tools`). For an `action:` node (no agent
        /// grant concept applies) this is always `true`.
        granted: bool,
        /// Whether the call was actually denied — either narrowed out
        /// of the agent's roster (never reached the registry) or
        /// denied by the MCP mode/permission gate.
        blocked: bool,
        /// Whether the step declared a non-empty `actions:` allowlist
        /// at all (disambiguates `declared: false`; see its doc).
        restricted: bool,
    },
    /// One outbound network flow attributed to this run.
    ///
    /// Phase 1 covers rupu's OWN egress — provider APIs, SCM connectors,
    /// MCP, webhooks. It does NOT cover the agent's `bash` subprocess
    /// traffic; `flow.fidelity` states what is actually known. See
    /// docs/superpowers/specs/2026-08-03-rupu-netflow-observability-design.md
    ///
    /// Boxed: `FlowRecord` dwarfs every other variant and clippy's
    /// `large_enum_variant` denies otherwise.
    NetFlow {
        flow: Box<rupu_netflow::FlowRecord>,
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
    fn tool_audit_roundtrips_with_the_adjacently_tagged_shape() {
        // The CP web reads this as `{"type":"tool_audit","data":{...}}` —
        // the SAME tag="type"/content="data" envelope every other Event
        // variant gets (see `rupu-cp/web/src/lib/transcript.ts`'s doc
        // comment). Assert the wire shape explicitly so this never
        // regresses to a flat `{"type":...,"tool":...}` shape.
        let e = Event::ToolAudit {
            tool: "issues.create".into(),
            declared: false,
            granted: true,
            blocked: false,
            restricted: false,
        };
        let s = serde_json::to_string(&e).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "tool_audit");
        assert_eq!(v["data"]["tool"], "issues.create");
        assert_eq!(v["data"]["granted"], true);
        assert_eq!(v["data"]["blocked"], false);

        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back, e);
    }

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

    #[test]
    fn netflow_event_round_trips() {
        use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, Origin, Outcome};

        let flow = FlowRecord {
            id: FlowId::from_parts(5, 5),
            ts: chrono::Utc::now(),
            ctx: FlowCtx {
                run_id: Some("run-9".into()),
                step_id: Some("step-1".into()),
                agent: Some("reviewer".into()),
                workspace_id: Some("ws".into()),
                origin: Origin::Provider("anthropic".into()),
            },
            fidelity: Fidelity::Http,
            method: "POST".into(),
            scheme: "https".into(),
            host: "api.anthropic.com".into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: Some("HTTP/1.1".into()),
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: Some(10),
            bytes_in: Some(20),
            body_complete: true,
            ttfb_ms: Some(5),
            duration_ms: Some(50),
        };

        let event = Event::NetFlow {
            flow: Box::new(flow),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"net_flow""#));

        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn legacy_transcripts_without_netflow_still_parse() {
        // A transcript written before this variant existed must still read.
        let line = r#"{"type":"turn_start","data":{"turn_idx":0}}"#;
        let back: Event = serde_json::from_str(line).unwrap();
        assert!(matches!(back, Event::TurnStart { turn_idx: 0 }));
    }
}
