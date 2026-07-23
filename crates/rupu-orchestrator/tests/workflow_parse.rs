use rupu_orchestrator::{is_approval_gate, TimeoutAction, Workflow};

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
fn accepts_input_description_field() {
    // `description:` is a free-form metadata field — common in
    // GitHub Actions / Argo / etc. Authors drop it in to document
    // each input; the runtime ignores it. Pre-fix it would trip the
    // `deny_unknown_fields` guard on `InputDef`.
    let s = "name: x\ninputs:\n  subject:\n    type: string\n    required: true\n    description: \"Path to review.\"\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n";
    let wf = Workflow::parse(s).expect("inputs: with description should parse");
    assert_eq!(
        wf.inputs["subject"].description.as_deref(),
        Some("Path to review.")
    );
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
fn rejects_branch_combined_with_agent_prompt_for_each_parallel_panel() {
    let combos = [
        "agent: a\n    prompt: hi",
        "for_each: \"{{ inputs.items }}\"\n    agent: a\n    prompt: hi",
        "parallel:\n      - { id: x, agent: a, prompt: y }",
        "panel:\n      panelists: [reviewer-a]\n      subject: \"x\"",
    ];
    for combo in combos {
        let s = format!(
            r#"
name: x
steps:
  - id: gate
    actions: []
    {combo}
    branch:
      condition: "{{{{ steps.gate.output }}}}"
      then: [a]
      else: [b]
  - id: a
    agent: ag
    prompt: p
  - id: b
    agent: ag
    prompt: p
"#
        );
        let err = Workflow::parse(&s).unwrap_err().to_string();
        assert!(
            err.contains("mutually exclusive"),
            "combo `{combo}` expected mutually-exclusive error, got: {err}"
        );
    }
}

#[test]
fn rejects_branch_with_empty_condition() {
    let s = r#"
name: x
steps:
  - id: gate
    actions: []
    branch:
      condition: "   "
      then: [a]
      else: [b]
  - id: a
    agent: ag
    prompt: p
  - id: b
    agent: ag
    prompt: p
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("condition"),
        "expected empty-condition error, got: {err}"
    );
}

#[test]
fn rejects_branch_with_when() {
    // `when:` is evaluated before the branch block by the runner. A
    // branch step with a falsy `when:` would be when:-skipped without
    // ever populating either arm's skip-set, so both arms would run —
    // a silent correctness bug. Reject `when:` on branch steps outright.
    let s = r#"
name: x
steps:
  - id: gate
    actions: []
    when: "true"
    branch:
      condition: "true"
      then: [a]
      else: [b]
  - id: a
    agent: ag
    prompt: p
  - id: b
    agent: ag
    prompt: p
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("when") && err.contains("not allowed") && err.contains("branch"),
        "expected when-on-branch error, got: {err}"
    );
}

#[test]
fn rejects_branch_arms_overlap() {
    let s = r#"
name: x
steps:
  - id: gate
    actions: []
    branch:
      condition: "{{ steps.gate.output }}"
      then: [a, shared]
      else: [b, shared]
  - id: a
    agent: ag
    prompt: p
  - id: b
    agent: ag
    prompt: p
  - id: shared
    agent: ag
    prompt: p
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("shared"),
        "expected arms-overlap error naming `shared`, got: {err}"
    );
}

#[test]
fn rejects_branch_targeting_self() {
    let s = r#"
name: x
steps:
  - id: gate
    actions: []
    branch:
      condition: "{{ steps.gate.output }}"
      then: [gate]
      else: [b]
  - id: b
    agent: ag
    prompt: p
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("gate"),
        "expected self-target error naming `gate`, got: {err}"
    );
}

#[test]
fn accepts_valid_branch_step() {
    let s = r#"
name: x
steps:
  - id: gate
    actions: []
    branch:
      condition: "true"
      then: [a]
      else: [b]
  - id: a
    agent: ag
    prompt: p
  - id: b
    agent: ag
    prompt: p
"#;
    let wf = Workflow::parse(s).expect("valid branch step should parse");
    let branch = wf.steps[0].branch.as_ref().unwrap();
    assert_eq!(branch.then, vec!["a".to_string()]);
    assert_eq!(branch.r#else, vec!["b".to_string()]);
    assert!(wf.steps[0].agent.is_none());
    assert!(wf.steps[0].prompt.is_none());
}

#[test]
fn rejects_branch_target_unknown() {
    let s = r#"
name: x
steps:
  - id: gate
    actions: []
    branch:
      condition: "true"
      then: [nope]
      else: [b]
  - id: b
    agent: ag
    prompt: p
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("nope"),
        "expected BranchTargetUnknown naming `nope`, got: {err}"
    );
}

