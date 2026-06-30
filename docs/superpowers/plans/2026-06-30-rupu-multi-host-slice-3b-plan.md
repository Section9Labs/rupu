# Multi-host Slice 3b — Per-Step Host Placement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a linear workflow step declare `host: <id>` so the coordinator runs that whole step's agent on a named fleet host and feeds its output into downstream steps — reusing the Slice 3a `UnitDispatcher` machinery verbatim.

**Architecture:** A new `host: Option<String>` field on `Step` marks a placement. `run_linear_step`, when `step.host` is `Some`, dispatches the step as a single unit (`index: 0`) through the existing `UnitDispatcher` port instead of the local `dispatch_one` path, and builds its `StepResult` from the returned `UnitOutcome`. Step events gain `host` attribution. The CLI coordinator's `build_dispatcher_if_needed` trigger widens to fire for `host:` steps too. No new ports, no reassignment, no transport changes.

**Tech Stack:** Rust 2021 (MSRV 1.88), tokio, serde/serde_yaml, thiserror (libs) / anyhow (CLI), async-trait, tracing.

## Global Constraints

- Backward compatible: a step with **no** `host:` runs byte-for-byte as today (the local inline path is untouched).
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code = "forbid"`.
- Libraries use `thiserror`; the CLI binary uses `anyhow`.
- Workspace dependencies only — never pin a version in a crate `Cargo.toml`. No new dependency is needed for this slice.
- Hexagonal: the orchestrator knows only the `UnitDispatcher` trait — never `rupu-cp` or any transport.
- Per-file `rustfmt` only (`rustfmt <path>`); `main` is fmt-dirty under the pinned toolchain — never run a package-wide `cargo fmt`.
- Reuse the 3a `UnitDispatcher` / `FleetUnitDispatcher` / `get_run`-envelope path (`run.final_output`) verbatim — no parallel seam.
- Self-contained-step guardrail: a placed step must be computable from its rendered prompt + prior-step **string** context, not the shared file workspace. Repo-file steps wait for Slice 3c. Document this; do not enforce it here.
- Clippy note: the worktree's toolchain flags a pre-existing `rupu-config` `is_none_or` lint unrelated to this work — run orchestrator/CLI clippy with `--no-deps` to scope to the changed crates.
- Scope is **linear** steps only. `parallel` / `panel` / `for_each` placement is out of scope (3a already owns `for_each` via `distribute:`).

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `crates/rupu-orchestrator/src/workflow.rs` | `host` field on `Step` + validation (linear-only, non-empty, not with `distribute`) | 1 |
| `crates/rupu-orchestrator/src/executor/event.rs` | `host` on `StepStarted` / `StepCompleted` + serde round-trip test | 2 |
| `crates/rupu-orchestrator/src/executor/in_memory_sink.rs`, `jsonl_sink.rs`, `sink.rs` | update test constructors to set `host` | 2 |
| `crates/rupu-cp/tests/sse.rs`, `crates/rupu-cp/tests/bucket_e2e.rs` | update test constructors | 2 |
| `crates/rupu-app/tests/run_model.rs` | update test constructors | 2 |
| `crates/rupu-orchestrator/src/runner.rs` | route placed linear step through `UnitDispatcher`; thread `host` into step-event emits | 3 |
| `crates/rupu-cli/src/fleet_unit_dispatcher.rs` | widen `build_dispatcher_if_needed` trigger to `host:` steps | 3 |
| `crates/rupu-orchestrator/tests/placed_step_e2e.rs` (new) | end-to-end: 2-step workflow, step 2 placed, output flows; no-host control | 4 |

---

## Task 1: `host` field on `Step` + validation

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (`Step` struct ~line 533; `WorkflowParseError` enum ~line 116; `validate_step_shape` ~line 818; tests `mod distribute_tests` ~line 1200)
- Test: same file (unit tests)

**Interfaces:**
- Consumes: nothing new.
- Produces: `Step.host: Option<String>` (public field). Two new `WorkflowParseError` variants: `HostOnNonLinearStep { step: String }` and `HostEmpty { step: String }`.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod distribute_tests` block (or a new `mod host_tests`) at the bottom of `crates/rupu-orchestrator/src/workflow.rs`:

