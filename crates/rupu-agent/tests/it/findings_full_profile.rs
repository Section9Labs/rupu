//! Under the default (full) profile an agent must send a complete report.
//! An incomplete one is rejected with every problem listed; the agent fixes
//! it and the retry records exactly one finding.

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{target_id, CoveragePaths, FindingWriteOptions};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

fn report() -> serde_json::Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap()
}

fn call(id: &str, report: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: "report_finding".into(),
        tool_input: serde_json::json!({ "scope": "repo", "report": report }),
        stop: StopReason::ToolUse,
    }
}

fn opts(
    workspace: &std::path::Path,
    store: &std::path::Path,
    turns: Vec<ScriptedTurn>,
) -> AgentRunOpts {
    AgentRunOpts {
        on_usage: None,
        seed_source: None,
        collectors: Vec::new(),
        agent_name: "assessor".into(),
        agent_system_prompt: "You assess code.".into(),
        agent_tools: Some(vec!["report_finding".into()]),
        provider: Box::new(CapturingMockProvider::new(turns)),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_full".into(),
        workspace_id: "ws_full".into(),
        workspace_path: workspace.to_path_buf(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 6,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.to_path_buf(),
            findings: Some(FindingWriteOptions {
                artifact_root: Some(store.to_path_buf()),
                ..Default::default()
            }),
            ..Default::default()
        },
        user_message: "Assess.".into(),
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
        concerns: None,
        scope_name: None,
        limits: rupu_providers::model_limits::ModelLimits::unknown(),
        surface_tag: Some("agent".into()),
        pause: None,
        codename: None,
        recovery: Default::default(),
    }
}

#[tokio::test]
async fn incomplete_report_is_rejected_then_retry_records_one_finding() {
    let ws = tempfile::TempDir::new().unwrap();
    let store = tempfile::TempDir::new().unwrap();
    let mut bad = report();
    bad["root_cause"] = serde_json::json!("");
    let turns = vec![
        call("t1", bad),
        call("t2", report()),
        ScriptedTurn::AssistantText {
            text: "Done.".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ];
    run_agent(opts(ws.path(), store.path(), turns))
        .await
        .expect("run succeeds");

    let paths = CoveragePaths::new(ws.path(), &target_id(ws.path(), "assessor"));
    let text = std::fs::read_to_string(&paths.findings).unwrap();
    let lines: Vec<_> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly the retry is recorded");
    let rec: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(rec["profile"], "full");
    assert_eq!(rec["severity"], "critical");

    let transcript = std::fs::read_to_string(ws.path().join("run.jsonl")).unwrap();
    assert!(
        transcript.contains("report.root_cause"),
        "rejection names the field"
    );
    assert!(
        transcript.contains("## Recording findings"),
        "full-profile guidance in the system prompt"
    );
}
