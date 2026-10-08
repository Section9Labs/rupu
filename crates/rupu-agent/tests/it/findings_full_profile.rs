//! Under the default (full) profile an agent must send a complete report.
//! An incomplete one is rejected with every problem listed; the agent fixes
//! it and the retry records exactly one finding.

use rupu_agent::runner::{CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{target_id, CoveragePaths, FindingWriteOptions};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;

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
    rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "You assess code.".into(),
            prompt: rupu_agent::UserTurn::new("Assess."),
            provider: Box::new(CapturingMockProvider::new(turns)),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: rupu_tools::PermissionPolicy::bypass(),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: {
                let mut tc = ToolContext {
                    workspace: rupu_tools::WorkspaceScope {
                        path: workspace.to_path_buf(),
                        ..Default::default()
                    },
                    services: rupu_tools::ToolServices {
                        findings: Some(FindingWriteOptions {
                            artifact_root: Some(store.to_path_buf()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                tc.identity = std::sync::Arc::new(rupu_tools::RunIdentity {
                    agent: "assessor".into(),
                    provider: "mock".into(),
                    model: "mock-1".into(),
                    run_id: "run_full".into(),
                    surface: rupu_tools::Surface::Agent,
                    ..Default::default()
                });
                tc.workspace.id = "ws_full".into();
                tc.workspace.path = workspace.to_path_buf();
                tc
            },
            pins: Default::default(),
            concerns: None,
            max_turns: 6,
            stream: rupu_agent::StreamOpts {
                no_stream: true,
                suppress_stdout: false,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
            transcript_path: workspace.join("run.jsonl"),
        },
        (Some(vec!["report_finding".into()])).as_deref(),
        &Vec::new(),
    )
    .expect("grant")
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
