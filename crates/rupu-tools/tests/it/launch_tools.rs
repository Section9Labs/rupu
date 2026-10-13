//! The launch tools over a scripted launcher: the canonical `dispatch` /
//! `join` / `workflows.generate` shapes, and the legacy names answering in
//! their pre-W7 shapes (`aliases_byte_compatible`, W7 §6.8).

use crate::support::{ScriptedLauncher, StubGenerator};
use rupu_tools::{ChildKind, ChildTarget, Tool, ToolContext};
use serde_json::{json, Value};
use std::sync::Arc;

fn ctx(launcher: Arc<ScriptedLauncher>, called_as: Option<&str>) -> ToolContext {
    let mut ctx = ToolContext::default();
    ctx.identity_mut().run_id = "run_P".into();
    ctx.services.launcher = Some(launcher);
    ctx.services.workflow_generator = Some(Arc::new(StubGenerator));
    ctx.call.called_as = called_as.map(str::to_string);
    ctx
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/launch")
            .join(name),
    )
    .unwrap()
}

async fn call(tool: &dyn Tool, input: Value, ctx: &ToolContext) -> rupu_tools::ToolOutput {
    tool.invoke(input, ctx).await.unwrap()
}

#[tokio::test]
async fn aliases_byte_compatible() {
    let dispatch = rupu_tools::launch::DispatchTool;
    for (success, single, parallel) in [
        (
            true,
            "dispatch_agent_ok.json",
            "dispatch_agents_parallel_ok.json",
        ),
        (
            false,
            "dispatch_agent_failed.json",
            "dispatch_agents_parallel_failed.json",
        ),
    ] {
        let output = if success { "the answer" } else { "" };
        let launcher = Arc::new(ScriptedLauncher {
            statuses: [(
                "reviewer".to_string(),
                ScriptedLauncher::done("reviewer", output, success),
            )]
            .into(),
            ..Default::default()
        });
        let out = call(
            &dispatch,
            json!({ "agent": "reviewer", "prompt": "p" }),
            &ctx(launcher.clone(), Some("dispatch_agent")),
        )
        .await;
        assert_eq!(out.error, None);
        assert_eq!(out.stdout, golden(single), "dispatch_agent ({success})");

        let out = call(
            &dispatch,
            json!({ "agents": [
                { "id": "b", "agent": "reviewer", "prompt": "p" },
                { "id": "a", "agent": "reviewer", "prompt": "q" },
            ] }),
            &ctx(launcher.clone(), Some("dispatch_agents_parallel")),
        )
        .await;
        assert_eq!(out.error, None);
        assert_eq!(
            out.stdout,
            golden(parallel),
            "dispatch_agents_parallel ({success})"
        );
        // Every legacy sub-agent is a `subagent` child.
        assert!(launcher
            .launched
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.kind == ChildKind::Subagent));
    }
}

#[tokio::test]
async fn dispatch_agent_folds_inputs_into_the_prompt() {
    let launcher = Arc::new(ScriptedLauncher::default());
    call(
        &rupu_tools::launch::DispatchTool,
        json!({ "agent": "reviewer", "prompt": "review", "inputs": { "subject": "x" } }),
        &ctx(launcher.clone(), Some("dispatch_agent")),
    )
    .await;
    let launched = launcher.launched.lock().unwrap();
    assert_eq!(
        launched[0].prompt.as_deref(),
        Some("review\n\nInputs (JSON):\n{\n  \"subject\": \"x\"\n}")
    );
}

#[tokio::test]
async fn dispatch_agents_parallel_checks_every_request_before_starting_any() {
    let launcher = Arc::new(ScriptedLauncher {
        refuse: vec!["intruder".into()],
        ..Default::default()
    });
    let out = call(
        &rupu_tools::launch::DispatchTool,
        json!({ "agents": [
            { "id": "1", "agent": "reviewer", "prompt": "p" },
            { "id": "2", "agent": "intruder", "prompt": "p" },
        ] }),
        &ctx(launcher.clone(), Some("dispatch_agents_parallel")),
    )
    .await;
    let err = out.error.unwrap();
    assert!(
        err.contains("agent_not_dispatchable") && err.contains("(id `2`)"),
        "{err}"
    );
    assert!(
        launcher.launched.lock().unwrap().is_empty(),
        "nothing started"
    );
}

#[tokio::test]
async fn the_flow_forms_answer_with_handle_and_participant() {
    let launcher = Arc::new(ScriptedLauncher {
        address: Some("lynx#1".into()),
        ..Default::default()
    });
    let dispatch = rupu_tools::launch::DispatchTool;
    // Inside a flow, `dispatch {agent, prompt}` without a kind: an agent
    // process.
    let mut flow_ctx = ctx(launcher.clone(), None);
    flow_ctx.identity_mut().launch.flow = Some("/g/agentiflows/af_1".into());
    let out = call(
        &dispatch,
        json!({ "agent": "recon", "prompt": "scan" }),
        &flow_ctx,
    )
    .await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(body, json!({ "handle": "ch_0", "participant": "lynx#1" }));
    // `run_workflow {workflow, inputs}`.
    let out = call(
        &dispatch,
        json!({ "workflow": "web-assess", "inputs": { "target": "x", "depth": 2 } }),
        &ctx(launcher.clone(), Some("run_workflow")),
    )
    .await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(body, json!({ "handle": "ch_1", "participant": "lynx#1" }));
    let launched = launcher.launched.lock().unwrap().clone();
    assert_eq!(launched[0].kind, ChildKind::Agent);
    assert_eq!(launched[0].prompt.as_deref(), Some("scan"));
    assert_eq!(launched[1].kind, ChildKind::Workflow);
    assert_eq!(launched[1].target, ChildTarget::Name("web-assess".into()));
    assert_eq!(launched[1].inputs["depth"], json!(2));
}