```rust
#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn host_parses_on_linear_step() {
        let yaml = r#"
name: placed
steps:
  - id: build
    agent: builder
    prompt: "do it"
    host: worker-1
"#;
        let wf = Workflow::parse(yaml).expect("valid");
        assert_eq!(wf.steps[0].host.as_deref(), Some("worker-1"));
    }

    #[test]
    fn host_absent_is_none() {
        let yaml = r#"
name: local
steps:
  - id: build
    agent: builder
    prompt: "do it"
"#;
        let wf = Workflow::parse(yaml).expect("valid");
        assert_eq!(wf.steps[0].host, None);
    }

    #[test]
    fn host_round_trips_skipping_none() {
        let yaml = r#"
name: local
steps:
  - id: build
    agent: builder
    prompt: "do it"
"#;
        let wf = Workflow::parse(yaml).expect("valid");
        let out = serde_yaml::to_string(&wf).expect("serialize");
        assert!(!out.contains("host"), "None host must be skipped: {out}");
    }

    #[test]
    fn host_rejected_on_for_each() {
        let yaml = r#"
name: bad
steps:
  - id: fan
    for_each: "a\nb"
    agent: a
    prompt: "p"
    host: worker-1
"#;
        let err = Workflow::parse(yaml).expect_err("for_each + host invalid");
        assert!(matches!(err, WorkflowParseError::HostOnNonLinearStep { .. }));
    }

    #[test]
    fn host_rejected_on_parallel() {
        let yaml = r#"
name: bad
steps:
  - id: par
    host: worker-1
    parallel:
      - id: s1
        agent: a
        prompt: p
"#;
        let err = Workflow::parse(yaml).expect_err("parallel + host invalid");
        assert!(matches!(err, WorkflowParseError::HostOnNonLinearStep { .. }));
    }

    #[test]
    fn host_rejected_on_panel() {
        let yaml = r#"
name: bad
steps:
  - id: pan
    host: worker-1
    panel:
      panelists: [reviewer]
      subject: "{{ inputs.x }}"
"#;
        let err = Workflow::parse(yaml).expect_err("panel + host invalid");
        assert!(matches!(err, WorkflowParseError::HostOnNonLinearStep { .. }));
    }

    #[test]
    fn empty_host_rejected() {
        let yaml = r#"
name: bad
steps:
  - id: build
    agent: builder
    prompt: "do it"
    host: ""
"#;
        let err = Workflow::parse(yaml).expect_err("empty host invalid");
        assert!(matches!(err, WorkflowParseError::HostEmpty { .. }));
    }
}
```

> Note: if `Workflow::parse` is not the exact constructor name, use the same parse entry point the existing `distribute_tests` use (check `mod distribute_tests` ~line 1200 for the call — it parses a YAML string to a `Workflow` and returns `Result<_, WorkflowParseError>`). Match it verbatim.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-orchestrator host_tests`
Expected: FAIL — `no field 'host' on type '&Step'` / `no variant ... HostOnNonLinearStep`.

- [ ] **Step 3: Add the `host` field to `Step`**

In `crates/rupu-orchestrator/src/workflow.rs`, inside `pub struct Step` (after the `distribute` field ~line 611), add:

```rust
    /// Optional fleet host placement for a **linear** step. When present,
    /// the whole step's agent runs on the named host (via the
    /// `UnitDispatcher` port) instead of locally, and its output feeds
    /// downstream steps exactly as a local step would. Valid only on a
    /// linear step (`agent` + `prompt`; not `for_each`/`parallel`/`panel`)
    /// and not together with `distribute:` (which is `for_each`-only).
    /// Absent ⇒ runs locally (backward compatible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
```

- [ ] **Step 4: Add the error variants**

In the `WorkflowParseError` enum, after the `DistributeEmptyHosts` variant (~line 118), add:

```rust
    #[error("step `{step}`: `host:` is only valid on a linear step (agent + prompt), not on `for_each:`/`parallel:`/`panel:`")]
    HostOnNonLinearStep { step: String },
    #[error("step `{step}`: `host:` must not be empty")]
    HostEmpty { step: String },
