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
    let rows = render_rows(&wf);
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
    let rows = render_rows(&wf);
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
    let rows = render_rows(&wf);
    insta::assert_yaml_snapshot!("single_linear_step", rows);
}
