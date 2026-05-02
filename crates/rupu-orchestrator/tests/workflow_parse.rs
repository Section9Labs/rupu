use rupu_orchestrator::Workflow;

const SIMPLE: &str = r#"
name: investigate-then-fix
description: Investigate a bug then propose a fix.
steps:
  - id: investigate
    agent: investigator
    actions:
      - log_finding
    prompt: |
      Investigate the bug: {{ inputs.prompt }}
  - id: propose
    agent: fixer
    actions:
      - propose_edit
    prompt: |
      Based on:
      {{ steps.investigate.output }}
      Propose a minimal fix.
"#;

#[test]
fn parses_two_step_linear_workflow() {
    let wf = Workflow::parse(SIMPLE).unwrap();
    assert_eq!(wf.name, "investigate-then-fix");
    assert_eq!(wf.steps.len(), 2);
    assert_eq!(wf.steps[0].id, "investigate");
    assert_eq!(wf.steps[0].agent, "investigator");
    assert_eq!(wf.steps[0].actions, vec!["log_finding".to_string()]);
    assert!(wf.steps[1].prompt.contains("Propose a minimal fix"));
}

#[test]
fn rejects_parallel_keyword() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: a\n    parallel: [b, c]\n    actions: []\n    prompt: hi\n";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(
        err.contains("parallel"),
        "expected unsupported-key error, got: {err}"
    );
}

#[test]
fn rejects_when_keyword() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: a\n    when: someday\n    actions: []\n    prompt: hi\n";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(
        err.contains("when"),
        "expected unsupported-key error, got: {err}"
    );
}

#[test]
fn rejects_gates_keyword() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: a\n    gates: [approval]\n    actions: []\n    prompt: hi\n";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(
        err.contains("gates"),
        "expected unsupported-key error, got: {err}"
    );
}

#[test]
fn empty_steps_list_is_an_error() {
    let s = "name: x\nsteps: []\n";
    assert!(Workflow::parse(s).is_err());
}

#[test]
fn step_id_must_be_unique() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: ag\n    actions: []\n    prompt: hi\n  - id: a\n    agent: ag\n    actions: []\n    prompt: hi2\n";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(
        err.contains("duplicate"),
        "expected duplicate-id error: {err}"
    );
}