```

- [ ] **Step 5: Add validation**

In `validate_step_shape` (~line 818), after the `distribute` validation block (the one ending ~line 894, just before `Ok(())`), add:

```rust
    // Validate host placement: only valid on a linear step (not panel /
    // parallel / for_each), non-empty, and never alongside `distribute:`
    // (which is for_each-only — structurally exclusive with a linear host
    // step, but assert it for a clear message).
    if let Some(host) = &step.host {
        let is_linear = step.panel.is_none()
            && step.parallel.is_none()
            && step.for_each.is_none();
        if !is_linear || step.distribute.is_some() {
            return Err(WorkflowParseError::HostOnNonLinearStep {
                step: step.id.clone(),
            });
        }
        if host.trim().is_empty() {
            return Err(WorkflowParseError::HostEmpty {
                step: step.id.clone(),
            });
        }
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator host_tests`
Expected: PASS (7 tests).

- [ ] **Step 7: Format, lint, and run the full crate suite**

Run:
```bash
rustfmt crates/rupu-orchestrator/src/workflow.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
cargo test -p rupu-orchestrator
```
Expected: clippy clean for `rupu-orchestrator`; all existing tests still green (the new field is additive with serde defaults).

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-orchestrator/src/workflow.rs
git commit -m "feat(multi-host): add host placement field to linear Step (3b T1)"
```

---

## Task 2: `host` attribution on step events

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/event.rs` (`StepStarted` ~line 22, `StepCompleted` ~line 38; test ~line 164)
- Modify (test constructors): `crates/rupu-orchestrator/src/executor/in_memory_sink.rs` (~lines 48, 66), `crates/rupu-orchestrator/src/executor/jsonl_sink.rs` (~lines 81, 90, 113), `crates/rupu-orchestrator/src/executor/sink.rs` (~line 63)
- Modify (test constructors): `crates/rupu-cp/tests/sse.rs` (~line 62), `crates/rupu-cp/tests/bucket_e2e.rs` (~line 296), `crates/rupu-app/tests/run_model.rs` (~lines 26, 39, 56, 62)
- Test: `crates/rupu-orchestrator/src/executor/event.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Event::StepStarted` and `Event::StepCompleted` each gain a field `host: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`). Pattern-match sites that already use `..` are unaffected; **struct-literal construction sites must set `host`**.

> Why every construction site changes: a struct-literal enum variant requires all fields. `serde(default)` only affects deserialization, not Rust construction. The grep list above is the complete set of literal construction sites; destructure sites (`run_model.rs:41/60`, `live_run.rs:245/270`, `jsonl_sink.rs:124`) use `..` and need no change.

- [ ] **Step 1: Write the failing test**

In `crates/rupu-orchestrator/src/executor/event.rs`, in the `#[cfg(test)] mod` that holds the existing `StepCompleted` test (~line 164), add:

```rust
    #[test]
    fn step_started_host_round_trips() {
        let ev = Event::StepStarted {
            run_id: "r1".into(),
            step_id: "build".into(),
            kind: StepKind::Linear,
            agent: Some("builder".into()),
            host: Some("worker-1".into()),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"host\":\"worker-1\""), "json: {json}");
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            back,
            Event::StepStarted { host: Some(ref h), .. } if h == "worker-1"
        ));
    }

    #[test]
    fn step_completed_host_defaults_to_none_when_absent() {
        // Older event logs without `host` must still deserialize.
        let json = r#"{"type":"step_completed","run_id":"r1","step_id":"build","success":true,"duration_ms":5}"#;
        let back: Event = serde_json::from_str(json).expect("deserialize legacy");
        assert!(matches!(back, Event::StepCompleted { host: None, .. }));
    }
```

> Verify the variant tag format: check the existing `StepCompleted` test (~line 164) and the enum's `#[serde(...)]` attributes for the exact `type` tag string (e.g. `"step_completed"` vs `"StepCompleted"`) and adjust the legacy-JSON literal to match. Use `StepKind::Linear` only if that's the real variant name — confirm against the `StepKind` enum near the top of the file.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-orchestrator -- step_started_host step_completed_host`
Expected: FAIL — `missing field 'host'` in the `StepStarted` literal / variant has no field `host`.

- [ ] **Step 3: Add the field to both variants**

In `crates/rupu-orchestrator/src/executor/event.rs`:

`StepStarted` (~line 22) — add after `agent`:
```rust
    StepStarted {
        run_id: String,
        step_id: String,
        kind: StepKind,
        agent: Option<String>,
        /// Host that ran this step. `None` = local (same host as the
        /// orchestrator). `Some(name)` = a remote fleet host (multi-host
        /// `host:` placement). Absent in older event logs; serde default
        /// restores `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
```

`StepCompleted` (~line 38) — add after `duration_ms`:
```rust
    StepCompleted {
        run_id: String,
        step_id: String,
        success: bool,
        duration_ms: u64,
        /// Host that ran this step. `None` = local. `Some(name)` = remote.
        /// Absent in older event logs; serde default restores `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
```

- [ ] **Step 4: Update every struct-literal construction site to set `host: None`**

These are all test/sink constructors (the runner's two production sites are handled in Task 3). Add `host: None,` to each literal:

- `crates/rupu-orchestrator/src/executor/event.rs` ~line 164 (the existing `StepCompleted` test) — add `host: None,`
- `crates/rupu-orchestrator/src/executor/in_memory_sink.rs` ~lines 48, 66 (`StepStarted`) — add `host: None,`
- `crates/rupu-orchestrator/src/executor/jsonl_sink.rs` ~lines 81 (`StepStarted`), 90 (`StepCompleted`), 113 (`StepStarted`) — add `host: None,`
- `crates/rupu-orchestrator/src/executor/sink.rs` ~line 63 (`StepStarted`) — add `host: None,`
- `crates/rupu-cp/tests/sse.rs` ~line 62 (`StepStarted`) — add `host: None,`
- `crates/rupu-cp/tests/bucket_e2e.rs` ~line 296 (`StepStarted`) — add `host: None,`
- `crates/rupu-app/tests/run_model.rs` ~lines 26, 39, 56 (`StepStarted`), 62 (`StepCompleted`) — add `host: None,`

For each, the edit is to insert the field inside the existing literal, e.g.:
```rust
Event::StepStarted {
    run_id: "r1".into(),
    step_id: "alpha".into(),
    kind: /* existing */,
    agent: /* existing */,
    host: None,
}
```

- [ ] **Step 5: Run the new tests and full suites for the touched crates**

Run:
```bash
cargo test -p rupu-orchestrator -- step_started_host step_completed_host
cargo test -p rupu-orchestrator
cargo test -p rupu-cp --test sse --test bucket_e2e
cargo test -p rupu-app --test run_model
```
Expected: the two new tests PASS; all touched suites compile and pass. (If `rupu-app` won't build in this environment due to GPUI/toolchain, note it in the report — the `run_model` test is the relevant target; a compile error there from a missing `host: None` is in scope, a GPUI link failure is not.)

- [ ] **Step 6: Format and lint**

Run:
```bash
rustfmt crates/rupu-orchestrator/src/executor/event.rs crates/rupu-orchestrator/src/executor/in_memory_sink.rs crates/rupu-orchestrator/src/executor/jsonl_sink.rs crates/rupu-orchestrator/src/executor/sink.rs crates/rupu-cp/tests/sse.rs crates/rupu-cp/tests/bucket_e2e.rs crates/rupu-app/tests/run_model.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
```
Expected: clippy clean for `rupu-orchestrator`.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-orchestrator/src/executor crates/rupu-cp/tests crates/rupu-app/tests
git commit -m "feat(multi-host): host attribution on StepStarted/StepCompleted (3b T2)"
```

---

## Task 3: Route a placed linear step through the `UnitDispatcher` (CORE)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` — `run_linear_step` (~line 949); step-event emit sites in the step loop (`StepStarted` ~line 751, `StepCompleted` ~line 778); add two helpers; tests `mod` at the bottom (the `FakeUnitDispatcher` harness is at ~line 2531)
- Modify: `crates/rupu-cli/src/fleet_unit_dispatcher.rs` — `build_dispatcher_if_needed` trigger (~line 159)
- Test: `crates/rupu-orchestrator/src/runner.rs` (unit tests reusing `FakeUnitDispatcher`)

**Interfaces:**
- Consumes: `Step.host` (Task 1); `Event::StepStarted/StepCompleted.host` (Task 2); the existing `UnitDispatcher` trait, `UnitDispatch { step_id, agent, rendered_prompt, index, run_id }`, `UnitOutcome { output, success, error }` (runner.rs ~lines 43–57); `OrchestratorRunOpts.unit_dispatcher: Option<Arc<dyn UnitDispatcher>>` (~line 205); `RunError::Provider`; `make_opts(...)` test helper (~line 2578); `FakeUnitDispatcher` (~line 2531).
- Produces: `run_linear_step` routes `step.host == Some` through `dispatch_unit` (index 0); new private helpers `dispatch_placed_step` and `placed_failure`. `build_dispatcher_if_needed` returns `Some` for a workflow with any `host:` step.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod` at the bottom of `crates/rupu-orchestrator/src/runner.rs` (the one that defines `FakeUnitDispatcher`, `make_opts`, and `WF_DISTRIBUTED` ~line 2603). These reuse `FakeUnitDispatcher` (which returns `output: "out-{index}-on-{host}"`, `success: true`, and records `(index, host)` calls; `with_failing_host` makes the first dispatch to a host `Err`).

```rust
    const WF_PLACED: &str = r#"
name: placed-test
steps:
  - id: build
    agent: builder
    prompt: "build {{ inputs.what }}"
    host: worker-1
"#;

    const WF_PLACED_TWO_STEP: &str = r#"
name: placed-chain
steps:
  - id: build
    agent: builder
    prompt: "build it"
    host: worker-1
  - id: report
    agent: reporter
    prompt: "summarize {{ steps.build.output }}"
    host: worker-2
"#;

    #[tokio::test]
    async fn placed_linear_step_dispatched_through_port() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(WF_PLACED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher.clone());
        opts.inputs.insert("what".into(), "rupu".into());

        let result = run_workflow(opts).await.expect("run ok");

        // The dispatcher saw exactly one unit at index 0 on worker-1.
        let calls = dispatcher.calls.lock().unwrap().clone();
        assert_eq!(calls, vec![(0, "worker-1".to_string())]);

        // The UnitOutcome.output became the step output.
        let sr = &result.step_results[0];
        assert_eq!(sr.step_id, "build");
        assert!(sr.success);
        assert_eq!(sr.output, "out-0-on-worker-1");
    }

    #[tokio::test]
    async fn placed_step_output_feeds_downstream() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::new());
        let wf = Workflow::parse(WF_PLACED_TWO_STEP).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), dispatcher.clone());

        let result = run_workflow(opts).await.expect("run ok");

        // Step 2 ran on worker-2, and its rendered prompt embedded step 1's output.
        let calls = dispatcher.calls.lock().unwrap().clone();
        assert_eq!(calls, vec![(0, "worker-1".to_string()), (0, "worker-2".to_string())]);
        assert_eq!(result.step_results.len(), 2);
        assert_eq!(result.step_results[1].rendered_prompt, "summarize out-0-on-worker-1");
    }

    #[tokio::test]
    async fn placed_step_remote_err_aborts_without_continue_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::with_failing_host("worker-1"));
        let wf = Workflow::parse(WF_PLACED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        opts.inputs.insert("what".into(), "rupu".into());

        let err = run_workflow(opts).await.expect_err("must abort");
        assert!(matches!(err, RunWorkflowError::Agent { ref step, .. } if step == "build"));
    }

    #[tokio::test]
    async fn placed_step_remote_err_tolerated_with_continue_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(FakeUnitDispatcher::with_failing_host("worker-1"));
        let yaml = r#"
