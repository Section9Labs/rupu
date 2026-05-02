use chrono::{TimeZone, Utc};
use rupu_transcript::event::{Event, FileEditKind, RunMode, RunStatus};

fn assert_roundtrip(e: &Event) {
    let json = serde_json::to_string(e).expect("serialize");
    let back: Event = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        e, &back,
        "roundtrip differed:\n  in:  {e:?}\n  out: {back:?}"
    );
}

#[test]
fn roundtrip_run_start() {
    assert_roundtrip(&Event::RunStart {
        run_id: "run_01HXXX".into(),
        workspace_id: "ws_01HXXX".into(),
        agent: "fix-bug".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        started_at: Utc.with_ymd_and_hms(2026, 5, 1, 17, 0, 0).unwrap(),
        mode: RunMode::Ask,
    });
}

#[test]
fn roundtrip_turn_start() {
    assert_roundtrip(&Event::TurnStart { turn_idx: 0 });
}

#[test]
fn roundtrip_assistant_message() {
    assert_roundtrip(&Event::AssistantMessage {
        content: "Looking at the failing test now.".into(),
        thinking: None,
    });
    assert_roundtrip(&Event::AssistantMessage {
        content: "Here's my plan.".into(),
        thinking: Some("First I'll grep for the symbol...".into()),
    });
}

#[test]
fn roundtrip_tool_call_and_result() {
    assert_roundtrip(&Event::ToolCall {
        call_id: "call_1".into(),
        tool: "bash".into(),
        input: serde_json::json!({ "command": "cargo test" }),
    });
    assert_roundtrip(&Event::ToolResult {
        call_id: "call_1".into(),
        output: "test result: ok. 12 passed".into(),
        error: None,
        duration_ms: 421,
    });
    assert_roundtrip(&Event::ToolResult {
        call_id: "call_2".into(),
        output: String::new(),
        error: Some("permission_denied".into()),
        duration_ms: 0,
    });
}

#[test]
fn roundtrip_file_edit() {
    assert_roundtrip(&Event::FileEdit {
        path: "src/lib.rs".into(),
        kind: FileEditKind::Modify,
        diff: "@@ -1,3 +1,4 @@\n fn foo() {\n+    todo!()\n }".into(),
    });
}

#[test]
fn roundtrip_command_run() {
    assert_roundtrip(&Event::CommandRun {
        argv: vec!["cargo".into(), "test".into()],
        cwd: "/Users/matt/Code/Oracle/rupu".into(),
        exit_code: 0,
        stdout_bytes: 4096,
        stderr_bytes: 128,
    });
}

#[test]
fn roundtrip_action_emitted() {
    assert_roundtrip(&Event::ActionEmitted {
        kind: "open_pr".into(),
        payload: serde_json::json!({ "title": "fix bug", "branch": "fix/123" }),
        allowed: true,
        applied: false,
        reason: None,
    });
    assert_roundtrip(&Event::ActionEmitted {
        kind: "delete_branch".into(),
        payload: serde_json::json!({}),
        allowed: false,
        applied: false,
        reason: Some("not in step allowlist".into()),
    });
}

#[test]
fn roundtrip_gate_requested() {
    assert_roundtrip(&Event::GateRequested {
        gate_id: "gate_1".into(),
        prompt: "Approve PR open?".into(),
        decision: None,
        decided_by: None,
    });
}

#[test]
fn roundtrip_turn_end() {
    assert_roundtrip(&Event::TurnEnd {
        turn_idx: 0,
        tokens_in: Some(1234),
        tokens_out: Some(567),
    });
}

#[test]
fn roundtrip_turn_end_no_token_counts() {
    assert_roundtrip(&Event::TurnEnd {
        turn_idx: 0,
        tokens_in: None,
        tokens_out: None,
    });
}

#[test]
fn roundtrip_run_complete_ok() {
    assert_roundtrip(&Event::RunComplete {
        run_id: "run_01HXXX".into(),
        status: RunStatus::Ok,
        total_tokens: 5000,
        duration_ms: 12345,
        error: None,
    });
}

#[test]
fn roundtrip_run_complete_error_with_reason() {
    assert_roundtrip(&Event::RunComplete {
        run_id: "run_01HXXX".into(),
        status: RunStatus::Error,
        total_tokens: 5000,
        duration_ms: 12345,
        error: Some("context_overflow".into()),
    });
}
