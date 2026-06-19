use rupu_cp::api::graph::{to_step_dag, GateDto, StepDag, StepNodeDto, SubStepDto};
use rupu_orchestrator::Workflow;

/// YAML fixture exercising all four step kinds:
///   1. plain `step`      (linear — agent + prompt, no fan-out)
///   2. `parallel`        (parallel sub-steps block)
///   3. `for_each`        (data fan-out with for_each + agent + prompt)
///   4. `panel`           (panel block with panelists + gate)
const FIXTURE: &str = r#"
name: dag-test
steps:
  - id: plain-step
    agent: analyst
    actions: []
    prompt: "Analyse {{ inputs.target }}"

  - id: parallel-step
    actions: []
    parallel:
      - id: sec
        agent: security-reviewer
        prompt: "Review {{ inputs.diff }}"
      - id: perf
        agent: perf-reviewer
        prompt: "Review {{ inputs.diff }}"

  - id: foreach-step
    agent: file-reviewer
    actions: []
    for_each: "{{ inputs.files }}"
    prompt: "Review {{ item }}"

  - id: panel-step
    actions: []
    panel:
      panelists:
        - reviewer-a
        - reviewer-b
      subject: "review me"
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: developer
        max_iterations: 3
"#;

fn parsed_dag() -> StepDag {
    let wf = Workflow::parse(FIXTURE).expect("fixture YAML must parse");
    to_step_dag(&wf)
}

#[test]
fn kinds_are_in_order() {
    let dag = parsed_dag();
    let kinds: Vec<&str> = dag.steps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, vec!["step", "parallel", "for_each", "panel"]);
}

#[test]
fn step_node_ids_match_yaml() {
    let dag = parsed_dag();
    let ids: Vec<&str> = dag.steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["plain-step", "parallel-step", "foreach-step", "panel-step"]
    );
}

#[test]
fn linear_step_has_agent() {
    let dag = parsed_dag();
    let node = dag.steps.iter().find(|s| s.id == "plain-step").unwrap();
    assert_eq!(node.agent.as_deref(), Some("analyst"));
    assert!(node.parallel.is_none());
    assert!(node.panelists.is_none());
    assert!(node.gate.is_none());
    assert!(node.for_each.is_none());
}

#[test]
fn parallel_step_captures_sub_steps() {
    let dag = parsed_dag();
    let node = dag
        .steps
        .iter()
        .find(|s| s.id == "parallel-step")
        .unwrap();
    assert_eq!(node.kind, "parallel");
    let subs = node.parallel.as_ref().expect("parallel field present");
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0].id, "sec");
    assert_eq!(subs[0].agent, "security-reviewer");
    assert_eq!(subs[1].id, "perf");
    assert_eq!(subs[1].agent, "perf-reviewer");
    assert!(node.agent.is_none());
    assert!(node.panelists.is_none());
    assert!(node.gate.is_none());
}

#[test]
fn for_each_step_captures_expression() {
    let dag = parsed_dag();
    let node = dag
        .steps
        .iter()
        .find(|s| s.id == "foreach-step")
        .unwrap();
    assert_eq!(node.kind, "for_each");
    assert_eq!(node.for_each.as_deref(), Some("{{ inputs.files }}"));
    assert_eq!(node.agent.as_deref(), Some("file-reviewer"));
    assert!(node.parallel.is_none());
    assert!(node.panelists.is_none());
    assert!(node.gate.is_none());
}

#[test]
fn panel_step_captures_panelists_and_gate() {
    let dag = parsed_dag();
    let node = dag.steps.iter().find(|s| s.id == "panel-step").unwrap();
    assert_eq!(node.kind, "panel");

    let panelists = node.panelists.as_ref().expect("panelists present");
    assert_eq!(panelists, &["reviewer-a", "reviewer-b"]);

    let gate = node.gate.as_ref().expect("gate present");
    assert_eq!(gate.max_iterations, 3);
    assert_eq!(gate.until_severity, "high");
    assert_eq!(gate.fix_with, "developer");

    assert!(node.agent.is_none());
    assert!(node.parallel.is_none());
    assert!(node.for_each.is_none());
}

#[test]
fn dto_types_are_serialize() {
    // Confirm the DTOs can actually be serialized to JSON without panic.
    let dag = parsed_dag();
    let json = serde_json::to_string(&dag).expect("StepDag must serialize");
    assert!(json.contains("parallel-step"));
    assert!(json.contains("security-reviewer"));
    assert!(json.contains("reviewer-a"));
    assert!(json.contains("\"until_severity\":\"high\""));
}

// ── Compile-time check: public type aliases are reachable ──────────────
fn _type_check() {
    let _: StepNodeDto;
    let _: SubStepDto;
    let _: GateDto;
}
