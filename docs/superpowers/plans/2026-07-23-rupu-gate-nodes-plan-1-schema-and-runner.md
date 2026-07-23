# Gate Nodes & Action Steps — Plan 1: Schema + Gate Runner

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** First PR of the 4-PR arc in `docs/superpowers/specs/2026-07-23-rupu-workflow-gate-and-action-nodes-design.md` — both new step shapes (`approval:`-standalone gate node, `action:` connector step) parse and validate; the gate node's full runtime lifecycle works (auto-approve, pause/approve, reject with inline cleanup chain, timeout routing); action steps parse but fail at runtime with an explicit "lands in the next release" error (execution is Plan 2).

**Architecture:** Follow the `branch:` step precedent exactly — a non-agent shape gets its own arm in `validate_step_shape`, its own `StepKind` variant, and its own block in the runner's step loop that emits `step_started`/`step_completed` itself and `continue`s past the agent-dispatch machinery. Gate pause/resume reuses the existing `PauseReason::Approval` + `ResumeState::from_approval` machinery keyed by step id, untouched. Reject-with-cleanup re-enters `run_workflow` via a new `ResumeState::from_rejection` that executes only the gate's `on_reject` chain.

**Tech Stack:** Rust (rupu-orchestrator, rupu-cli), serde/serde_yaml, minijinja, existing MockProvider test harness.

## Global Constraints

- Workspace deps only; versions pinned in root `Cargo.toml` (CLAUDE.md rule 3).
- `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- Errors: `thiserror` in libraries (rupu-orchestrator), `anyhow` in rupu-cli.
- **No new `Event` enum variants and no removal/renaming of existing `StepKind` variants** — gates emit only existing events (`step_started`, `step_awaiting_approval`, `step_completed`, `step_failed`). Adding the `ApprovalGate` StepKind variant follows the `Branch` precedent (spec §4.3).
- Never run package-wide `cargo fmt` — per-file only (main is fmt-dirty under the pinned toolchain).
- Baseline caveat: 4 tests in `crates/rupu-orchestrator/tests/linear_runner.rs` are flaky on main (mock-provider "script exhausted"). They are NOT caused by this work — compare failures against a clean checkout before debugging.
- All paths below are relative to the repo root. Line numbers reference the tree at commit `a154584` (v0.59.3) — re-locate by the quoted code if drifted.

---

### Task 1: Schema — gate fields on `Approval`, new structs, gate shape validation

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (Approval struct ~line 653; `validate_step_shape` ~line 1033; `WorkflowParseError` ~line 21)
- Test: `crates/rupu-orchestrator/tests/workflow_parse.rs`

**Interfaces:**
- Produces: `Approval { required, prompt, timeout_seconds, auto_approve: Option<String>, on_timeout: Option<TimeoutAction>, notify: Vec<NotifyAction>, on_reject: Vec<Step> }`; `pub enum TimeoutAction { Approve, Reject, Fail }`; `pub struct NotifyAction { action: String, with: serde_json::Value }`; `pub fn is_approval_gate(step: &Step) -> bool`. Later tasks and Plans 2-4 rely on these exact names.

- [ ] **Step 1: Write the failing parse tests**

Append to `crates/rupu-orchestrator/tests/workflow_parse.rs` (match the file's existing test style — plain `Workflow::parse(yaml)` on inline YAML strings):

```rust
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
    let wf = rupu_orchestrator::workflow::Workflow::parse(GATE_WORKFLOW).unwrap();
    let gate = &wf.steps[1];
    assert!(rupu_orchestrator::workflow::is_approval_gate(gate));
    let ap = gate.approval.as_ref().unwrap();
    assert_eq!(ap.auto_approve.as_deref(), Some("{{ steps.review.output == 'clean' }}"));
    assert_eq!(ap.on_timeout, Some(rupu_orchestrator::workflow::TimeoutAction::Reject));
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
    let wf = rupu_orchestrator::workflow::Workflow::parse(yaml).unwrap();
    assert!(!rupu_orchestrator::workflow::is_approval_gate(&wf.steps[0]));
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
    assert!(rupu_orchestrator::workflow::Workflow::parse(yaml).is_err());
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
    let err = rupu_orchestrator::workflow::Workflow::parse(yaml).unwrap_err();
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
    assert!(rupu_orchestrator::workflow::Workflow::parse(yaml).is_err());
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
    assert!(rupu_orchestrator::workflow::Workflow::parse(yaml2).is_err());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator --test workflow_parse approval_gate 2>&1 | tail -5`
Expected: FAIL — `auto_approve` unknown field (deny_unknown_fields) / `is_approval_gate` not found.

- [ ] **Step 3: Implement the schema**

In `workflow.rs`, extend `Approval` (keep every existing field and doc comment; add after `timeout_seconds`):

```rust
    /// Minijinja expression evaluated when the gate is reached (same
    /// context as `prompt:`). Truthy ⇒ the gate resolves as approved
    /// (`via: auto`) without pausing. Only meaningful on a gate NODE
    /// (standalone `approval:` step); ignored on the legacy inline option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_approve: Option<String>,
    /// What a timed-out gate resolves to. Requires `timeout_seconds`.
    /// Absent ⇒ `fail` (today's behavior: run marked Failed on expiry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_timeout: Option<TimeoutAction>,
    /// Connector actions fired best-effort on entering AwaitingApproval
    /// (spec §3.1). Parsed in Plan 1; executed in Plan 4.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notify: Vec<NotifyAction>,
    /// Inline cleanup steps run after a reject decision, before the run
    /// ends Rejected. Linear agent steps and (from Plan 2) action steps
    /// only — no nested gates, no fan-out/panel/branch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_reject: Vec<Step>,