name: placed-tolerant
steps:
  - id: build
    agent: builder
    prompt: "build it"
    host: worker-1
    continue_on_error: true
"#;
        let wf = Workflow::parse(yaml).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);

        let result = run_workflow(opts).await.expect("tolerated");
        assert!(!result.step_results[0].success);
    }

    #[tokio::test]
    async fn placed_step_failed_outcome_aborts() {
        // Agent ran but reported success=false → still aborts under
        // continue_on_error:false (symmetric with the fan-out path).
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(AlwaysFailedOutcomeDispatcher);
        let wf = Workflow::parse(WF_PLACED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        opts.inputs.insert("what".into(), "rupu".into());

        let err = run_workflow(opts).await.expect_err("must abort on success=false");
        assert!(matches!(err, RunWorkflowError::Agent { ref step, .. } if step == "build"));
    }

    #[tokio::test]
    async fn placed_step_without_dispatcher_errors_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let wf = Workflow::parse(WF_PLACED).unwrap();
        // make_opts requires a dispatcher; build opts with None directly.
        let mut opts = make_opts(
            wf,
            dir.path().to_path_buf(),
            Arc::new(FakeUnitDispatcher::new()),
        );
        opts.unit_dispatcher = None;
        opts.inputs.insert("what".into(), "rupu".into());

        let err = run_workflow(opts).await.expect_err("must error without fleet");
        let msg = err.to_string();
        assert!(msg.contains("fleet"), "expected clear fleet-access error, got: {msg}");
    }
