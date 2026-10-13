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
