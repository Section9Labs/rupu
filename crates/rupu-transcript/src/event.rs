//! Event schema for rupu transcripts. See the Slice A spec for details.
//!
//! All events are tagged JSON objects with a `type` discriminator and a
//! `data` payload. JSONL on disk is one event per line.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Tag strings (post `rename_all = "snake_case"`) for every variant except
/// [`Event::Unknown`]. Used by the hand-written [`Deserialize`] impl below
/// to recognize a known `type` before handing off to the derive-generated
/// logic — see that impl's doc comment for why this indirection exists.
/// Kept in sync by construction: every variant has a roundtrip test
/// (`event::tests` / `tests/roundtrip.rs`) that would start failing (parsing
/// as `Unknown` instead of the real variant) if its tag were missing here.
const KNOWN_EVENT_TAGS: &[&str] = &[
    "run_start",
    "turn_start",
    "assistant_delta",
    "assistant_message",
    "tool_call",
    "tool_result",
    "file_edit",
    "command_run",
    "action_emitted",
    "gate_requested",
    "turn_end",
    "usage",
    "run_complete",
    "tool_audit",
    "net_flow",
    "thinking",
    "thinking_delta",
    "user_message",
    "seed",
    "compaction",
    "notice",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    remote = "Self",
    tag = "type",
    content = "data",
    rename_all = "snake_case"
)]
pub enum Event {
    RunStart {
        run_id: String,
        workspace_id: String,
        agent: String,
        provider: String,
        model: String,
        started_at: DateTime<Utc>,
        mode: RunMode,
        /// Transcript schema version. `None` = v1 (pre-2026-08-31 writer).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        schema: Option<u32>,
        /// The effective system prompt actually sent (post coverage-section
        /// append). `None` on legacy transcripts.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        system_prompt: Option<String>,
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
        /// Provider-reported stop reason for the turn (snake_case of
        /// `rupu_providers::StopReason`), when known.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        stop_reason: Option<String>,
        /// Provider response id for the turn, when non-empty.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        response_id: Option<String>,
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
    /// One model reasoning block, in its true position within the turn's
    /// block order. `raw` is the producing provider's byte-exact block
    /// (signature payloads included) — persisted for replay, never for
    /// display. `text: None` means redacted / display-omitted reasoning:
    /// the block existed, its content is not human-readable.
    Thinking {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        text: Option<String>,
        provider: String,
        model: String,
        raw: Value,
    },
    /// A streamed chunk of reasoning text — the thinking counterpart of
    /// `AssistantDelta`. After-the-fact renderers drop it (the committed
    /// `Thinking` event carries the whole block); live tails may stream it.
    ThinkingDelta {
        content: String,
    },
    /// The user turn appended for this run (`opts.user_message`). Absent on
    /// seed-only resumes.
    UserMessage {
        content: String,
    },
    /// The pre-existing conversation this run was seeded with
    /// (`opts.initial_messages`, e.g. a session's history or an orchestrator
    /// pause/resume seed). Stored ONCE, not re-embedded per turn (spec §3
    /// seed dedup rule): `source_transcript` names a transcript whose
    /// replay-reconstruction equals the seed byte-exact (chains resolve
    /// recursively); `messages` is the full inline `Vec<Message>` fallback
    /// (reasoning `raw` intact) when no caller vouches for a source.
    /// Exactly one of the two is `Some`. `sha256` is over the canonical
    /// seed JSON either way, so replay verifies the chain instead of
    /// trusting it.
    Seed {
        message_count: u32,
        sha256: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        source_transcript: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        messages: Option<Value>,
    },
    /// Context compaction rewrote the conversation. `messages` is the full
    /// post-compaction `Vec<Message>` — the only way a reader can know the
    /// conversation state the following turns were actually built from.
    Compaction {
        seq: u32,
        summarized_messages: u32,
        backup_path: String,
        messages: Value,
    },
    /// A runtime intervention worth showing but not part of the
    /// conversation: `kind` ∈ {"context_trim", "provider_retry"} today.
    Notice {
        kind: String,
        message: String,
    },
    /// Forward-compatibility catch-all (mirrors
    /// `rupu_providers::ContentBlock::Unknown`): an unrecognized `type` tag
    /// lands here instead of failing the line. Never written.
    #[serde(other)]
    Unknown,
}

// `#[serde(remote = "Self")]` above turns the derive output into inherent
// `Event::serialize`/`Event::deserialize` associated functions instead of
// trait impls (the standard trick for hooking custom logic around derived
// serde code — see https://serde.rs/remote-derive.html). We need that hook
// because serde's `#[serde(other)]` fallback, for an *adjacently* tagged
// enum (`tag`+`content`, as opposed to internally tagged), still tries to
// deserialize the unrecognized tag's `content` payload as the catch-all
// variant. Since `Unknown` is a unit variant, that only succeeds when
// `content` is `null`/absent — a `content` of `{"x":1}` (any real-world
// unrecognized event's `data`) fails with "invalid type: map, expected unit
// variant Event::Unknown" before `#[serde(other)]` ever gets a chance to
// apply. So we peek the `type` tag ourselves first via a generic
// `serde_json::Value` buffer, and only hand off to the derived
// `Event::deserialize` when the tag is one we recognize; an unrecognized
// tag short-circuits straight to `Event::Unknown` without ever trying to
// interpret `content`.
//
// This fallback must stay NARROW: `Event::Unknown` is reserved for a
// well-formed line whose `type` is a string that just isn't one we know
// about (forward compatibility with a future writer). Anything structurally
// malformed — no top-level JSON object at all, an object with no `type`
// key, or a `type` that isn't a string — is genuine corruption and must
// still fail with a real deserialize error (the `reader.rs` module doc's
// "bad JSON lines mid-file are returned as `Err(ReadError::Parse)`"
// contract depends on this), so those shapes are explicitly rejected up
// front / delegated to the derived parser's own error paths rather than
// being swallowed into `Unknown`.
impl Serialize for Event {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Event::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        // Anything other than a JSON object can't carry a `type` tag at
        // all — that's corruption, not an unrecognized-but-valid event.
        let Some(obj) = value.as_object() else {
            return Err(serde::de::Error::custom(format!(
                "invalid type: expected a JSON object with a \"type\" field, got {value}"
            )));
        };
        match obj.get("type") {
            // Well-formed (object, string tag) but not one we know:
            // forward-compatible `Unknown`, never an error.
            Some(Value::String(tag)) if !KNOWN_EVENT_TAGS.contains(&tag.as_str()) => {
                Ok(Event::Unknown)
            }
            // Known tag, missing tag, or a non-string tag all fall through
            // to the derived parser: it accepts the known-tag shape, and
            // produces a real error (missing field / wrong type) for the
            // other two — neither should silently become `Unknown`.
            _ => Event::deserialize(value).map_err(serde::de::Error::custom),
        }
    }
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