```

The `AlwaysFailedOutcomeDispatcher` referenced above already exists at ~line 2755 (it returns `Ok(UnitOutcome { success: false, .. })`). Confirm its definition; if its field shape differs, mirror it. If `run_workflow` is not the exact entry name used by the sibling fan-out tests, use whatever those tests call (check the existing distributed fan-out tests in the same `mod` — they build `make_opts` and invoke the same top-level runner). Match it verbatim.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-orchestrator -- placed_`
Expected: FAIL — placed steps currently take the local `dispatch_one` path (which uses `PanicFactory` in `make_opts`), so they panic / never call the dispatcher. The assertions on `calls` / abort behavior fail.

- [ ] **Step 3: Add the placement helpers**

In `crates/rupu-orchestrator/src/runner.rs`, immediately **before** `async fn run_linear_step` (~line 949), add:

```rust
/// Run a host-placed linear step as a single remote unit through the
/// [`UnitDispatcher`] port (index 0). Mirrors the fan-out remote path:
/// `Ok(success:true)` → that output; `Ok(success:false)` or `Err` → a
/// failed step honoring `continue_on_error`. There is **no reassignment**
/// — a single named host has no alternate. Absence of a dispatcher is a
/// hard configuration error (coordinator without fleet access), surfaced
/// clearly with no silent local fallback.
async fn dispatch_placed_step(
    host: &str,
    step: &Step,
    agent_name: &str,
    rendered: &str,
    run_id: &str,
    opts: &OrchestratorRunOpts,
    continue_on_error: bool,
) -> Result<(String, bool), RunWorkflowError> {
    let Some(dispatcher) = opts.unit_dispatcher.as_ref() else {
        let source =
            RunError::Provider("host placement requires fleet access — run via the CP".into());
        let output = source.to_string();
        return placed_failure(step, host, output, source, continue_on_error);
    };
    let unit = UnitDispatch {
        step_id: step.id.clone(),
        agent: agent_name.to_string(),
        rendered_prompt: rendered.to_string(),
        index: 0,
        run_id: run_id.to_string(),
    };
    match dispatcher.dispatch_unit(unit, host).await {
        Ok(outcome) if outcome.success => Ok((outcome.output, true)),
        Ok(outcome) => {
            // Agent ran but reported failure: preserve its output, but
            // synthesize a raw error so the abort below fires — symmetric
            // with the fan-out remote path.
            let source = RunError::Provider(
                outcome
                    .error
                    .clone()
                    .unwrap_or_else(|| "remote step failed".into()),
            );
            placed_failure(step, host, outcome.output, source, continue_on_error)
        }
        Err(source) => {
            let output = source.to_string();
            placed_failure(step, host, output, source, continue_on_error)
        }
    }
}

/// Apply `continue_on_error` to a failed placement: tolerate (record a
/// failed `(output, false)`) or abort with the same `RunWorkflowError::Agent`
/// a local step failure produces.
fn placed_failure(
    step: &Step,
    host: &str,
    output: String,
    source: RunError,
    continue_on_error: bool,
) -> Result<(String, bool), RunWorkflowError> {
    if continue_on_error {
        warn!(
            step = %step.id,
            host = %host,
            error = %source,
            "placed step failed but continue_on_error is set; proceeding"
        );
        Ok((output, false))
    } else {
        Err(RunWorkflowError::Agent {
            step: step.id.clone(),
            source,
        })
    }
}
```

