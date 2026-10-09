//! `findings_single_impl` (spec W4 §6.1): `findings.report` is one
//! implementation. Called through the agent loop, through an `action:` step
//! and through `rupu mcp serve`'s `CatalogServer` with the same input and
//! the same run identity, it writes byte-identical ledger lines (apart from
//! the minted id and the timestamp). With a run stream, `findings.tag`
//! mirrors to it in the agent path.

use rupu_agent::runner::{MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_mcp::{CatalogServer, InProcessTransport, Transport};
use rupu_orchestrator::runner::{call_action_tool, ActionServices};
use rupu_providers::types::StopReason;
use rupu_tools::{PermissionMode, PermissionPolicy, RunIdentity, Surface, ToolContext};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

fn identity() -> RunIdentity {
    RunIdentity {
        run_id: "run_same".into(),
        codename: Some("jade-reef".into()),
        agent: "reviewer".into(),
        provider: "mock".into(),
        model: "mock-1".into(),
        surface: Surface::Workflow,
        scope_name: Some("single-impl".into()),
        ..Default::default()
    }
}

/// The context every transport calls in: same identity, the workspace, the
/// default (full) findings profile, an empty SCM registry.
fn context(workspace: &Path, stream: Option<&Path>) -> ToolContext {
    let mut ctx = ToolContext::in_workspace(workspace);
    ctx.identity = Arc::new(identity());
    ctx.services.findings = Some(rupu_coverage::FindingWriteOptions::default());
    ctx.services.scm = Some(Arc::new(rupu_scm::Registry::empty()));
    ctx.services.coverage_stream = stream.map(Path::to_path_buf);
    ctx
}

fn input() -> Value {
    let report: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    json!({ "scope": "repo", "tags": ["class:authz"], "report": report })
}

fn agent_opts(ctx: ToolContext, turns: Vec<ScriptedTurn>, transcript: &Path) -> AgentRunOpts {
    rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "test".into(),
            prompt: rupu_agent::UserTurn::new("go"),
            provider: Box::new(MockProvider::new(turns)),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: PermissionPolicy::bypass(),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: ctx,
            pins: Default::default(),
            concerns: None,
            max_turns: 5,
            stream: rupu_agent::StreamOpts {
                no_stream: true,
                suppress_stdout: true,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
            transcript_path: transcript.to_path_buf(),
        },
        Some(&["findings.*".to_string()]),
        &[],
    )
    .expect("grant")
}

fn call_then_stop(tool: &str, input: Value) -> Vec<ScriptedTurn> {
    vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "t1".into(),
            tool_name: tool.into(),
            tool_input: input,
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantText {
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]
}

/// The one findings line `workspace`'s ledger holds, with the minted id and
/// the timestamp replaced by placeholders.
fn ledger_line(workspace: &Path) -> String {
    let ctx = context(workspace, None);
    let text = std::fs::read_to_string(rupu_tools::ledger::paths(&ctx).findings).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    let v: Value = serde_json::from_str(lines[0]).unwrap();
    lines[0]
        .replace(v["id"].as_str().unwrap(), "<id>")
        .replace(v["declared_at"].as_str().unwrap(), "<at>")
}

#[tokio::test]
async fn findings_report_writes_the_same_line_through_every_transport() {
    // Agent loop.
    let agent_ws = tempfile::TempDir::new().unwrap();
    let res = run_agent(agent_opts(
        context(agent_ws.path(), None),
        call_then_stop("findings.report", input()),
        &agent_ws.path().join("t.jsonl"),
    ))
    .await
    .unwrap();
    assert_eq!(res.status, rupu_transcript::RunStatus::Ok);

    // `action: findings.record` (the step's name for the same tool).
    let action_ws = tempfile::TempDir::new().unwrap();
    let services = ActionServices {
        context: context(action_ws.path(), None),
        mode: PermissionMode::Bypass,
    };
    let out = call_action_tool(
        &services,
        "findings.record",
        input(),
        rupu_coverage::FindingProfile::Full,
    )
    .await
    .unwrap();
    assert!(out.starts_with("finding_id: fnd_"), "{out}");

    // `rupu mcp serve`.
    let mcp_ws = tempfile::TempDir::new().unwrap();
    let (client, server_t) = InProcessTransport::pair();
    let (server, _) = CatalogServer::new(
        server_t,
        context(mcp_ws.path(), None),
        PermissionMode::Bypass,
        None,
    )
    .unwrap();
    let handle = tokio::spawn(server.run());
    client
        .send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "findings.report", "arguments": input() }
        }))
        .await
        .unwrap();
    let resp = client.recv().await.unwrap().unwrap();
    assert!(resp["result"]["isError"].is_null(), "{resp}");
    drop(client);
    let _ = handle.await;

    let agent = ledger_line(agent_ws.path());
    assert_eq!(agent, ledger_line(action_ws.path()), "agent vs action");
    assert_eq!(agent, ledger_line(mcp_ws.path()), "agent vs mcp");
    assert!(agent.contains("\"run_id\":\"run_same\""), "{agent}");
}

#[tokio::test]
async fn findings_tag_mirrors_to_the_run_stream_in_the_agent_path() {
    let ws = tempfile::TempDir::new().unwrap();
    let services = ActionServices {
        context: context(ws.path(), None),
        mode: PermissionMode::Bypass,
    };
    let out = call_action_tool(
        &services,
        "findings.report",
        input(),
        rupu_coverage::FindingProfile::Full,
    )
    .await
    .unwrap();
    let id = out.trim_start_matches("finding_id: ").to_string();

    let stream = ws.path().join("coverage.jsonl");
    run_agent(agent_opts(
        context(ws.path(), Some(&stream)),
        call_then_stop(
            "findings.tag",
            json!({ "finding_ids": [id], "add": ["needs-poc"] }),
        ),
        &ws.path().join("t.jsonl"),
    ))
    .await
    .unwrap();

    let text = std::fs::read_to_string(&stream).expect("run stream written");
    let tag_line = text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v.to_string().contains("needs-poc"))
        .unwrap_or_else(|| panic!("no tag event in the stream: {text}"));
    assert!(tag_line.to_string().contains(&id), "{tag_line}");
}