#[test]
fn rejects_branch_target_not_forward() {
    let s = r#"
name: x
steps:
  - id: before
    agent: ag
    prompt: p
  - id: gate
    actions: []
    branch:
      condition: "true"
      then: [before]
      else: [after]
  - id: after
    agent: ag
    prompt: p
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("before"),
        "expected BranchTargetNotForward naming `before`, got: {err}"
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
  source: linear:72b2a2dc-6f4f-4423-9d34-24b5bd10634a
  priority: 100
  selector:
    states: [open]
    labels_all: [autoflow]
    labels_any: [bug, urgent]
    labels_none: [blocked]
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
    assert_eq!(
        autoflow.source.as_deref(),
        Some("linear:72b2a2dc-6f4f-4423-9d34-24b5bd10634a")
    );
    assert_eq!(autoflow.priority, 100);
    assert_eq!(autoflow.selector.labels_all, vec!["autoflow"]);
    assert_eq!(autoflow.selector.labels_any, vec!["bug", "urgent"]);
    assert_eq!(autoflow.selector.labels_none, vec!["blocked"]);
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

// ── Template-reference lint ───────────────────────────────────────────

#[test]
fn lint_accepts_valid_step_field_reference() {
    let s = r#"
name: x
steps:
  - id: a
    agent: w
    actions: []
    prompt: hello
  - id: b
    agent: w
    actions: []
    prompt: |
      prior output: {{ steps.a.output }}
      success: {{ steps.a.success }}
"#;
    Workflow::parse(s).expect("valid step.field reference should parse");
}

#[test]
fn lint_rejects_unknown_step_reference() {
    let s = r#"
name: x
steps:
  - id: a
    agent: w
    actions: []
    prompt: |
      missing: {{ steps.does_not_exist.output }}
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("steps.does_not_exist") && err.contains("no step with that id"),
        "expected unknown-step error, got: {err}"
    );
}

#[test]
fn lint_rejects_forward_step_reference() {
    let s = r#"
name: x
steps:
  - id: a
    agent: w
    actions: []
    prompt: |
      from later: {{ steps.b.output }}
  - id: b
    agent: w
    actions: []
    prompt: hello
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("steps.b") && err.contains("forward reference"),
        "expected forward-reference error, got: {err}"
    );
}

#[test]
fn lint_rejects_unknown_step_field() {
    let s = r#"
name: x
steps:
  - id: a
    agent: w
    actions: []
    prompt: hi
  - id: b
    agent: w
    actions: []
    prompt: |
      typo: {{ steps.a.outupt }}
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("steps.a.outupt") && err.contains("not a known step-output field"),
        "expected unknown-field error, got: {err}"
    );
}

#[test]
fn lint_accepts_deeper_paths_through_known_field() {
    // sub_results.<sub_id>.output — first two segments validate
    // (sub_results is a known field), deeper segments aren't checked.
    let s = r#"
name: x
steps:
  - id: review
    actions: []
    parallel:
      - id: sec
        agent: a
        prompt: hello
      - id: perf
        agent: a
        prompt: hello
  - id: summary
    agent: w
    actions: []
    prompt: |
      sec: {{ steps.review.sub_results.sec.output }}
      perf: {{ steps.review.sub_results.perf.output }}
"#;
    Workflow::parse(s).expect("deep paths through known fields should pass");
}

#[test]
fn lint_validates_panel_subject_template() {
    // Panel `subject:` is a templated string — references inside it
    // must validate the same way prompts do.
    let s = r#"
name: x
steps:
  - id: review
    actions: []
    panel:
      panelists: [a, b]
      subject: "from earlier: {{ steps.nope.output }}"
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("steps.nope") && err.contains("no step with that id"),
        "expected lint to walk panel.subject, got: {err}"
    );
}