- [ ] **Step 4: Branch `run_linear_step` on `step.host`**

In `run_linear_step` (~line 949), the current body (after `rendered`, `run_id`, `transcript_path`, `persist_active_step`) builds `on_tool_call`, calls `dispatch_one`, computes `success`, then `read_final_assistant_text`. Replace the block **from** the `let on_tool_call ...` binding (~line 975) **through** the `let output = read_final_assistant_text(...)` line (~line 1024) with a `match` on the placement — keeping the local arm byte-for-byte:

```rust
    let (output, success) = match step.host.as_deref() {
        Some(host) => {
            dispatch_placed_step(
                host,
                step,
                agent_name,
                &rendered,
                &run_id,
                opts,
                continue_on_error,
            )
            .await?
        }
        None => {
            // --- Existing local (inline) path — UNCHANGED ---
            let on_tool_call: Option<rupu_agent::OnToolCallCallback> =
                opts.event_sink.as_ref().map(|sink| {
                    let sink = sink.clone();
                    let wf_run_id = workflow_run_id.to_string();
                    let step_id = step.id.clone();
                    std::sync::Arc::new(move |_caller_step_id: &str, tool_name: &str| {
                        sink.emit(
                            &wf_run_id,
                            &crate::executor::Event::StepWorking {
                                run_id: wf_run_id.clone(),
                                step_id: step_id.clone(),
                                note: Some(tool_name.to_string()),
                            },
                        );
                    }) as rupu_agent::OnToolCallCallback
                });

            let outcome = dispatch_one(
                &opts.factory,
                &step.id,
                agent_name,
                rendered.clone(),
                run_id.clone(),
                opts.workspace_id.clone(),
                opts.workspace_path.clone(),
                transcript_path.clone(),
                on_tool_call,
            )
            .await;

            let success = match outcome {
                Ok(_) => true,
                Err(source) => {
                    if continue_on_error {
                        warn!(
                            step = %step.id,
                            error = %source,
                            "step failed but continue_on_error is set; proceeding"
                        );
                        false
                    } else {
                        return Err(RunWorkflowError::Agent {
                            step: step.id.clone(),
                            source,
                        });
                    }
                }
            };

            let output =
                read_final_assistant_text(&transcript_path, success, &run_id, &step.id);
            (output, success)
        }
    };

    Ok(StepResult {
        step_id: step.id.clone(),
        rendered_prompt: rendered,
        run_id,
        transcript_path,
        output,
        success,
        skipped: false,
        items: Vec::new(),
        ..Default::default()
    })
```

> The final `Ok(StepResult { ... })` is the existing one (~line 1025) — leave it as is; only the dispatch/output computation moves into the `match`. The remote arm keeps the coordinator-side `run_id` + `transcript_path` as placeholders (the agent ran remotely with its own run id), exactly as the fan-out remote path keeps its per-item ids.

- [ ] **Step 5: Thread placement host into the step events**

In the step loop, the `StepStarted` emit (~line 751) and `StepCompleted` emit (~line 778). Add `host: step.host.clone(),` to each literal. Since `host` is only ever `Some` on a linear step, this is `None` for every other step kind automatically:

`StepStarted` (~line 751):
```rust
                &crate::executor::Event::StepStarted {
                    run_id: run_id.to_string(),
                    step_id: step.id.clone(),
                    kind: step_kind,
                    agent: step.agent.clone(),
                    host: step.host.clone(),
                },
```

