//! `report_finding` must be usable WITHOUT the coverage harness.
//!
//! Recording a finding and running the coverage harness are different things.
//! The harness is code-shaped — it needs a catalog of concerns and marks
//! (concern_id, file_path) pairs — so an assessment of hosts, endpoints or
//! cloud resources has no catalog and, before this, no way to record a finding
//! at all. It reported findings somewhere else entirely and the control plane
//! showed zero.

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{target_id, CoveragePaths};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

/// A finding with no `file_path` — the shape a network assessment produces.
fn finding_input() -> serde_json::Value {
    serde_json::json!({
        "scope": "repo",
        "summary": "Console endpoint accepts a percent-encoded approval bypass",
        "severity": "medium",
        "evidence": { "rationale": "Observed on the live endpoint; no file involved." }
    })
}

fn opts_for(
    workspace: &std::path::Path,
    agent_tools: Option<Vec<String>>,
    turns: Vec<ScriptedTurn>,
) -> AgentRunOpts {
    AgentRunOpts {
        seed_source: None,
        agent_name: "net-assessor".into(),
        agent_system_prompt: "You assess hosts.".into(),
        agent_tools,
        provider: Box::new(CapturingMockProvider::new(turns)),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_findings_test".into(),
        workspace_id: "ws_findings_test".into(),
        workspace_path: workspace.to_path_buf(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.to_path_buf(),
            ..Default::default()
        },
        user_message: "Assess the endpoint.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        // The whole point: no coverage harness.
        concerns: None,
        scope_name: None,
        max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
        surface_tag: Some("autoflow".into()),
        context_window_tokens: None,
        compact_at_percent: None,
        pause: None,
    }
}

fn call_then_stop() -> Vec<ScriptedTurn> {
    vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "t1".into(),
            tool_name: "report_finding".into(),
            tool_input: finding_input(),
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantText {
            text: "Recorded.".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]
}

#[tokio::test]
async fn granted_agent_records_a_finding_without_a_concerns_block() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    run_agent(opts_for(
        &workspace,
        Some(vec!["report_finding".to_string()]),
        call_then_stop(),
    ))
    .await
    .expect("agent run should succeed");

    let paths = CoveragePaths::new(&workspace, &target_id(&workspace, "net-assessor"));
    let text = std::fs::read_to_string(&paths.findings)
        .unwrap_or_else(|e| panic!("findings ledger at {:?} should exist: {e}", paths.findings));
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("one record");
    let rec: serde_json::Value = serde_json::from_str(line).expect("valid JSON record");

    assert_eq!(rec["severity"], "medium");
    assert_eq!(rec["scope"], "repo");
    assert!(rec["file_path"].is_null(), "network finding has no file");

    // Attribution used to be set only when coverage was enabled, which would
    // have written this record with empty run_id/model — a silent hole in the
    // audit trail rather than a loud failure.
    assert_eq!(
        rec["declared_by"]["run_id"], "run_findings_test",
        "attribution must survive without the coverage harness"
    );
    assert_eq!(rec["declared_by"]["model"], "mock-1");
    assert_eq!(rec["declared_by"]["surface"], "autoflow");
}

#[tokio::test]
async fn tool_is_absent_when_not_granted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // Same run, same script, but `report_finding` is not in `tools:`.
    // Registration is an explicit grant, not automatic — if this ever starts
    // producing a ledger, the grant has stopped meaning anything.
    let _ = run_agent(opts_for(
        &workspace,
        Some(vec!["read_file".to_string()]),
        call_then_stop(),
    ))
    .await;

    let paths = CoveragePaths::new(&workspace, &target_id(&workspace, "net-assessor"));
    assert!(
        !paths.findings.exists(),
        "ungranted agent must not be able to write findings"
    );
}