```

New types next to `Approval`:

```rust
/// Outcome routing for a timed-out gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeoutAction {
    Approve,
    Reject,
    Fail,
}

/// One notify hook on a gate: an SCM/issue/CI tool invocation, same
/// shape as an `action:` step's (`action`, `with`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyAction {
    pub action: String,
    #[serde(default)]
    pub with: serde_json::Value,
}
```

Gate detector (pub, exported from the crate root alongside `Workflow`):

```rust
/// A step is an approval GATE NODE when it has an `approval:` block and
/// no other shape (no agent/prompt/for_each/parallel/panel/branch/action).
/// `approval:` alongside `agent`+`prompt` is the legacy inline option.
pub fn is_approval_gate(step: &Step) -> bool {
    step.approval.is_some()
        && step.agent.is_none()
        && step.prompt.is_none()
        && step.for_each.is_none()
        && step.parallel.is_none()
        && step.panel.is_none()
        && step.branch.is_none()
        && step.action.is_none()
}
```

(`step.action` lands in Task 2 — implement Tasks 1+2 together before compiling, or stub the field first.)

In `validate_step_shape`, add a new arm BEFORE the final linear `else` (after the `parallel` arm), plus new `WorkflowParseError` variants following the existing thiserror style:

```rust
    } else if is_approval_gate(step) {
        let ap = step.approval.as_ref().expect("gate has approval");
        if ap.on_timeout.is_some() && ap.timeout_seconds.is_none() {
            return Err(WorkflowParseError::GateOnTimeoutWithoutTimeout {
                step: step.id.clone(),
            });
        }
        for sub in &ap.on_reject {
            if sub.approval.is_some()
                || sub.for_each.is_some()
                || sub.parallel.is_some()
                || sub.panel.is_some()
                || sub.branch.is_some()
            {
                return Err(WorkflowParseError::GateOnRejectInvalidStep {
                    step: step.id.clone(),
                    sub: sub.id.clone(),
                });
            }
            validate_step_shape(sub)?; // linear (or Plan-2 action) rules apply
        }
    } else {
```

Note: a step with `approval:` AND `agent:` but no prompt falls to the linear arm and fails on missing `prompt` — that satisfies `gate_node_rejects_agent_mixing` without a special case.

Error variants (add to `WorkflowParseError`):

```rust
    #[error("step `{step}`: approval.on_timeout requires timeout_seconds")]
    GateOnTimeoutWithoutTimeout { step: String },
    #[error("step `{step}`: on_reject step `{sub}` must be a plain agent or action step (no nested gates, fan-out, panel, or branch)")]
    GateOnRejectInvalidStep { step: String, sub: String },
```

Also update `Workflow::parse`'s duplicate-id sweep (~line 893) to ALSO reject an `on_reject` sub-step id colliding with any top-level step id (they share the `step_results` namespace):

```rust
        for step in &wf.steps {
            if let Some(ap) = &step.approval {
                for sub in &ap.on_reject {
                    if !seen.insert(sub.id.clone()) {
                        return Err(WorkflowParseError::DuplicateStep(sub.id.clone()));
                    }
                }
            }
            // (existing per-step checks continue here)
        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-orchestrator --test workflow_parse 2>&1 | tail -3`
Expected: all pass (new + pre-existing).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/workflow.rs crates/rupu-orchestrator/tests/workflow_parse.rs
git commit -m "feat(orchestrator): approval gate node schema (auto_approve, on_timeout, notify, on_reject)"
```

---

### Task 2: Schema — `action:` step shape (parse-only) + explicit runtime stub error

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (Step struct ~line 719; `validate_step_shape`), `crates/rupu-orchestrator/src/runner.rs` (step loop, next to the branch block ~line 1123), `crates/rupu-orchestrator/src/runs.rs` (StepKind ~line 270)
- Test: `crates/rupu-orchestrator/tests/workflow_parse.rs`

**Interfaces:**
- Produces: `Step.action: Option<String>`, `Step.with: Option<serde_json::Value>`, `StepKind::Action`, `WorkflowParseError::ActionMutuallyExclusive`, `RunWorkflowError::ActionStepsNotYetSupported { step: String }`. Plan 2 replaces the runtime stub with real execution and adds catalog/name validation — parse accepts any non-empty tool name in Plan 1.

- [ ] **Step 1: Write the failing tests**

```rust
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
    let wf = rupu_orchestrator::workflow::Workflow::parse(yaml).unwrap();
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
    assert!(rupu_orchestrator::workflow::Workflow::parse(yaml).is_err());
}

#[test]
fn action_step_rejects_empty_name() {
    let yaml = r#"
name: bad
steps:
  - id: x
    action: ""
"#;
    assert!(rupu_orchestrator::workflow::Workflow::parse(yaml).is_err());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator --test workflow_parse action_step 2>&1 | tail -5`
Expected: FAIL — unknown field `action`.

- [ ] **Step 3: Implement**

`Step` gains (after `branch`):

```rust
    /// Connector action step (spec §3.2): the name of a tool from the
    /// MCP catalog (e.g. `scm.prs.create`). Mutually exclusive with
    /// every other shape. Parse-level in Plan 1; execution in Plan 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Parameters for `action:`; values are minijinja-rendered at
    /// execution time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<serde_json::Value>,
```

`validate_step_shape`: new arm between the gate arm and the linear else:

```rust
    } else if let Some(action) = &step.action {
        if step.agent.is_some()
            || step.prompt.is_some()
            || step.for_each.is_some()
            || step.parallel.is_some()
            || step.panel.is_some()
        {
            return Err(WorkflowParseError::ActionMutuallyExclusive {
                step: step.id.clone(),
            });
        }
        if action.trim().is_empty() {
            return Err(WorkflowParseError::ActionEmptyName {
                step: step.id.clone(),
            });
        }
    } else {
```

Also add `step.action.is_some()` to the mutual-exclusion checks inside the `branch`, `panel`, and `parallel` arms (each currently checks `agent/prompt/for_each/...`), and `step.action.is_none()` to the gate detector's conjunction (Task 1) and to the `host:` linear-only check (~line 1159).

Error variants:

```rust
    #[error("step `{step}`: `action:` is mutually exclusive with agent/prompt/for_each/parallel/panel")]
    ActionMutuallyExclusive { step: String },
    #[error("step `{step}`: `action:` must name a tool (e.g. scm.prs.create)")]
    ActionEmptyName { step: String },
```

`StepKind` in `runs.rs` gains `Action` (after `Branch`); `step_kind_for_run_record` (runner.rs ~1412) gains `else if step.action.is_some() { crate::runs::StepKind::Action }` before the final `else`.

Runner stub — in the step loop, immediately after the branch block (~line 1179), so an action step never reaches agent dispatch:

```rust
        // Action steps parse (Plan 1) but execute in Plan 2. Fail loudly
        // rather than silently no-op — a workflow that names one needs
        // the newer binary.
        if step.action.is_some() {
            return Err(RunWorkflowError::ActionStepsNotYetSupported {
                step: step.id.clone(),
            });
        }
```

`RunWorkflowError` variant (thiserror, in runner.rs's error enum):

```rust
    #[error("step `{step}`: `action:` steps are not supported by this rupu version yet (Plan 2)")]
    ActionStepsNotYetSupported { step: String },
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-orchestrator 2>&1 | grep -E "^test result" `
Expected: lib + workflow_parse all pass (modulo the 4 known-flaky linear_runner baseline failures).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/workflow.rs crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/src/runs.rs crates/rupu-orchestrator/tests/workflow_parse.rs
git commit -m "feat(orchestrator): action step shape parses; explicit runtime stub until Plan 2"
```

---

### Task 3: Runner — gate lifecycle (auto-approve, pause, approve-resume result synthesis)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (step loop: insert gate block BEFORE the legacy `step.approval` check ~line 1075; extend the gate-suppression path), `crates/rupu-orchestrator/src/workflow.rs` (`validate_template_refs` valid-fields list ~line 1164: add `"decision"`), `crates/rupu-orchestrator/src/runs.rs` (`StepKind::ApprovalGate` variant)
- Test: `crates/rupu-orchestrator/tests/gate_node.rs` (new)

**Interfaces:**
- Consumes: `is_approval_gate`, `TimeoutAction` (Task 1).
- Produces: gate `StepResult` with `kind: StepKind::ApprovalGate` and `output` = `{"decision":"approved"|"rejected","via":"human"|"auto"|"timeout","reason":<string|null>,"decided_at":<rfc3339>}` (serde_json-serialized). Tasks 4-5 and the Plan-3 DTO rely on this exact output shape. Test helpers: reuse `MockProvider` + `BypassDecider` from `rupu_agent::runner` and the existing harness in `tests/linear_runner.rs` / `tests/pause_resume_e2e.rs` (copy its store+factory setup; read those files first).

- [ ] **Step 1: Write failing runtime tests** (`tests/gate_node.rs`)

Model the harness on `tests/pause_resume_e2e.rs` (it builds a `RunStore` in a tempdir, a mock `StepFactory`, and drives `run_workflow` + approve + resume end-to-end). Cases:

```rust
// 1. auto_approve truthy: run completes without pausing; the gate's
//    StepResult has kind ApprovalGate, success=true, and output JSON
//    with decision=="approved" && via=="auto"; events.jsonl contains
//    step_started+step_completed for the gate and NO step_awaiting_approval.
// 2. auto_approve falsy: run parks AwaitingApproval keyed to the gate id;
//    events.jsonl ends with step_awaiting_approval for the gate.
// 3. approve + resume (ResumeState::from_approval with the gate id):
//    the resumed run synthesizes the gate StepResult (decision=="approved",
//    via=="human"), continues to the following step, and completes.
// 4. a workflow whose LAST step is a gate: approve-resume completes the
//    run with the gate result recorded (boundary case).
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator --test gate_node 2>&1 | tail -5`
Expected: FAIL (gate steps fall to linear validation → parse passes but runner treats gate as linear and panics/errors on absent prompt — the new block doesn't exist yet).

- [ ] **Step 3: Implement the gate block**

Add `ApprovalGate` to `StepKind` (runs.rs) and to `step_kind_for_run_record` (`if is_approval_gate(step) { StepKind::ApprovalGate }` as the FIRST check). In the runner step loop, insert after the branch block and before the legacy `step.approval` check:

```rust
        // ── Approval GATE NODE (spec §4.1) ─────────────────────────────
        if crate::workflow::is_approval_gate(step) {
            let ap = step.approval.as_ref().expect("gate has approval");
            let gate_suppressed = approved_step_id == Some(step.id.as_str());
            let prompt = match &ap.prompt {
                Some(t) => render_step_prompt(t, &ctx, render_mode(opts.strict_templates))
                    .map_err(|e| RunWorkflowError::Render { step: step.id.clone(), source: e })?,
                None => format!("Approve gate `{}` of workflow `{}`?", step.id, opts.workflow.name),
            };

            // Resolution helper: emit started+completed, record the result.
            let resolve = |via: &str, step_results: &mut Vec<StepResult>| { /* see below */ };

            if gate_suppressed {
                emit_gate_resolved(opts, run_id, step, "human", step_results);
                continue;
            }
            if let Some(expr) = &ap.auto_approve {
                let truthy = render_when_expression(expr, &ctx, render_mode(opts.strict_templates))
                    .map_err(|e| RunWorkflowError::Render { step: step.id.clone(), source: e })?;
                if truthy {
                    info!(step = %step.id, "gate auto-approved");
                    emit_gate_resolved(opts, run_id, step, "auto", step_results);
                    continue;
                }
            }
            info!(step = %step.id, "gate: pausing for approval");
            if let Some(sink) = opts.event_sink.as_ref() {
                sink.emit(run_id, &crate::executor::Event::StepAwaitingApproval {
                    run_id: run_id.to_string(),
                    step_id: step.id.clone(),
                    reason: prompt.clone(),
                });
            }
            return Ok(InnerOutcome::Paused {
                step_id: step.id.clone(),
                prompt,
                timeout_seconds: ap.timeout_seconds,
                reason: PauseReason::Approval,
                seed: Vec::new(),
                fanout_completed_units: std::collections::BTreeMap::new(),
            });
        }
```

With a free function (place near `step_kind_for_run_record`) — the real implementation of the resolve path used above (call it directly rather than via a closure; the closure sketch above is illustrative):

```rust
/// Record an approved gate node's result: step_started + step_completed
/// events, a StepResult whose output is the decision JSON (spec §3.1),
/// persisted like any other step.
fn emit_gate_resolved(
    opts: &OrchestratorRunOpts,
    run_id: &str,
    step: &Step,
    via: &str,
    step_results: &mut Vec<StepResult>,
) {
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(run_id, &crate::executor::Event::StepStarted {
            run_id: run_id.to_string(),
            step_id: step.id.clone(),
            kind: crate::runs::StepKind::ApprovalGate,
            agent: None,
            host: None,
        });
    }
    let output = serde_json::json!({
        "decision": "approved",
        "via": via,
        "reason": serde_json::Value::Null,
        "decided_at": chrono::Utc::now().to_rfc3339(),
    })
    .to_string();
    let result = StepResult {
        step_id: step.id.clone(),
        output,
        success: true,
        skipped: false,
        kind: crate::runs::StepKind::ApprovalGate,
        ..Default::default()
    };
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(run_id, &crate::executor::Event::StepCompleted {
            run_id: run_id.to_string(),
            step_id: step.id.clone(),
            success: true,
            duration_ms: 0,
            host: None,
        });
    }
    persist_step_result(opts, run_id, &result);
    step_results.push(result);
}
```

Template fields: in `validate_template_refs`'s valid step-output field list (`output, success, skipped, results, sub_results, findings, max_severity, iterations, resolved`), add `decision`. Downstream templates reference the JSON via `{{ steps.merge_gate.output }}` (string) — `decision` as a first-class field is bound when building context: in `base_context_for_step`, for `StepKind::ApprovalGate` results, parse `output` as JSON and bind `decision` from it (follow how `findings`/`max_severity` are bound for panel steps — read that code in `templates.rs` and mirror it).

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-orchestrator --test gate_node 2>&1 | tail -3`
Expected: all 4 pass. Then `cargo test -p rupu-orchestrator --lib` — all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/src/runs.rs crates/rupu-orchestrator/src/workflow.rs crates/rupu-orchestrator/tests/gate_node.rs
git commit -m "feat(orchestrator): approval gate node runtime — auto-approve, pause, approve-resume"
```

---

### Task 4: Reject with inline cleanup chain

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`ResumeState` ~line 466: add `from_rejection`; step loop: rejection-cleanup mode), `crates/rupu-orchestrator/src/runs.rs` (RunStore::reject: gate-aware), `crates/rupu-cli/src/cmd/workflow.rs` (reject subcommand: run cleanup)
- Test: `crates/rupu-orchestrator/tests/gate_node.rs` (extend)

**Interfaces:**
- Consumes: gate block (Task 3), `RunStore::reject` (runs.rs ~line 1018 — appends the terminal event since PR #501).
- Produces: `ResumeState::from_rejection(run_id, prior_step_results, rejected_step_id: String, reason: String) -> Self` + `PauseReason` untouched; new `pub async fn run_reject_cleanup(opts: OrchestratorRunOpts, rejected_step_id: &str, reason: &str) -> Result<(), RunWorkflowError>` in runner.rs. CLI calls it; Plan 4's cp-serve worker will call the same function for web rejects.

- [ ] **Step 1: Write failing tests** (extend `tests/gate_node.rs`)

```rust
// 5. reject with cleanup: park a gate (case 2 harness), call
//    store.reject(...), then run_reject_cleanup(...) with the workflow +
//    store wired. Assert: the on_reject step's StepResult is persisted
//    (its mock agent ran), the run record status is Rejected, its
//    error_message contains the reason, and the gate's own StepResult
//    output JSON has decision=="rejected" && via=="human".
// 6. cleanup step failure does not change the terminal outcome: make the
//    cleanup agent's mock fail; run still ends Rejected; the cleanup
//    StepResult records success=false.
// 7. gate with EMPTY on_reject: store.reject alone suffices (today's
//    behavior); run_reject_cleanup returns Ok without dispatching anything.
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator --test gate_node reject 2>&1 | tail -5`
Expected: FAIL — `run_reject_cleanup` not found.

- [ ] **Step 3: Implement**

Semantics (spec §4.1 step 6, ordered for crash-safety): `RunStore::reject` stays FIRST and terminal (status `Rejected` + terminal event — unchanged), THEN cleanup runs; a crash mid-cleanup leaves a correctly-Rejected run with partial cleanup logged.

`run_reject_cleanup` in runner.rs:

```rust
/// Execute a rejected gate's `on_reject` chain (spec §4.1). Called by
/// the rejecting process AFTER `RunStore::reject` finalized the run.
/// Failures inside the chain are logged per-step (`continue`), never
/// returned — the run is already terminal.
pub async fn run_reject_cleanup(
    opts: OrchestratorRunOpts,
    rejected_step_id: &str,
    reason: &str,
) -> Result<(), RunWorkflowError> {
    let Some(gate) = opts.workflow.steps.iter().find(|s| s.id == rejected_step_id) else {
        return Ok(()); // legacy inline approval or unknown id — nothing to run
    };
    if !crate::workflow::is_approval_gate(gate) {
        return Ok(());
    }
    let chain = gate.approval.as_ref().map(|a| a.on_reject.clone()).unwrap_or_default();
    // Body, in order (each numbered item maps to existing code to mirror):
    // 1. Load prior results: read `<run_dir>/step_results.jsonl` into
    //    Vec<StepResult> — same loader the CLI approve path uses (search
    //    `from_approval` in crates/rupu-cli/src/cmd/workflow.rs and reuse
    //    the orchestrator-side helper it calls).
    // 2. Push the gate's own rejected result via a `via`/`decision`
    //    variant of Task 3's emit_gate_resolved — generalize that fn to
    //    `emit_gate_result(opts, run_id, step, decision: &str, via: &str,
    //    reason: Option<&str>, step_results: &mut Vec<StepResult>)` and
    //    have Task 3's call sites pass ("approved", via, None).
    // 3. For each step in `chain`: build ctx with base_context_for_step
    //    (same call as the main loop, ~runner.rs:1024), render the prompt
    //    with render_step_prompt, dispatch through opts.step_factory
    //    exactly as run_workflow's linear arm does (StepStarted event →
    //    factory build_opts_for_step → agent run → StepCompleted/StepFailed
    //    event → persist_step_result → push). On error: log with
    //    tracing::warn!, record success=false, CONTINUE the chain.
    // 4. Return Ok(()) — cleanup never changes the terminal status.
    //
    // If extracting run_workflow's linear dispatch into a shared helper
    // stays under ~50 lines of diff, extract (`dispatch_linear_step`);
    // otherwise mirror the code inline here and leave a comment pointing
    // at the main-loop original.
    Ok(())
}
```

Implementation guidance (the engineer must read `run_workflow`'s linear dispatch before writing this): prior step results load exactly the way the approve-resume path loads them (see the CLI approve path in `crates/rupu-cli/src/cmd/workflow.rs` — search `from_approval` — it reads `step_results.jsonl` back into `Vec<StepResult>`). The gate's rejected `StepResult` uses the same JSON shape as Task 3 with `"decision":"rejected","via":"human","reason":<reason>`.

CLI: in the `reject` subcommand handler (search `fn` containing `RunStore::reject` in `crates/rupu-cli/src/cmd/workflow.rs`), after a successful reject, load the run's persisted workflow YAML (the run dir stores it — same load the approve path uses), build `OrchestratorRunOpts` the same way approve-resume does, and call `run_reject_cleanup(opts, &step_id, &reason).await`, printing a `cleanup: <n> step(s) executed` line. The `ApprovalDecision::Rejected { step_id, .. }` return value carries the step id.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-orchestrator --test gate_node 2>&1 | tail -3` — all 7 pass.
Run: `cargo test -p rupu-cli 2>&1 | grep "test result"` — no regressions (compare against the known-red baseline under the local toolchain; see Global Constraints).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/src/runs.rs crates/rupu-orchestrator/tests/gate_node.rs crates/rupu-cli/src/cmd/workflow.rs
git commit -m "feat(orchestrator): gate reject runs the on_reject cleanup chain"
```

---

### Task 5: Timeout routing (`on_timeout`) in the lazy expiry path

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs` (`expire_if_overdue` ~line 884)
- Test: `crates/rupu-orchestrator/src/runs.rs` (in-file `mod tests`, next to the existing `expire_if_overdue_appends_terminal_event` test)

**Interfaces:**
- Consumes: `TimeoutAction` (Task 1). `expire_if_overdue` currently flips to `Failed` unconditionally.
- Produces: `expire_if_overdue(record, now, on_timeout: Option<TimeoutAction>) -> Result<Option<TimeoutAction>, RunStoreError>` — returns `Some(action)` when expiry fired, telling the CALLER what to do next (`Approve` ⇒ caller resumes like an approval; `Reject` ⇒ caller invokes reject + cleanup; `Fail` ⇒ handled fully inside, as today). All existing callers (approve/reject/runs paths — `grep -rn expire_if_overdue crates/`) pass the gate's `on_timeout` when the awaiting step is a gate node, else `None` (= `Fail`). Plan 4's cp-serve sweep consumes the same return contract.

- [ ] **Step 1: Write failing tests** (same in-file style as `expire_if_overdue_appends_terminal_event`)

```rust
// on_timeout=fail (or None): status flips Failed + RunFailed event (today).
// on_timeout=approve: record is NOT failed — status stays AwaitingApproval,
//   fn returns Some(Approve); the record's error_message untouched.
// on_timeout=reject: status flips Rejected + terminal event appended,
//   error_message contains "approval expired", fn returns Some(Reject)
//   (caller then runs cleanup).
```

- [ ] **Step 2: Run to verify failure** — signature change won't compile: `cargo test -p rupu-orchestrator --lib expire 2>&1 | tail -5` → compile error (callers), then fix callers, then behavioral FAILs.

- [ ] **Step 3: Implement** — thread `on_timeout` through; `Fail` arm = existing body verbatim; `Reject` arm mirrors `reject`'s field mutations (status `Rejected`, finished_at, error_message "approval expired: ...", terminal event `RunCompleted{Rejected}`); `Approve` arm mutates nothing and returns `Some(Approve)`. Callers resolve the awaiting step's gate `on_timeout` by parsing the run's stored workflow YAML (same loader as Task 4); on `Some(Approve)` the CLI paths print that the gate auto-approved on timeout and proceed exactly like an operator approve; the plain `runs`-listing path treats `Some(Approve)` as "resume now via the normal approve flow".

- [ ] **Step 4: Run** `cargo test -p rupu-orchestrator --lib 2>&1 | grep "test result"` — all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/runs.rs crates/rupu-cli/src/cmd/workflow.rs
git commit -m "feat(orchestrator): gate on_timeout routing (approve|reject|fail) in lazy expiry"
```

---

### Task 6: Sample workflow + full verification

**Files:**
- Create: `.rupu/workflows/gate-demo.yaml` (dogfood sample exercising gate + auto_approve + on_reject; NO action steps yet — they'd error at runtime until Plan 2)
- Modify: `CLAUDE.md` — one line under the rupu-orchestrator crate bullet noting the two new shapes and this plan file.

- [ ] **Step 1: Write the sample** (agent-only cleanup so it runs today)

```yaml
name: gate-demo
description: "Demo: approval gate node with auto-approve and reject cleanup."
steps:
  - id: assess
    agent: code-reviewer
    prompt: "Assess the working tree; reply exactly 'clean' if nothing is risky."
  - id: ship_gate
    approval:
      prompt: "Assessment: {{ steps.assess.output }} — approve to continue."
      auto_approve: "{{ steps.assess.output == 'clean' }}"
      timeout_seconds: 86400
      on_timeout: reject
      on_reject:
        - id: note_rejection
          agent: code-reviewer
          prompt: "Summarize why run {{ steps.assess.output }} was rejected, one line."
  - id: proceed
    agent: code-reviewer
    prompt: "Gate decision was {{ steps.ship_gate.decision }}. Say 'shipped'."
```

- [ ] **Step 2: Verify end-to-end by hand**

Run: `cargo run -p rupu-cli -- workflow validate .rupu/workflows/gate-demo.yaml` (or the repo's equivalent lint subcommand — check `rupu workflow --help`). Expected: parses clean.

- [ ] **Step 3: Full suite + clippy**

Run: `cargo test -p rupu-orchestrator 2>&1 | grep "test result"` and `cargo clippy -p rupu-orchestrator -p rupu-cli 2>&1 | grep -c "^error" ` → expect `0`. (linear_runner baseline flakes excepted — compare to a clean checkout.)

- [ ] **Step 4: Commit + PR**

```bash
git add .rupu/workflows/gate-demo.yaml CLAUDE.md
git commit -m "feat(orchestrator): gate-demo sample workflow + docs note"
# branch: feat/gate-nodes-plan-1 → push → gh pr create --draft (PR ① of the arc)
```

---

## Deferred to later plans (do NOT implement here)

- **Plan 2:** action-step execution through the in-process MCP layer, catalog/name/schema parse validation (needs `rupu-orchestrator → rupu-mcp` dep — verified acyclic), readonly-mode Write refusal, audit unification.
- **Plan 3:** renderers (StepNodeDto kinds `approval`/`action`, GateNode/ActionNode, app-canvas emitters, editor palette groups + generated connector forms, legacy inline-approval synthesized gate + extraction).
- **Plan 4:** `notify:` execution on entering awaiting; cp-serve gate sweep consuming Task 5's `expire_if_overdue` contract; web-reject cleanup via the resume worker.