#[test]
fn lint_validates_when_template() {
    let s = r#"
name: x
steps:
  - id: a
    agent: w
    actions: []
    prompt: hi
  - id: b
    when: "{{ steps.a.maxx_severity == 'critical' }}"
    agent: w
    actions: []
    prompt: hi
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("steps.a.maxx_severity"),
        "expected when: lint to fire, got: {err}"
    );
}

#[test]
fn lint_validates_branch_condition_unknown_field() {
    let s = r#"
name: x
steps:
  - id: a
    agent: w
    actions: []
    prompt: hi
  - id: gate
    actions: []
    branch:
      condition: "{{ steps.a.outupt }}"
      then: [b]
      else: [c]
  - id: b
    agent: w
    prompt: p
  - id: c
    agent: w
    prompt: p
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("steps.a.outupt") && err.contains("not a known step-output field"),
        "expected branch.condition unknown-field lint to fire, got: {err}"
    );
}

#[test]
fn lint_rejects_branch_condition_forward_reference() {
    let s = r#"
name: x
steps:
  - id: gate
    actions: []
    branch:
      condition: "{{ steps.b.output }}"
      then: [a]
      else: [b]
  - id: a
    agent: w
    prompt: p
  - id: b
    agent: w
    prompt: p
"#;
    let err = Workflow::parse(s).unwrap_err().to_string();
    assert!(
        err.contains("steps.b") && err.contains("forward reference"),
        "expected branch.condition forward-ref lint to fire, got: {err}"
    );
}

const GATE_WORKFLOW: &str = r#"
name: gate-demo
steps:
  - id: review
    agent: reviewer
    prompt: "review it"
  - id: merge_gate
    approval:
      prompt: "Approve to open the PR."
      auto_approve: "{{ steps.review.output == 'clean' }}"
      timeout_seconds: 86400
      on_timeout: reject
      on_reject:
        - id: note_rejection
          agent: issue-commenter
          prompt: "note the rejection"
"#;

#[test]
fn approval_gate_node_parses() {
    let wf = Workflow::parse(GATE_WORKFLOW).unwrap();
    let gate = &wf.steps[1];
    assert!(is_approval_gate(gate));
    let ap = gate.approval.as_ref().unwrap();
    assert_eq!(ap.auto_approve.as_deref(), Some("{{ steps.review.output == 'clean' }}"));
    assert_eq!(ap.on_timeout, Some(TimeoutAction::Reject));
    assert_eq!(ap.on_reject.len(), 1);
    assert_eq!(ap.on_reject[0].id, "note_rejection");
}

#[test]
fn inline_approval_option_is_not_a_gate_node() {
    // Legacy shape: approval alongside agent+prompt keeps its old meaning.
    let yaml = r#"
name: legacy
steps:
  - id: deploy
    agent: deployer
    prompt: "deploy"
    approval:
      required: true
"#;
    let wf = Workflow::parse(yaml).unwrap();
    assert!(!is_approval_gate(&wf.steps[0]));
}

#[test]
fn gate_node_rejects_agent_mixing() {
    let yaml = r#"
name: bad
steps:
  - id: g
    agent: someone
    approval:
      on_reject: []
"#;
    // agent + approval WITHOUT prompt: linear validation already fails on
    // missing prompt; the point is a gate-ish step with agent must error.
    assert!(Workflow::parse(yaml).is_err());
}

#[test]
fn gate_on_timeout_requires_timeout_seconds() {
    let yaml = r#"
name: bad
steps:
  - id: g
    approval:
      on_timeout: approve
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("on_timeout"), "got: {err}");
}

#[test]
fn gate_on_reject_forbids_nested_gates_and_fanout() {
    let yaml = r#"
name: bad
steps:
  - id: g
    approval:
      on_reject:
        - id: nested
          approval:
            prompt: "no"
"#;
    assert!(Workflow::parse(yaml).is_err());
    let yaml2 = r#"
name: bad2
steps:
  - id: g
    approval:
      on_reject:
        - id: fan
          agent: a
          for_each: "{{ steps.x.output }}"
          prompt: "p"
"#;
    assert!(Workflow::parse(yaml2).is_err());
}

