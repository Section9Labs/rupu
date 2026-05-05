use rupu_tui::state::{NodeState, NodeStatus, TokenCounters};

#[test]
fn fresh_node_is_waiting_with_empty_tokens() {
    let n = NodeState::new("step-1", "spec-agent");
    assert_eq!(n.status, NodeStatus::Waiting);
    assert_eq!(n.tokens, TokenCounters::default());
    assert_eq!(n.tools_used.len(), 0);
    assert!(n.transcript_tail.is_empty());
}

use chrono::Utc;
use rupu_transcript::Event;
use rupu_tui::state::RunModel;

fn run_start(_step_id: &str, agent: &str) -> Event {
    Event::RunStart {
        run_id: "run_test".into(),
        workspace_id: "ws_test".into(),
        agent: agent.into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        started_at: Utc::now(),
        mode: rupu_transcript::RunMode::Ask,
    }
}

#[test]
fn run_start_marks_node_active() {
    let mut m = RunModel::new();
    m.upsert_node("step-1", "spec-agent");
    m.apply_event("step-1", &run_start("step-1", "spec-agent"));
    assert_eq!(m.node("step-1").unwrap().status, NodeStatus::Active);
}

#[test]
fn turn_start_marks_node_working_and_increments_turn_idx() {
    let mut m = RunModel::new();
    m.upsert_node("step-1", "spec-agent");
    m.apply_event("step-1", &Event::TurnStart { turn_idx: 0 });
    let n = m.node("step-1").unwrap();
    assert_eq!(n.status, NodeStatus::Working);
    assert_eq!(n.turn_idx, 1);
}

#[test]
fn tool_call_records_last_action_and_increments_counter() {
    let mut m = RunModel::new();
    m.upsert_node("step-1", "spec-agent");
    m.apply_event("step-1", &Event::ToolCall {
        call_id: "c1".into(),
        tool: "bash".into(),
        input: serde_json::json!({"command": "cargo test"}),
    });
    let n = m.node("step-1").unwrap();
    assert_eq!(n.tools_used.get("bash"), Some(&1));
    assert!(n.last_action.is_some());
}

#[test]
fn usage_accumulates_tokens() {
    let mut m = RunModel::new();
    m.upsert_node("step-1", "spec-agent");
    m.apply_event("step-1", &Event::Usage {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        input_tokens: 100,
        output_tokens: 50,
        cached_tokens: 10,
    });
    let n = m.node("step-1").unwrap();
    assert_eq!(n.tokens.input, 100);
    assert_eq!(n.tokens.output, 50);
    assert_eq!(n.tokens.cached, 10);
}

#[test]
fn gate_requested_flips_to_awaiting() {
    let mut m = RunModel::new();
    m.upsert_node("step-1", "deploy-gate");
    m.apply_event("step-1", &Event::GateRequested {
        gate_id: "g1".into(),
        prompt: "Deploy v2.31?".into(),
        decision: None,
        decided_by: None,
    });
    let n = m.node("step-1").unwrap();
    assert_eq!(n.status, NodeStatus::Awaiting);
    assert_eq!(n.gate_prompt.as_deref(), Some("Deploy v2.31?"));
}

#[test]
fn run_complete_with_ok_flips_to_complete() {
    let mut m = RunModel::new();
    m.upsert_node("step-1", "spec-agent");
    m.apply_event("step-1", &Event::RunComplete {
        run_id: "run_test".into(),
        status: rupu_transcript::RunStatus::Ok,
        total_tokens: 0,
        duration_ms: 0,
        error: None,
    });
    assert_eq!(m.node("step-1").unwrap().status, NodeStatus::Complete);
}