    #[test]
    fn thinking_event_roundtrips_with_raw_payload() {
        let e = Event::Thinking {
            text: Some("weighing options".into()),
            provider: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            raw: serde_json::json!({"type":"thinking","thinking":"weighing options","signature":"sig-x"}),
        };
        let s = serde_json::to_string(&e).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "thinking");
        assert_eq!(v["data"]["raw"]["signature"], "sig-x");
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);

        // Redacted: text omitted from JSON entirely, parses back as None.
        let redacted = Event::Thinking {
            text: None,
            provider: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            raw: serde_json::json!({"type":"redacted_thinking","data":"opaque"}),
        };
        let s2 = serde_json::to_string(&redacted).unwrap();
        assert!(!s2.contains("\"text\""));
        assert_eq!(serde_json::from_str::<Event>(&s2).unwrap(), redacted);
    }

    #[test]
    fn new_v2_variants_roundtrip() {
        for (e, tag) in [
            (
                Event::ThinkingDelta {
                    content: "chunk".into(),
                },
                "thinking_delta",
            ),
            (
                Event::UserMessage {
                    content: "do the task".into(),
                },
                "user_message",
            ),
            (
                Event::Seed {
                    message_count: 3,
                    sha256: "ab".repeat(32),
                    source_transcript: None,
                    messages: Some(serde_json::json!([{"role":"user","content":[]}])),
                },
                "seed",
            ),
            (
                Event::Seed {
                    message_count: 7,
                    sha256: "cd".repeat(32),
                    source_transcript: Some("/w/.rupu/transcripts/run_prev.jsonl".into()),
                    messages: None,
                },
                "seed",
            ),
            (
                Event::Compaction {
                    seq: 1,
                    summarized_messages: 12,
                    backup_path: "/tmp/b.json".into(),
                    messages: serde_json::json!([]),
                },
                "compaction",
            ),
            (
                Event::Notice {
                    kind: "context_trim".into(),
                    message: "trimmed".into(),
                },
                "notice",
            ),
        ] {
            let s = serde_json::to_string(&e).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["type"], tag);
            assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
        }
    }

    #[test]
    fn unrecognized_event_type_parses_as_unknown_not_error() {
        let line = r#"{"type":"hologram_projection","data":{"x":1}}"#;
        assert_eq!(serde_json::from_str::<Event>(line).unwrap(), Event::Unknown);
    }

    // The three malformed shapes below must all be real deserialize errors,
    // never `Event::Unknown` — `Unknown` is reserved for a well-formed
    // object whose string `type` just isn't one we recognize. Silently
    // downgrading corruption to `Unknown` would break the documented
    // `reader.rs` contract that bad JSON lines surface as
    // `Err(ReadError::Parse)`.

    #[test]
    fn object_missing_type_key_is_a_deserialize_error() {
        let line = r#"{"data":{"turn_idx":0}}"#;
        assert!(serde_json::from_str::<Event>(line).is_err());
    }

    #[test]
    fn non_string_type_is_a_deserialize_error() {
        let line = r#"{"type":123,"data":{}}"#;
        assert!(serde_json::from_str::<Event>(line).is_err());
    }

    #[test]
    fn non_object_top_level_is_a_deserialize_error() {
        assert!(serde_json::from_str::<Event>("[1,2,3]").is_err());
        assert!(serde_json::from_str::<Event>("42").is_err());
        assert!(serde_json::from_str::<Event>("\"just a string\"").is_err());
        assert!(serde_json::from_str::<Event>("null").is_err());
    }

    #[test]
    fn run_start_and_turn_end_additive_fields_are_optional_and_roundtrip() {
        // Legacy line without the new fields still parses.
        let legacy = r#"{"type":"turn_end","data":{"turn_idx":0,"tokens_in":1,"tokens_out":2}}"#;
        assert!(matches!(
            serde_json::from_str::<Event>(legacy).unwrap(),
            Event::TurnEnd {
                stop_reason: None,
                response_id: None,
                ..
            }
        ));
        let e = Event::TurnEnd {
            turn_idx: 4,
            tokens_in: Some(10),
            tokens_out: Some(20),
            stop_reason: Some("tool_use".into()),
            response_id: Some("msg_01".into()),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);

        let legacy_start = r#"{"type":"run_start","data":{"run_id":"r","workspace_id":"w","agent":"a","provider":"p","model":"m","started_at":"2026-08-31T00:00:00Z","mode":"bypass"}}"#;
        assert!(matches!(
            serde_json::from_str::<Event>(legacy_start).unwrap(),
            Event::RunStart {
                schema: None,
                system_prompt: None,
                ..
            }
        ));
    }
}
