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
    assert_eq!(wf.steps[0].agent.as_deref(), Some("investigator"));
    assert_eq!(wf.steps[0].actions, vec!["log_finding".to_string()]);
    assert!(wf.steps[1]
        .prompt
        .as_deref()
        .unwrap()
        .contains("Propose a minimal fix"));
}

#[test]
fn accepts_parallel_block() {
    let s = r#"
name: x
steps:
  - id: triage
    actions: []
    parallel:
      - id: sec
        agent: security-reviewer
        prompt: "review {{ inputs.diff }}"
      - id: perf
        agent: perf-reviewer
        prompt: "review {{ inputs.diff }}"
"#;
    let wf = Workflow::parse(s).expect("parallel: should parse");
    let subs = wf.steps[0].parallel.as_ref().unwrap();
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0].id, "sec");
    assert_eq!(subs[1].agent, "perf-reviewer");
    assert!(wf.steps[0].agent.is_none());
    assert!(wf.steps[0].prompt.is_none());
}

#[test]
fn rejects_parallel_combined_with_for_each_or_agent() {
    let s = r#"
name: x
steps:
  - id: triage
    agent: a
    prompt: hi
    actions: []
    parallel:
      - { id: x, agent: a, prompt: y }
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("mutually exclusive"),
        "expected mutually-exclusive error, got: {err}"
    );
}

#[test]
fn rejects_empty_parallel_block() {
    let s = "name: x\nsteps:\n  - id: a\n    actions: []\n    parallel: []\n";
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("at least one sub-step"),
        "expected empty-parallel error, got: {err}"
    );
}

#[test]
fn rejects_duplicate_sub_step_id() {
    let s = r#"
name: x
steps:
  - id: triage
    actions: []
    parallel:
      - { id: dup, agent: a, prompt: p }
      - { id: dup, agent: b, prompt: q }
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("duplicate sub-step id"),
        "expected duplicate-sub-step error, got: {err}"
    );
}

#[test]
fn rejects_linear_step_without_agent() {
    let s = "name: x\nsteps:\n  - id: a\n    actions: []\n    prompt: hi\n";
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("agent") && err.contains("required"),
        "expected missing-agent error, got: {err}"
    );
}

#[test]
fn accepts_approval_block_on_step() {
    let s = r#"
name: x
steps:
  - id: deploy
    agent: deployer
    actions: []
    approval:
      required: true
      prompt: "About to deploy {{ inputs.tag }}. Approve?"
      timeout_seconds: 3600
    prompt: "go"
"#;
    let wf = Workflow::parse(s).expect("approval: should parse");
    let approval = wf.steps[0].approval.as_ref().unwrap();
    assert!(approval.required);
    assert_eq!(
        approval.prompt.as_deref(),
        Some("About to deploy {{ inputs.tag }}. Approve?")
    );
    assert_eq!(approval.timeout_seconds, Some(3600));
}

#[test]
fn approval_block_omitted_means_no_gate() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n";
    let wf = Workflow::parse(s).unwrap();
    assert!(wf.steps[0].approval.is_none());
}

#[test]
fn accepts_when_expression() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: a\n    when: \"{{ inputs.go }}\"\n    actions: []\n    prompt: hi\n";
    let wf = Workflow::parse(s).expect("when: should parse");
    assert_eq!(wf.steps[0].when.as_deref(), Some("{{ inputs.go }}"));
}

#[test]
fn accepts_for_each_with_max_parallel() {
    let s = r#"
name: x
steps:
  - id: a
    agent: a
    actions: []
    for_each: "{{ inputs.files }}"
    max_parallel: 4
    prompt: "review {{ item }}"
"#;
    let wf = Workflow::parse(s).expect("for_each + max_parallel should parse");
    assert_eq!(wf.steps[0].for_each.as_deref(), Some("{{ inputs.files }}"));
    assert_eq!(wf.steps[0].max_parallel, Some(4));
}

#[test]
fn rejects_max_parallel_zero() {
    let s = r#"
name: x
steps:
  - id: a
    agent: a
    actions: []
    for_each: "{{ inputs.files }}"
    max_parallel: 0
    prompt: "x"
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("max_parallel") && err.contains("at least 1"),
        "expected max_parallel validation error, got: {err}"
    );
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

#[test]
fn accepts_panel_step() {
    let s = r#"
name: x
steps:
  - id: panel
    actions: []
    panel:
      panelists:
        - security-reviewer
        - perf-reviewer
      subject: "{{ inputs.diff }}"
      max_parallel: 2
"#;
    let wf = Workflow::parse(s).expect("panel: should parse");
    let panel = wf.steps[0].panel.as_ref().unwrap();
    assert_eq!(panel.panelists.len(), 2);
    assert_eq!(panel.subject, "{{ inputs.diff }}");
    assert_eq!(panel.max_parallel, Some(2));
    assert!(panel.gate.is_none());
    assert!(wf.steps[0].agent.is_none());
    assert!(wf.steps[0].prompt.is_none());
}