#[tokio::test]
async fn dispatch_returns_a_handle_and_wait_adds_the_result() {
    let launcher = Arc::new(ScriptedLauncher::default());
    let dispatch = rupu_tools::launch::DispatchTool;
    let out = call(
        &dispatch,
        json!({ "kind": "agent", "name": "recon", "prompt": "scan" }),
        &ctx(launcher.clone(), None),
    )
    .await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(body["handle"], "ch_0");
    assert_eq!(body["kind"], "agent");
    assert_eq!(body["name"], "recon");
    assert!(body.get("result").is_none(), "async by default");

    let out = call(
        &dispatch,
        json!({ "name": "reviewer", "prompt": "look", "wait": true }),
        &ctx(launcher.clone(), None),
    )
    .await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(body["kind"], "subagent", "subagent is the default kind");
    assert_eq!(body["result"]["status"], "done");
    assert_eq!(body["result"]["output"], "reviewer done");
}

#[tokio::test]
async fn dispatch_refuses_malformed_calls() {
    let launcher = Arc::new(ScriptedLauncher::default());
    let dispatch = rupu_tools::launch::DispatchTool;
    for input in [
        json!({ "kind": "robot", "name": "x", "prompt": "p" }),
        json!({ "kind": "agent", "prompt": "p" }),
        json!({ "kind": "agent", "name": "x" }),
        json!({ "kind": "agent", "file": "x.yaml", "prompt": "p" }),
        json!({ "kind": "workflow", "name": "w", "inputs": { "k": [1] } }),
    ] {
        assert!(
            dispatch
                .invoke(input.clone(), &ctx(launcher.clone(), None))
                .await
                .is_err(),
            "{input}"
        );
    }
    assert!(launcher.launched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn join_reports_children_and_the_flow_form_its_old_status() {
    let launcher = Arc::new(ScriptedLauncher::default());
    let c = ctx(launcher.clone(), None);
    call(
        &rupu_tools::launch::DispatchTool,
        json!({ "kind": "agent", "name": "recon", "prompt": "scan" }),
        &c,
    )
    .await;
    let join = rupu_tools::launch::JoinTool;
    let out = call(&join, json!({}), &c).await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(body["children"][0]["handle"], "ch_0");
    assert_eq!(body["children"][0]["status"], "done");
    assert_eq!(body["children"][0]["output"], "recon done");

    let out = call(&join, json!({ "handle": "ch_0" }), &c).await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(
        body,
        json!({ "status": "done", "success": true, "output": "recon done" })
    );
    let out = call(&join, json!({ "handle": "ch_9" }), &c).await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(
        body,
        json!({ "status": "failed", "error": "unknown handle" })
    );
}

#[tokio::test]
async fn workflows_generate_saves_and_generate_workflow_also_starts_it() {
    let launcher = Arc::new(ScriptedLauncher::default());
    let tool = rupu_tools::launch::WorkflowsGenerateTool;
    let out = call(
        &tool,
        json!({ "description": "triage the inbox" }),
        &ctx(launcher.clone(), None),
    )
    .await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(
        body,
        json!({
            "file": "/runs/run_P/generated/triage-01.yaml",
            "name": "triage",
            "steps": 2,
            "inputs": ["target"],
        })
    );
    assert!(
        launcher.launched.lock().unwrap().is_empty(),
        "nothing started"
    );

    let out = call(
        &tool,
        json!({ "description": "triage", "inputs": { "target": "x" } }),
        &ctx(launcher.clone(), Some("generate_workflow")),
    )
    .await;
    let body: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(body["handle"], "ch_0");
    assert_eq!(
        body["generated_file"],
        "/runs/run_P/generated/triage-01.yaml"
    );
    let launched = launcher.launched.lock().unwrap();
    assert_eq!(
        launched[0].target,
        ChildTarget::File("/runs/run_P/generated/triage-01.yaml".into())
    );
}

/// Outside a flow there was never a `dispatch` tool: the legacy shape there
/// comes from a `dispatch_agent` prompt, and answers as `dispatch_agent`.
#[tokio::test]
async fn the_legacy_agent_shape_outside_a_flow_is_dispatch_agent() {
    let launcher = Arc::new(ScriptedLauncher {
        statuses: [(
            "reviewer".to_string(),
            ScriptedLauncher::done("reviewer", "the answer", true),
        )]
        .into(),
        ..Default::default()
    });
    let out = call(
        &rupu_tools::launch::DispatchTool,
        json!({ "agent": "reviewer", "prompt": "p" }),
        &ctx(launcher.clone(), None),
    )
    .await;
    assert_eq!(out.stdout, golden("dispatch_agent_ok.json"));
    assert_eq!(
        launcher.launched.lock().unwrap()[0].kind,
        ChildKind::Subagent
    );
}