`StepCompleted` (~line 778):
```rust
                        &crate::executor::Event::StepCompleted {
                            run_id: run_id.to_string(),
                            step_id: step.id.clone(),
                            success: result.success,
                            duration_ms,
                            host: step.host.clone(),
                        },
```

- [ ] **Step 6: Widen the CLI dispatcher trigger**

In `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (~line 159), change the short-circuit so a host-placed step also gets a dispatcher:

```rust
    if !workflow
        .steps
        .iter()
        .any(|s| s.distribute.is_some() || s.host.is_some())
    {
        return None;
    }
```

Also update the doc comment on `build_dispatcher_if_needed` (~line 147–152) — replace "no `distribute:` step" with "no `distribute:` or `host:` step".

- [ ] **Step 7: Run the new tests**

Run: `cargo test -p rupu-orchestrator -- placed_`
Expected: PASS (6 placed_* tests).

- [ ] **Step 8: Backward-compat — run the existing linear-runner and full suites**

Run:
```bash
cargo test -p rupu-orchestrator
cargo test -p rupu-cli --lib
```
Expected: all existing tests green (the no-host path is byte-for-byte; the CLI trigger change is additive). If `rupu-cli` has unrelated red under the worktree's Homebrew toolchain (per project memory), confirm the `fleet_unit_dispatcher` tests and any tests referencing `build_dispatcher_if_needed` pass and note the toolchain caveat in the report.

- [ ] **Step 9: Format and lint**

Run:
```bash
rustfmt crates/rupu-orchestrator/src/runner.rs crates/rupu-cli/src/fleet_unit_dispatcher.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
cargo clippy -p rupu-cli --no-deps
```
Expected: clippy clean for both (ignore the pre-existing `rupu-config` `is_none_or` lint — `--no-deps` scopes it out).

- [ ] **Step 10: Commit**

```bash
git add crates/rupu-orchestrator/src/runner.rs crates/rupu-cli/src/fleet_unit_dispatcher.rs
git commit -m "feat(multi-host): route placed linear step through UnitDispatcher (3b T3)"
```

---

## Task 4: End-to-end placed-step test

**Files:**
- Create: `crates/rupu-orchestrator/tests/placed_step_e2e.rs`
- Test: that file

**Interfaces:**
- Consumes: the public runner API the existing `crates/rupu-orchestrator/tests/distributed_fanout_e2e.rs` uses — the `RecordingDispatcher` pattern (~line 128 there), `OrchestratorRunOpts`, the top-level run entry point, and `UnitDispatcher` / `UnitDispatch` / `UnitOutcome`. Reuse that file's imports and opts-builder shape verbatim.
- Produces: an integration test proving a placed step runs "remotely" and its output feeds the next step, plus a no-host control identical to local behavior.

- [ ] **Step 1: Write the e2e test**

First open `crates/rupu-orchestrator/tests/distributed_fanout_e2e.rs` and copy its import block, its `RecordingDispatcher` (a public-API `UnitDispatcher` that records calls and returns a deterministic `UnitOutcome`), and the exact opts-construction + run-invocation it uses. Then create `crates/rupu-orchestrator/tests/placed_step_e2e.rs`:

```rust
//! End-to-end: a linear step with `host:` runs through the UnitDispatcher
//! port (a fake remote), its output feeds the downstream step, and the run
//! is one coherent run with per-step host attribution. A no-host control
//! confirms byte-for-byte local behavior is unchanged.

// <copy the imports + RecordingDispatcher from distributed_fanout_e2e.rs>
// RecordingDispatcher must: record (step_id, agent, rendered_prompt, host)
// per dispatch and return Ok(UnitOutcome { output: format!("REMOTE[{}]", unit.rendered_prompt), success: true, error: None }).

const WF: &str = r#"
name: placed-e2e
steps:
  - id: gather
    agent: gatherer
    prompt: "gather {{ inputs.topic }}"
    host: worker-1
  - id: summarize
    agent: summarizer
    prompt: "summarize: {{ steps.gather.output }}"
    host: worker-2
"#;

#[tokio::test]
async fn placed_steps_run_remotely_and_chain() {
    // <build opts exactly as distributed_fanout_e2e.rs does, with:
    //   - WF parsed via the same Workflow parser
    //   - inputs: { topic: "rust" }
    //   - unit_dispatcher: Some(Arc::new(RecordingDispatcher::new()))
    //   - a real RunStore in a tempdir so it is "one coherent run" >
    // Run the workflow to completion. >

    // Assert: the dispatcher saw two units —
    //   (agent "gatherer", prompt "gather rust", host "worker-1")
    //   (agent "summarizer", prompt contains "REMOTE[gather rust]", host "worker-2")
    // Assert: step_results[1].output == "REMOTE[summarize: REMOTE[gather rust]]"
    // Assert: both steps succeeded; the run completed (status Completed).
}