#[test]
fn inline_approval_rejects_auto_approve() {
    let yaml = r#"
name: bad
steps:
  - id: deploy
    agent: deployer
    prompt: "deploy"
    approval:
      required: true
      auto_approve: "{{ true }}"
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("auto_approve"), "got: {err}");
}

#[test]
fn inline_approval_rejects_on_timeout() {
    let yaml = r#"
name: bad
steps:
  - id: deploy
    agent: deployer
    prompt: "deploy"
    approval:
      required: true
      timeout_seconds: 60
      on_timeout: approve
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("on_timeout"), "got: {err}");
}

#[test]
fn inline_approval_rejects_notify() {
    let yaml = r#"
name: bad
steps:
  - id: deploy
    agent: deployer
    prompt: "deploy"
    approval:
      required: true
      notify:
        - action: issues.comment
          with:
            body: "awaiting approval"
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("notify"), "got: {err}");
}

#[test]
fn inline_approval_rejects_on_reject() {
    let yaml = r#"
name: bad
steps:
  - id: deploy
    agent: deployer
    prompt: "deploy"
    approval:
      required: true
      on_reject:
        - id: cleanup
          agent: cleaner
          prompt: "clean up"
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("on_reject"), "got: {err}");
}

#[test]
fn inline_approval_plain_fields_still_parse() {
    let yaml = r#"
name: ok
steps:
  - id: deploy
    agent: deployer
    prompt: "deploy"
    approval:
      required: true
      prompt: "Approve the deploy?"
      timeout_seconds: 3600
"#;
    let wf = Workflow::parse(yaml).unwrap();
    assert!(!is_approval_gate(&wf.steps[0]));
    let ap = wf.steps[0].approval.as_ref().unwrap();
    assert!(ap.required);
    assert_eq!(ap.timeout_seconds, Some(3600));
}

#[test]
fn gate_on_reject_sub_rejects_host() {
    let yaml = r#"
name: bad
steps:
  - id: g
    approval:
      on_reject:
        - id: cleanup
          agent: cleaner
          prompt: "clean up"
          host: worker-1
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("host"), "got: {err}");
}

#[test]
fn gate_on_reject_sub_rejects_when() {
    let yaml = r#"
name: bad
steps:
  - id: g
    approval:
      on_reject:
        - id: cleanup
          agent: cleaner
          prompt: "clean up"
          when: "{{ true }}"
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("when"), "got: {err}");
}

#[test]
fn gate_node_rejects_host() {
    let yaml = r#"
name: bad
steps:
  - id: g
    approval:
      prompt: "approve?"
    host: worker-1
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("host"), "got: {err}");
}

#[test]
fn gate_on_reject_sub_id_collides_with_top_level_step() {
    let yaml = r#"
name: bad
steps:
  - id: cleanup
    agent: someone
    prompt: "p"
  - id: g
    approval:
      on_reject:
        - id: cleanup
          agent: cleaner
          prompt: "clean up"
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("cleanup"), "got: {err}");
}

#[test]
fn action_step_parses() {
    let yaml = r#"
name: act
steps:
  - id: open_pr
    action: scm.prs.create
    with:
      repo: "org/repo"
      title: "{{ inputs.title }}"
"#;
    let wf = Workflow::parse(yaml).unwrap();
    assert_eq!(wf.steps[0].action.as_deref(), Some("scm.prs.create"));
    assert!(wf.steps[0].with.is_some());
}

#[test]
fn action_step_rejects_agent_mixing() {
    let yaml = r#"
name: bad
steps:
  - id: x
    action: issues.comment
    agent: someone
    prompt: "p"
"#;
    assert!(Workflow::parse(yaml).is_err());
}

#[test]
fn action_step_rejects_empty_name() {
    let yaml = r#"
name: bad
steps:
  - id: x
    action: ""
"#;
    assert!(Workflow::parse(yaml).is_err());
}

#[test]
fn action_step_rejects_approval_mixing() {
    let yaml = r#"
name: bad
steps:
  - id: x
    action: issues.comment
    approval:
      required: true
"#;
    assert!(Workflow::parse(yaml).is_err());
}
