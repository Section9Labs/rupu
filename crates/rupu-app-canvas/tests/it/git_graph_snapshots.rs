//! Snapshot tests for rupu-app-canvas's git-graph row emitter.
//!
//! Each test parses a representative workflow YAML, runs
//! `render_rows`, and snapshots the result via insta. The .snap
//! files are committed alongside the test so visual changes show
//! up as PR diffs.

use rupu_app_canvas::render_rows;
use rupu_orchestrator::Workflow;

fn fixture(yaml: &str) -> Workflow {
    Workflow::parse(yaml).expect("fixture workflow parses")
}

#[test]
fn snapshot_linear_3_steps() {
    let wf = fixture(
        r#"
name: linear3
steps:
  - id: classify
    agent: classifier
    actions: []
    prompt: hi
  - id: review
    agent: reviewer
    actions: []
    prompt: hi
  - id: publish
    agent: publisher
    actions: []
    prompt: hi
"#,
    );
    let rows = render_rows(&wf, |_| rupu_app_canvas::NodeStatus::Waiting);
    insta::assert_yaml_snapshot!("linear_3_steps", rows);
}

#[test]
fn snapshot_panel_with_3_panelists() {
    let wf = fixture(
        r#"
name: review
steps:
  - id: classify
    agent: classifier
    actions: []
    prompt: hi
  - id: review_panel
    actions: []
    panel:
      subject: review
      panelists:
        - security-reviewer
        - perf-reviewer
        - style-reviewer
  - id: aggregate
    agent: findings-aggregator
    actions: []
    prompt: hi
"#,
    );
    let rows = render_rows(&wf, |_| rupu_app_canvas::NodeStatus::Waiting);
    insta::assert_yaml_snapshot!("panel_with_3_panelists", rows);
}

#[test]
fn snapshot_single_linear_step() {
    let wf = fixture(
        r#"
name: single
steps:
  - id: hello
    agent: greeter
    actions: []
    prompt: hi
"#,
    );
    let rows = render_rows(&wf, |_| rupu_app_canvas::NodeStatus::Waiting);
    insta::assert_yaml_snapshot!("single_linear_step", rows);
}

#[test]
fn snapshot_standalone_approval_gate() {
    let wf = fixture(
        r#"
name: gate-flow
steps:
  - id: classify
    agent: classifier
    actions: []
    prompt: hi
  - id: ship_gate
    approval:
      required: true
      prompt: "Ship this to prod?"
    actions: []
  - id: publish
    agent: publisher
    actions: []
    prompt: hi
"#,
    );
    let rows = render_rows(&wf, |_| rupu_app_canvas::NodeStatus::Waiting);
    insta::assert_yaml_snapshot!("standalone_approval_gate", rows);
}

#[test]
fn snapshot_standalone_approval_gate_with_auto_approve() {
    let wf = fixture(
        r#"
name: auto-gate-flow
steps:
  - id: classify
    agent: classifier
    actions: []
    prompt: hi
  - id: ship_gate
    approval:
      required: true
      prompt: "Ship this to prod?"
      auto_approve: "{{ inputs.trusted }}"
    actions: []
"#,
    );
    let rows = render_rows(&wf, |_| rupu_app_canvas::NodeStatus::Waiting);
    insta::assert_yaml_snapshot!("standalone_approval_gate_with_auto_approve", rows);
}

#[test]
fn snapshot_action_step() {
    let wf = fixture(
        r#"
name: action-flow
steps:
  - id: classify
    agent: classifier
    actions: []
    prompt: hi
  - id: open_pr
    action: scm.prs.create
    with:
      owner: acme
      repo: widgets
      title: "Automated PR"
      body: "Opened by workflow"
      head: feature-branch
      base: main
    actions: []
"#,
    );
    let rows = render_rows(&wf, |_| rupu_app_canvas::NodeStatus::Waiting);
    insta::assert_yaml_snapshot!("action_step", rows);
}

#[test]
fn snapshot_panel_with_one_active_step() {
    let yaml = r#"
name: live-snapshot
steps:
  - id: classify
    agent: classifier
    prompt: "go"
  - id: review_panel
    panel:
      panelists: [sec, perf]
      subject: "review"
    actions: []
"#;
    let wf = rupu_orchestrator::Workflow::parse(yaml).expect("parse");
    let rows = rupu_app_canvas::render_rows(&wf, |id| match id {
        "classify" => rupu_app_canvas::NodeStatus::Complete,
        "review_panel" => rupu_app_canvas::NodeStatus::Active,
        "sec" => rupu_app_canvas::NodeStatus::Working,
        _ => rupu_app_canvas::NodeStatus::Waiting,
    });
    insta::assert_yaml_snapshot!(rows);
}

#[test]
fn snapshot_branch_with_then_and_else_arms() {
    // The not-taken arm is Skipped; the taken arm Complete — the lanes
    // are coloured by their target so taken vs skipped is visible.
    let wf = fixture(
        r#"
name: branchy
steps:
  - id: classify
    agent: classifier
    prompt: hi
    next: [route]
  - id: route
    branch:
      condition: "{{ steps.classify.output == 'bug' }}"
      then: [fix, notify]
      else: [close]
  - id: fix
    agent: fixer
    prompt: hi
  - id: notify
    agent: notifier
    prompt: hi
  - id: close
    agent: closer
    prompt: hi
"#,
    );
    let rows = render_rows(&wf, |id| match id {
        "classify" | "route" | "fix" | "notify" => rupu_app_canvas::NodeStatus::Complete,
        "close" => rupu_app_canvas::NodeStatus::Skipped,
        _ => rupu_app_canvas::NodeStatus::Waiting,
    });
    insta::assert_yaml_snapshot!("branch_with_then_and_else_arms", rows);
}

#[test]
fn snapshot_loop_framing_members() {
    let wf = fixture(
        r#"
name: looped
steps:
  - id: seed
    agent: seeder
    prompt: hi
    next: [gen]
  - id: gen
    agent: generator
    prompt: hi
  - id: test
    agent: tester
    prompt: hi
    depends_on: [gen]
  - id: critique
    agent: critic
    prompt: hi
    depends_on: [test]
  - id: ship
    agent: shipper
    prompt: hi
    depends_on: [critique]
loops:
  refine:
    nodes: [gen, test, critique]
    until: "{{ steps.critique.output }}"
    max_iterations: 5
"#,
    );
    let rows = render_rows(&wf, |id| match id {
        "seed" | "gen" => rupu_app_canvas::NodeStatus::Complete,
        "test" => rupu_app_canvas::NodeStatus::Working,
        _ => rupu_app_canvas::NodeStatus::Waiting,
    });
    insta::assert_yaml_snapshot!("loop_framing_members", rows);
}