#[tokio::test]
async fn no_host_control_runs_locally() {
    // Same two-step workflow but with the `host:` lines removed and a
    // factory that yields deterministic local outputs (reuse the local
    // FakeFactory pattern from tests/linear_runner.rs). Provide
    // unit_dispatcher: None.
    // Assert: the run completes with two successful steps and the
    // dispatcher is NEVER consulted (None is fine — the local path runs).
}
```

> Fill the placeholder comments with concrete code mirroring the two sibling test files: `tests/distributed_fanout_e2e.rs` for the remote (RecordingDispatcher + RunStore) setup, and `tests/linear_runner.rs` for the local `FakeFactory` setup used by `no_host_control_runs_locally`. Use the exact struct/field/function names those files use — do not invent names. The two assertions that matter: (1) the placed chain threads `gather`'s remote output into `summarize`'s rendered prompt; (2) the no-host control runs to completion with no dispatcher.

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p rupu-orchestrator --test placed_step_e2e`
Expected: PASS (2 tests). If a name mismatch causes a compile error, reconcile against the sibling test files (they are the source of truth for the public API shape).

- [ ] **Step 3: Format, lint, full suite**

Run:
```bash
rustfmt crates/rupu-orchestrator/tests/placed_step_e2e.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
cargo test -p rupu-orchestrator
```
Expected: clippy clean; whole `rupu-orchestrator` suite green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-orchestrator/tests/placed_step_e2e.rs
git commit -m "test(multi-host): e2e placed-step chain + no-host control (3b T4)"
```

---

## Resolved Open Question (from the spec)

**`StepResultRecord.host` vs event-only:** resolved to **event-only**. Slice 3a added host attribution to the *unit* events only (`UnitStarted`/`UnitCompleted`), not to any persisted record; the persisted `StepResultRecord` carries no host today. For symmetry and minimal surface, 3b adds `host` to the *step* events (`StepStarted`/`StepCompleted`) only — no `StepResultRecord` field. The host is observable on the event stream the live view and CP run-detail already consume. If a future CP run-detail read path needs the host persisted, that's a small additive follow-up; it is not required for 3b's goals.

---

## Self-Review

**Spec coverage:**
- Spine decision 1 (`host: Option<String>` on linear Step, validation) → Task 1. ✅
- Spine decision 2 (reuse `UnitDispatcher`, index 0, build `StepResult` from `UnitOutcome`, local path byte-for-byte) → Task 3 Steps 3–4. ✅
- Spine decision 3 (no reassignment; fail honoring `continue_on_error`) → Task 3 `dispatch_placed_step`/`placed_failure` + tests. ✅
- Spine decision 4 (host attribution on step events) → Task 2 + Task 3 Step 5. ✅
- Spine decision 5 (widen `build_dispatcher_if_needed`; transports/guardrail documented) → Task 3 Step 6 + Global Constraints. ✅
- Goals (output feeds downstream; one coherent run with attribution; failure honors `continue_on_error`; no-host = byte-for-byte) → Tasks 3–4. ✅
- Testing section (model serde+validation; routing via fake dispatcher; backward-compat; step-event host; wiring; e2e) → Tasks 1, 2, 3, 4. ✅
- Open question resolved (event-only) → Resolved Open Question section. ✅

**Wiring test note:** the spec's "build_dispatcher_if_needed returns Some/None" test lives in `rupu-cli`. Task 3 Step 8 runs `rupu-cli --lib`; if `fleet_unit_dispatcher.rs` has an existing `#[cfg(test)] mod`, the implementer should add a `host:`-step assertion there mirroring the existing `distribute:` assertion. This is covered by the Task 3 reviewer scope (the trigger change + its test).

**Placeholder scan:** the only deliberately-templated content is the e2e test body (Task 4 Step 1), which directs the implementer to copy concrete code from two named sibling test files rather than inventing an API; every code-bearing step in Tasks 1–3 contains complete code. No "TBD"/"handle errors"/"add validation" placeholders.

**Type consistency:** `host: Option<String>` is used identically on `Step` (Task 1) and both events (Task 2); `dispatch_placed_step` returns `Result<(String, bool), RunWorkflowError>` and `run_linear_step` binds `(output, success)` from it (Task 3); `placed_failure` signature matches its two call sites; `UnitDispatch`/`UnitOutcome` field names match runner.rs ~lines 43–57.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-30-rupu-multi-host-slice-3b-plan.md`. Build via subagent-driven-development: fresh implementer per task, task review (spec + quality) after each, broad whole-branch review at the end, then a single PR to `main` (no self-merge — matt reviews before merge).