#[test]
fn accepts_panel_with_gate() {
    let s = r#"
name: x
steps:
  - id: panel
    actions: []
    panel:
      panelists:
        - reviewer-a
      subject: "review me"
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: developer
        max_iterations: 3
"#;
    let wf = Workflow::parse(s).unwrap();
    let gate = wf.steps[0].panel.as_ref().unwrap().gate.as_ref().unwrap();
    assert_eq!(
        gate.until_no_findings_at_severity_or_above,
        rupu_orchestrator::Severity::High
    );
    assert_eq!(gate.fix_with, "developer");
    assert_eq!(gate.max_iterations, 3);
}

#[test]
fn rejects_panel_with_top_level_agent() {
    let s = r#"
name: x
steps:
  - id: panel
    agent: a
    prompt: hi
    actions: []
    panel:
      panelists: [reviewer-a]
      subject: "x"
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("mutually exclusive"),
        "expected panel mutually-exclusive error, got: {err}"
    );
}

#[test]
fn rejects_panel_with_empty_panelists() {
    let s = r#"
name: x
steps:
  - id: panel
    actions: []
    panel:
      panelists: []
      subject: "x"
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("at least one agent"),
        "expected panel-empty error, got: {err}"
    );
}

#[test]
fn rejects_panel_gate_max_iterations_zero() {
    let s = r#"
name: x
steps:
  - id: panel
    actions: []
    panel:
      panelists: [reviewer-a]
      subject: "x"
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: developer
        max_iterations: 0
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("max_iterations") && err.contains("at least 1"),
        "expected max_iterations error, got: {err}"
    );
}

#[test]
fn parses_notify_issue_field() {
    let s = r#"
name: x
notifyIssue: true
steps:
  - id: a
    agent: ag
    actions: []
    prompt: hi
"#;
    let wf = Workflow::parse(s).expect("notifyIssue should parse");
    assert!(wf.notify_issue);
}

#[test]
fn notify_issue_defaults_to_false() {
    let s = "name: x\nsteps:\n  - id: a\n    agent: ag\n    actions: []\n    prompt: hi\n";
    let wf = Workflow::parse(s).unwrap();
    assert!(!wf.notify_issue);
}

#[test]
fn parses_autoflow_and_contracts_blocks() {
    let s = r#"
name: issue-supervisor-dispatch
autoflow:
  enabled: true
  entity: issue
  priority: 100
  selector:
    states: [open]
    labels_all: [autoflow]
    limit: 100
  wake_on:
    - github.issue.opened
    - github.pr.merged
  reconcile_every: 10m
  claim:
    key: issue
    ttl: 3h
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: issue-understander
    actions: []
    contract:
      emits: autoflow_outcome_v1
      format: json
    prompt: "Return only valid JSON."
"#;
    let wf = Workflow::parse(s).expect("autoflow should parse");
    let autoflow = wf.autoflow.as_ref().expect("autoflow");
    assert!(autoflow.enabled);
    assert_eq!(autoflow.priority, 100);
    assert_eq!(autoflow.selector.labels_all, vec!["autoflow"]);
    assert_eq!(autoflow.reconcile_every.as_deref(), Some("10m"));
    assert_eq!(
        wf.contracts.outputs["result"].schema,
        "autoflow_outcome_v1".to_string()
    );
}

#[test]
fn autoflow_priority_defaults_to_zero() {
    let s = r#"
name: issue-supervisor-dispatch
autoflow:
  enabled: true
  entity: issue
steps:
  - id: decide
    agent: a
    actions: []
    prompt: hi
"#;
    let wf = Workflow::parse(s).expect("parse");
    assert_eq!(wf.autoflow.as_ref().unwrap().priority, 0);
}

#[test]
fn rejects_invalid_autoflow_duration() {
    let s = r#"
name: issue-supervisor-dispatch
autoflow:
  enabled: true
  entity: issue
  reconcile_every: daily
steps:
  - id: decide
    agent: a
    actions: []
    prompt: hi
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(err.contains("invalid duration"), "got: {err}");
}

#[test]
fn rejects_contract_output_referencing_missing_step() {
    let s = r#"
name: issue-supervisor-dispatch
contracts:
  outputs:
    result:
      from_step: finalize
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: a
    actions: []
    prompt: hi
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(err.contains("unknown step `finalize`"), "got: {err}");
}

#[test]
fn rejects_autoflow_outcome_referencing_unknown_output() {
    let s = r#"
name: issue-supervisor-dispatch
autoflow:
  enabled: true
  entity: issue
  outcome:
    output: result
steps:
  - id: decide
    agent: a
    actions: []
    prompt: hi
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(err.contains("unknown workflow output"), "got: {err}");
}

#[test]
fn rejects_step_contract_mismatch_with_workflow_output() {
    let s = r#"
name: issue-supervisor-dispatch
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: a
    actions: []
    contract:
      emits: workflow_dispatch_v1
      format: json
    prompt: hi
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(err.contains("contract disagree on `schema`"), "got: {err}");
}
