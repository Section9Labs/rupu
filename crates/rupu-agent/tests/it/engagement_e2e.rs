//! End-to-end: an agent running under an engagement profile records a finding
//! that routes to the owning profile, passes its completeness gate, and stamps
//! the asset graph — and `asset_mark` advances coverage depth. Drives the real
//! `run_agent` loop with a scripted mock provider and synthetic finding data.

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{builtin_registry, target_id, CoveragePaths, FindingWriteOptions};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

/// A complete, valid finding report (synthetic repo fixture). The engagement
/// asset supplies the network identity; the report body is reused as-is.
fn report() -> serde_json::Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap()
}

/// A `network:service` asset pinned to a host + port (what the network
/// profile's `service_identified` completeness check requires).
fn service_asset(with_port: bool) -> serde_json::Value {
    let mut coords = vec![serde_json::json!({ "t": "host", "v": "10.0.0.5" })];
    if with_port {
        coords.push(serde_json::json!({ "t": "port", "v": { "number": 22, "proto": "tcp" } }));
    }
    serde_json::json!({ "kind": "network:service", "coordinates": coords })
}

fn record(id: &str, asset: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: "report_finding".into(),
        tool_input: serde_json::json!({ "scope": "repo", "report": report(), "asset": asset }),
        stop: StopReason::ToolUse,
    }
}

fn mark(id: &str, depth: &str) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: "asset_mark".into(),
        tool_input: serde_json::json!({
            "kind": "network:service",
            "coordinates": [
                { "t": "host", "v": "10.0.0.5" },
                { "t": "port", "v": { "number": 22, "proto": "tcp" } }
            ],
            "depth": depth,
        }),
        stop: StopReason::ToolUse,
    }
}

fn done() -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: "Done.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

fn opts(
    workspace: &std::path::Path,
    store: &std::path::Path,
    turns: Vec<ScriptedTurn>,
) -> AgentRunOpts {
    let engagement = builtin_registry()
        .unwrap()
        .active_set(&["network".into()])
        .unwrap();
    AgentRunOpts {
        on_usage: None,
        seed_source: None,
        agent_name: "assessor".into(),
        agent_system_prompt: "You assess networks.".into(),
        agent_tools: Some(vec!["report_finding".into(), "asset_mark".into()]),
        provider: Box::new(CapturingMockProvider::new(turns)),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_engagement".into(),
        workspace_id: "ws_engagement".into(),
        workspace_path: workspace.to_path_buf(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 8,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.to_path_buf(),
            findings: Some(FindingWriteOptions {
                artifact_root: Some(store.to_path_buf()),
                engagement: Some(Arc::new(engagement)),
                ..Default::default()
            }),
            ..Default::default()
        },
        user_message: "Assess the host.".into(),
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
    }
}

#[tokio::test]
async fn engagement_run_routes_gates_stamps_and_marks_depth() {
    let ws = tempfile::TempDir::new().unwrap();
    let store = tempfile::TempDir::new().unwrap();
    let turns = vec![
        record("t1", service_asset(true)),
        mark("t2", "tested"),
        done(),
    ];
    run_agent(opts(ws.path(), store.path(), turns))
        .await
        .expect("run succeeds");

    let paths = CoveragePaths::new(ws.path(), &target_id(ws.path(), "assessor"));

    // The finding was recorded under the full profile.
    let findings = std::fs::read_to_string(&paths.findings).unwrap();
    let flines: Vec<_> = findings.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(flines.len(), 1, "exactly one finding recorded");

    // The asset was stamped and routed as network:service, and asset_mark
    // advanced its depth to `tested`.
    let assets = rupu_coverage::read_assets(&paths.assets).unwrap();
    assert_eq!(assets.len(), 1, "one asset in the graph");
    assert_eq!(assets[0].kind, "network:service");
    assert_eq!(assets[0].depth.as_deref(), Some("tested"));
}

#[tokio::test]
async fn completeness_gate_rejects_incomplete_then_retry_records() {
    let ws = tempfile::TempDir::new().unwrap();
    let store = tempfile::TempDir::new().unwrap();
    // First call omits the port: the network profile's `service_identified`
    // check fails, the finding is refused, the agent fixes it, and the retry
    // records exactly one finding.
    let turns = vec![
        record("t1", service_asset(false)),
        record("t2", service_asset(true)),
        done(),
    ];
    run_agent(opts(ws.path(), store.path(), turns))
        .await
        .expect("run succeeds (a refused tool call does not fail the run)");

    let paths = CoveragePaths::new(ws.path(), &target_id(ws.path(), "assessor"));
    let findings = std::fs::read_to_string(&paths.findings).unwrap();
    let flines: Vec<_> = findings.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(flines.len(), 1, "only the complete retry is recorded");

    let transcript = std::fs::read_to_string(ws.path().join("run.jsonl")).unwrap();
    assert!(
        transcript.contains("service_identified")
            || transcript.to_lowercase().contains("host + port")
            || transcript.contains("incomplete"),
        "the rejection explains the unmet completeness check"
    );
}
