//! `WorkflowRuntime::new`: a workflow another run launched refuses to start
//! a step whose agent its parent may not dispatch (W7).

use std::sync::Arc;

use rupu_orchestrator::{RunStore, Workflow, WorkflowRuntime, WorkflowRuntimeInputs};
use rupu_runtime::argv::LaunchLink;
use rupu_runtime::assembly::{AssemblyContext, WorkspaceBinding};
use rupu_tools::PermissionMode;

const WF: &str = "name: sweep\nsteps:\n  - id: scan\n    agent: probe\n    prompt: go\n  - id: fix\n    agent: outsider\n    prompt: go\n";

fn link(agents: &[&str]) -> LaunchLink {
    LaunchLink {
        depth: 1,
        ceiling: PermissionMode::Bypass,
        parent_run: "run_parent".into(),
        root_run: "run_parent".into(),
        usage_root: None,
        agents: agents.iter().map(|s| s.to_string()).collect(),
        workflows: Vec::new(),
        actions: None,
        no_messaging: false,
        strict_approval: false,
    }
}

#[test]
fn a_launched_workflow_refuses_a_step_agent_outside_its_parents_cap() {
    let dir = tempfile::tempdir().unwrap();
    let err = WorkflowRuntime::new(inputs(dir.path(), Some(link(&["probe"]))))
        .err()
        .expect("refused");
    assert!(err.to_string().contains("outsider"), "{err}");
    assert!(WorkflowRuntime::new(inputs(dir.path(), Some(link(&["probe", "outsider"])))).is_ok());
    assert!(WorkflowRuntime::new(inputs(dir.path(), None)).is_ok());
}

fn inputs(dir: &std::path::Path, link: Option<LaunchLink>) -> WorkflowRuntimeInputs {
    WorkflowRuntimeInputs {
        context: AssemblyContext::minimal(dir.join("global")),
        workflow: Workflow::parse(WF).unwrap(),
        run_id: "run_child".into(),
        workspace: WorkspaceBinding {
            id: "ws".into(),
            path: dir.to_path_buf(),
        },
        store: Arc::new(RunStore::new(dir.join("runs"))),
        events: None,
        launcher: None,
        link,
        mode: PermissionMode::Bypass,
        system_prompt_suffix: None,
        scope_name_override: None,
        codename: None,
    }
}

#[test]
fn harness_workflow_refuses_headless_ask_and_remote_placement_on_launch_or_resume() {
    for use_marker in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut policy_link = link(&["probe", "outsider"]);
        policy_link.strict_approval = !use_marker;
        if use_marker {
            rupu_runtime::strict_approval::require_workflow(
                &dir.path().join("global/runs/run_child"),
            )
            .unwrap();
        }
        let mut request = inputs(dir.path(), (!use_marker).then_some(policy_link.clone()));
        request.mode = PermissionMode::Ask;
        assert!(matches!(
            WorkflowRuntime::new(request),
            Err(rupu_orchestrator::WorkflowRuntimeError::ApprovalUnavailable)
        ));

        let mut request = inputs(dir.path(), (!use_marker).then_some(policy_link.clone()));
        request.workflow = Workflow::parse("name: placed\nsteps:\n  - id: scan\n    agent: probe\n    prompt: go\n    host: worker-1\n").unwrap();
        assert!(matches!(
            WorkflowRuntime::new(request),
            Err(rupu_orchestrator::WorkflowRuntimeError::StrictRemotePlacement)
        ));

        let request = inputs(dir.path(), (!use_marker).then_some(policy_link));
        let runtime = WorkflowRuntime::new(request).unwrap();
        assert!(
            runtime
                .action_services
                .context
                .identity
                .launch
                .strict_approval
        );
    }
}

#[test]
fn ordinary_headless_ask_workflow_keeps_legacy_behavior_and_policy_corruption_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = inputs(dir.path(), None);
    request.mode = PermissionMode::Ask;
    let runtime = WorkflowRuntime::new(request).unwrap();
    assert!(
        !runtime
            .action_services
            .context
            .identity
            .launch
            .strict_approval
    );
    let run_dir = dir.path().join("global/runs/run_child");
    rupu_runtime::strict_approval::require_workflow(&run_dir).unwrap();
    std::fs::write(run_dir.join("strict-approval"), "corrupt").unwrap();
    assert!(matches!(
        WorkflowRuntime::new(inputs(dir.path(), None)),
        Err(rupu_orchestrator::WorkflowRuntimeError::ApprovalPolicy(_))
    ));
}

#[test]
fn in_memory_workflow_without_durable_identity_remains_supported() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = inputs(dir.path(), None);
    request.run_id.clear();
    request.mode = PermissionMode::Ask;
    let runtime = WorkflowRuntime::new(request).unwrap();
    assert!(
        !runtime
            .action_services
            .context
            .identity
            .launch
            .strict_approval
    );
}
