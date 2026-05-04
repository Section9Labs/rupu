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
fn accepts_when_expression() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: a\n    when: \"{{ inputs.go }}\"\n    actions: []\n    prompt: hi\n";
    let wf = Workflow::parse(s).expect("when: should parse");
    assert_eq!(wf.steps[0].when.as_deref(), Some("{{ inputs.go }}"));
}

#[test]
fn accepts_continue_on_error_per_step() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: a\n    continue_on_error: true\n    actions: []\n    prompt: hi\n";
    let wf = Workflow::parse(s).expect("continue_on_error: should parse");
    assert_eq!(wf.steps[0].continue_on_error, Some(true));
}

#[test]
fn accepts_typed_inputs_block() {
    let s = "name: x\ninputs:\n  threshold:\n    type: int\n    default: 30\n  mode:\n    type: string\n    enum: [strict, lax]\n    default: strict\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n";
    let wf = Workflow::parse(s).expect("inputs: should parse");
    assert_eq!(wf.inputs.len(), 2);
    assert_eq!(
        wf.inputs["threshold"].ty,
        rupu_orchestrator::workflow::InputType::Int
    );
    assert_eq!(wf.inputs["mode"].allowed, vec!["strict", "lax"]);
}

#[test]
fn rejects_input_default_with_wrong_type() {
    let s = "name: x\ninputs:\n  threshold:\n    type: int\n    default: \"thirty\"\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(
        err.contains("default") && err.contains("int"),
        "expected default-type error, got: {err}"
    );
}

#[test]
fn rejects_input_default_not_in_enum() {
    let s = "name: x\ninputs:\n  mode:\n    type: string\n    enum: [strict, lax]\n    default: chaos\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(
        err.contains("enum"),
        "expected enum-mismatch error, got: {err}"
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

#[test]
fn defaults_to_manual_trigger_when_omitted() {
    let s = "name: x
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let wf = Workflow::parse(s).expect("parse");
    assert_eq!(wf.trigger.on, rupu_orchestrator::TriggerKind::Manual);
    assert!(wf.trigger.cron.is_none());
    assert!(wf.trigger.event.is_none());
}

#[test]
fn parses_cron_trigger_with_valid_expression() {
    let s = "name: x
trigger:
  on: cron
  cron: \"0 4 * * *\"
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let wf = Workflow::parse(s).expect("parse");
    assert_eq!(wf.trigger.on, rupu_orchestrator::TriggerKind::Cron);
    assert_eq!(wf.trigger.cron.as_deref(), Some("0 4 * * *"));
}

#[test]
fn rejects_cron_trigger_without_cron_field() {
    let s = "name: x
trigger:
  on: cron
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(
        err.contains("cron") && err.contains("requires"),
        "got: {err}"
    );
}

#[test]
fn rejects_cron_with_wrong_field_count() {
    let s = "name: x
trigger:
  on: cron
  cron: \"0 4 * *\"
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(err.contains("5 fields"), "got: {err}");
}

#[test]
fn rejects_cron_with_invalid_chars() {
    let s = "name: x
trigger:
  on: cron
  cron: \"0 4 * * ;\"
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(err.contains("invalid characters"), "got: {err}");
}

#[test]
fn parses_event_trigger_with_filter() {
    let s = r#"name: x
trigger:
  on: event
  event: github.pr.opened
  filter: "{{event.repo.name == 'rupu'}}"
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
"#;
    let wf = Workflow::parse(s).expect("parse");
    assert_eq!(wf.trigger.on, rupu_orchestrator::TriggerKind::Event);
    assert_eq!(wf.trigger.event.as_deref(), Some("github.pr.opened"));
    assert!(wf.trigger.filter.is_some());
}

#[test]
fn rejects_event_trigger_without_event_field() {
    let s = "name: x
trigger:
  on: event
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(
        err.contains("event") && err.contains("requires"),
        "got: {err}"
    );
}

#[test]
fn rejects_extraneous_fields_for_kind() {
    // manual + cron field
    let s = "name: x
trigger:
  on: manual
  cron: \"0 4 * * *\"
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(err.contains("manual") && err.contains("cron"), "got: {err}");
    // cron + event field
    let s = "name: x
trigger:
  on: cron
  cron: \"0 4 * * *\"
  event: x
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(err.contains("cron") && err.contains("event"), "got: {err}");
    // event + cron field
    let s = "name: x
trigger:
  on: event
  event: x
  cron: \"0 4 * * *\"
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
";
    let err = format!("{}", Workflow::parse(s).unwrap_err());
    assert!(err.contains("event") && err.contains("cron"), "got: {err}");
}
