# Gate Nodes & Action Steps — Plan 2: Action Execution

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** PR ② of the arc in `docs/superpowers/specs/2026-07-23-rupu-workflow-gate-and-action-nodes-design.md` — `action:` steps execute for real through the existing in-process MCP tool layer (no agent, no tokens), with parse-time catalog validation, readonly-mode Write refusal, output binding, and an audit transcript line. Replaces Plan 1's `ActionStepsNotYetSupported` stub.

**Architecture:** The runner executes an action step via `rupu_mcp::ToolDispatcher::call(name, args) -> Result<String, McpError>` — the same object the agent loop's `McpToolAdapter` calls (crates/rupu-mcp/src/dispatcher.rs:25); `serve_in_process` is NOT on the invoke path. A new `OrchestratorRunOpts.action_dispatcher: Option<Arc<ToolDispatcher>>` carries it; the CLI builds it from the same `Arc<rupu_scm::Registry>` + mode string it already gives `DefaultStepFactory`. Permission: `McpPermission::new(parse_mode, vec!["*".into()])` — the tool is author-declared in YAML so no allowlist narrowing; readonly-mode Write refusal comes free (`permission.rs:44-64`). Platform/tracker fallback comes free via `resolve_platform`/`resolve_tracker` inside each `dispatch_*`.

**Tech Stack:** Rust; new workspace-internal dep `rupu-orchestrator → rupu-mcp` (verified acyclic: rupu-mcp deps only rupu-scm/rupu-tools/rupu-config).

## Global Constraints

- Workspace deps only (add `rupu-mcp = { path = "../rupu-mcp" }` to crates/rupu-orchestrator/Cargo.toml, version-free per workspace convention — check how the crate's other path deps are declared and match).
- NO new `Event` variants; NO new `StepKind` variants (Action exists since Plan 1).
- thiserror (library) / anyhow (CLI). `#![deny(clippy::all)]`. Never package-wide cargo fmt.
- Fail-closed everywhere: an action step must never silently no-op. Missing dispatcher wiring, unknown tool, denied permission — all are loud step/parse errors.
- Baseline: 4 flaky `linear_runner.rs` tests (mock-provider) + rupu-cli ANSI/session-test redness are pre-existing; compare failures against base before debugging.
- Line refs are against v0.65.1 main (`65d8642b`); re-locate by quoted code if drifted.

---

### Task 1: Parse-time catalog validation of `action:` steps

**Files:**
- Modify: `crates/rupu-orchestrator/Cargo.toml` (add rupu-mcp dep), `crates/rupu-orchestrator/src/workflow.rs` (new validation pass in `Workflow::parse`)
- Test: `crates/rupu-orchestrator/tests/workflow_parse.rs`

**Interfaces:**
- Consumes: `rupu_mcp::tools::tool_catalog() -> Vec<ToolSpec>` (`ToolSpec { name: &'static str, input_schema: serde_json::Value, kind: ToolKind, .. }`, crates/rupu-mcp/src/tools/mod.rs:24-42).
- Produces: `WorkflowParseError::ActionUnknownTool { step: String, tool: String }` and `ActionInvalidParams { step: String, tool: String, detail: String }`. Validation helper `fn validate_action_step(step_id: &str, tool: &str, with: Option<&serde_json::Value>) -> Result<(), WorkflowParseError>` applied to top-level action steps AND `on_reject` action subs AND (forward-looking, they parse already) `notify` entries.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn action_step_unknown_tool_fails_parse() {
    let yaml = r#"
name: bad
steps:
  - id: x
    action: scm.prs.frobnicate
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("scm.prs.frobnicate"), "got: {err}");
}

#[test]
fn action_step_unknown_with_key_fails_parse() {
    let yaml = r#"
name: bad
steps:
  - id: x
    action: issues.comment
    with:
      issue_number: "7"
      bodyy: "typo key"
"#;
    let err = Workflow::parse(yaml).unwrap_err();
    assert!(err.to_string().contains("bodyy"), "got: {err}");
}

#[test]
fn action_step_missing_required_key_fails_parse() {
    // issues.comment requires `body` (check the actual CommentIssueArgs schema
    // via tool_catalog() and adjust the YAML to omit one genuinely-required key).
    let yaml = r#"
name: bad
steps:
  - id: x
    action: issues.comment
    with:
      issue_number: "7"
"#;
    assert!(Workflow::parse(yaml).is_err());
}

#[test]
fn action_step_valid_with_templated_values_parses() {
    // Values are templates rendered at runtime — parse validates KEYS only.
    let yaml = r#"
name: ok
steps:
  - id: seed
    agent: a
    prompt: p
  - id: x
    action: issues.comment
    with:
      issue_number: "{{ event.payload.issue.number }}"
      body: "{{ steps.seed.output }}"
"#;
    assert!(Workflow::parse(yaml).is_ok());
}

#[test]
fn notify_and_on_reject_action_entries_validate_too() {
    let yaml = r#"
name: bad
steps:
  - id: g
    approval:
      on_reject:
        - id: c
          action: nope.nope
"#;
    assert!(Workflow::parse(yaml).is_err());
}
```

Adjust key names to the REAL schemas: dump `tool_catalog()` first (`cargo test -p rupu-mcp schema_snapshot -- --nocapture` or read `crates/rupu-mcp/src/tools/issues.rs` `CommentIssueArgs`) and use genuine required/optional keys; the test bodies above are the shape, the exact keys must match the schemas.

- [ ] **Step 2: Run to confirm RED** — `cargo test -p rupu-orchestrator --test workflow_parse action_step 2>&1 | tail -5` (unknown tool currently parses fine).

- [ ] **Step 3: Implement**

Key-level schema check (no jsonschema crate — walk the schema Value):

```rust
/// Parse-time validation of an action invocation against the static MCP
/// catalog (spec §4.2): the tool must exist, `with:` keys must be schema
/// properties, and required keys must be present. VALUES are not checked —
/// they may be minijinja templates rendered at runtime (the dispatcher's
/// typed serde parse re-validates then).
fn validate_action_step(
    step_id: &str,
    tool: &str,
    with: Option<&serde_json::Value>,
) -> Result<(), WorkflowParseError> {
    let catalog = rupu_mcp::tools::tool_catalog();
    let Some(spec) = catalog.iter().find(|s| s.name == tool) else {
        return Err(WorkflowParseError::ActionUnknownTool {
            step: step_id.to_string(),
            tool: tool.to_string(),
        });
    };
    let schema = &spec.input_schema;
    let props = schema.get("properties").and_then(|p| p.as_object());
    let empty = serde_json::Map::new();
    let with_map = match with {
        None => &empty,
        Some(serde_json::Value::Object(m)) => m,
        Some(_) => {
            return Err(WorkflowParseError::ActionInvalidParams {
                step: step_id.to_string(),
                tool: tool.to_string(),
                detail: "`with:` must be a mapping".into(),
            })
        }
    };
    if let Some(props) = props {
        for key in with_map.keys() {
            if !props.contains_key(key) {
                return Err(WorkflowParseError::ActionInvalidParams {
                    step: step_id.to_string(),
                    tool: tool.to_string(),
                    detail: format!("unknown parameter `{key}`"),
                });
            }
        }
    }
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for req in required.iter().filter_map(|v| v.as_str()) {
            if !with_map.contains_key(req) {
                return Err(WorkflowParseError::ActionInvalidParams {
                    step: step_id.to_string(),
                    tool: tool.to_string(),
                    detail: format!("missing required parameter `{req}`"),
                });
            }
        }
    }
    Ok(())
}
```

Call it from `Workflow::parse`'s per-step sweep for: `step.action`, each `on_reject` sub with `action`, each `notify` entry. Error variants follow the file's thiserror style.

- [ ] **Step 4: GREEN + full crate suite** — `cargo test -p rupu-orchestrator 2>&1 | grep "test result"`.
- [ ] **Step 5: Commit** — `feat(orchestrator): parse-time catalog validation for action steps`

---

### Task 2: Runner executes action steps through ToolDispatcher

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (OrchestratorRunOpts + replace the `ActionStepsNotYetSupported` stub block; the stub sits in the step loop after the branch block)
- Test: `crates/rupu-orchestrator/tests/action_step.rs` (new)

**Interfaces:**
- Consumes: `rupu_mcp::ToolDispatcher` (`pub async fn call(&self, name: &str, args: Value) -> Result<String, McpError>`), `rupu_mcp::McpError` (variants incl. `PermissionDenied { tool, reason }`), Plan 1's action-step schema.
- Produces: `OrchestratorRunOpts.action_dispatcher: Option<Arc<rupu_mcp::ToolDispatcher>>` (default None — extend the struct's `Default`/builders and ALL construction sites; `cargo build --workspace` enumerates them); `RunWorkflowError::ActionStepsNotYetSupported` REMOVED and replaced by `ActionDispatcherMissing { step: String }` ("action step `{step}` requires runtime wiring — this entry point does not provide an SCM dispatcher"); action `StepResult { kind: Action, output: <dispatcher JSON string>, success, .. }`. Template rendering of `with:` values: every JSON string leaf is rendered with the step context (same `render_step_prompt` machinery); non-string leaves pass through.

- [ ] **Step 1: Failing tests** (`tests/action_step.rs`)

Harness recipe (from the recon — no agent, no MCP server):

```rust
// Fake connector: implement rupu_scm::connectors::RepoConnector's create_pr /
// comment_pr (and IssueConnector::comment_issue) recording calls into an
// Arc<Mutex<Vec<..>>>; unimplemented methods return ScmError as the trait's
// default-fail or unimplemented!() — model on FakePrConnector in
// crates/rupu-cli/src/cmd/autoflow.rs (~line 16844).
let mut reg = rupu_scm::Registry::empty();          // needs rupu-scm test-helpers feature as a dev-dep of rupu-orchestrator
reg.insert_repo_connector(Platform::Github, Arc::new(fake));
let dispatcher = Arc::new(rupu_mcp::ToolDispatcher::new(
    Arc::new(reg),
    rupu_mcp::McpPermission::new(PermissionMode::Ask, vec!["*".into()]),
));
// OrchestratorRunOpts { action_dispatcher: Some(dispatcher), .. } with the
// existing FakeFactory/tempdir harness from tests/gate_node.rs.
```

Cases:
```rust
// 1. happy path: workflow [agent step "seed" → action scm.prs.comment with
//    templated body "{{ steps.seed.output }}"] — fake connector records the
//    rendered body; StepResult kind Action, success=true, output = the
//    dispatcher's JSON string; events.jsonl has step_started+step_completed
//    for the action step.
// 2. templated values render: with-value "{{ steps.seed.output }}" arrives at
//    the connector rendered (assert on the recorded call).
// 3. connector error → step_failed + RunWorkflowError (run fails) — and with
//    continue_on_error: true, the run continues and records success=false.
// 4. readonly mode (McpPermission::new(Readonly, ...)): Write tool refused —
//    step fails with a message containing "readonly"; no connector call recorded.
// 5. action_dispatcher: None → ActionDispatcherMissing error naming the step.
// 6. action step inside on_reject cleanup chain: rejected gate's cleanup
//    dispatches the action for real (extend the Task-4-Plan-1 harness; the
//    cleanup mirror currently records action subs as failures — this task
//    fixes that path too).
```

- [ ] **Step 2: RED** — `cargo test -p rupu-orchestrator --test action_step 2>&1 | tail -5` (stub error fires).

- [ ] **Step 3: Implement**

Replace the stub block with (shape — final code follows the file's conventions):

```rust
        if let Some(tool) = &step.action {
            let step_kind = crate::runs::StepKind::Action;
            // events: StepStarted (agent: None) …
            let Some(dispatcher) = opts.action_dispatcher.as_ref() else {
                return Err(RunWorkflowError::ActionDispatcherMissing { step: step.id.clone() });
            };
            let args = render_action_args(step.with.as_ref(), &ctx, render_mode(opts.strict_templates))
                .map_err(|e| RunWorkflowError::Render { step: step.id.clone(), source: e })?;
            let timer = std::time::Instant::now();
            let outcome = dispatcher.call(tool, args).await;
            // success → StepCompleted + StepResult{output: json_string};
            // Err(McpError) → StepFailed{error: e.to_string()} + continue_on_error
            //   handling identical to the linear arm's failure path;
            // audit: see Task 3 (transcript line) — hook point here.
            …
            continue;
        }
```

`render_action_args`: deep-walk `Option<&Value>` (None → `json!({})`), rendering every string leaf through the existing template renderer; objects/arrays recurse; numbers/bools/null pass through. Also route the cleanup-chain mirror's Action handling (`run_reject_cleanup`) through the same execution helper — extract `execute_action_step(opts_like, step, ctx) -> StepResult`-style helper so the main loop and the cleanup mirror share it (this is the reuse point Plan 4's notify hooks will also call — name it and keep it free of main-loop-only state).

- [ ] **Step 4: GREEN + suites** — `cargo test -p rupu-orchestrator` + `cargo build --workspace 2>&1 | tail -3` (all OrchestratorRunOpts construction sites updated).
- [ ] **Step 5: Commit** — `feat(orchestrator): action steps execute through the in-process MCP dispatcher`

---

### Task 3: Audit transcript line + CLI wiring

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (audit line in the action execution helper), `crates/rupu-cli/src/cmd/workflow.rs` + `crates/rupu-cli/src/resume.rs` (+ `cp_launcher.rs` path if it builds OrchestratorRunOpts directly — grep `OrchestratorRunOpts {` in rupu-cli and wire every site), `crates/rupu-cli/src/cmd/cron.rs` if it constructs opts.
- Test: extend `crates/rupu-orchestrator/tests/action_step.rs`; CLI wiring is compile-checked + one existing-path smoke (`cargo test -p rupu-cli --lib` baseline-compare).

**Interfaces:**
- Consumes: Task 2's execution helper; the step factory's existing `Arc<rupu_scm::Registry>` + `mode_str` (`DefaultStepFactory` fields, step_factory.rs:47-48).
- Produces: every CLI entry point that runs workflows passes `action_dispatcher: Some(Arc::new(ToolDispatcher::new(registry, McpPermission::new(parse_mode_for_runtime(&mode_str), vec!["*".into()]))))` — one shared builder fn in rupu-cli (`fn action_dispatcher_for(registry: &Arc<Registry>, mode_str: &str) -> Arc<ToolDispatcher>`); an action step writes ONE transcript JSONL line at its `transcript_path`: the existing action-envelope shape (`{"type":"action_emitted","kind":"<tool>","payload":<rendered args>,"applied":true/false,"error":<string|null>}` — read `crates/rupu-agent/src/action.rs` and the transcript writer for the exact current envelope encoding and MATCH it; if the transcript format has a header line requirement, honor it — check how agent transcripts open).

- [ ] **Step 1: Failing test** — extend case 1: after the run, the action step's `transcript_path` exists and contains one line whose JSON has the tool name and `applied: true`; failure case has `applied: false` + error.
- [ ] **Step 2: RED**, **Step 3: implement** (audit line in the shared helper; `parse_mode_for_runtime` is in rupu-agent — reuse, do not duplicate: check its visibility, re-export if needed), **Step 4: GREEN + `cargo build --workspace`**, then wire ALL rupu-cli opts-construction sites via the shared builder.
- [ ] **Step 5: Commit** — `feat(cli): wire the action dispatcher into every workflow entry point`

---

### Task 4: Sample + docs + full verification

**Files:**
- Create: `.rupu/workflows/action-demo.yaml` — uses a READ tool so dogfooding never writes: e.g. `action: scm.prs.list` with `with: { owner: "Section9Labs", repo: "rupu", state: "open" }` (verify key names against the real `ListPrsArgs` schema) followed by an agent step consuming `{{ steps.list_prs.output }}`.
- Modify: `CLAUDE.md` — update the Plan-1 sentence: action steps now execute (Plan 2); `docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-1-schema-and-runner.md` — tick the "Deferred to later plans" Plan-2 line as done (one-word edit, optional).

- [ ] **Step 1:** Write sample; `cargo run -p rupu-cli -- workflow show action-demo --view full` parses clean; `workflow list` discovers it.
- [ ] **Step 2:** Full verification: `cargo test -p rupu-orchestrator` (all green except the 4 documented flakes), `cargo test -p rupu-mcp`, `cargo build --workspace` clean, `cargo clippy -p rupu-orchestrator -p rupu-cli 2>&1 | grep -c "^error"` → 0 (toolchain-artifact caveat applies).
- [ ] **Step 3:** Commit — `feat(orchestrator): action-demo sample + docs`

---

## Deferred (unchanged from Plan 1's ledger)
Plan 3: renderers/editor (incl. ActionNode, rejected-gate-without-step-result shape). Plan 4: notify execution (calls Task 2's shared helper), cp-serve gate sweep + orphan reaper, unattended cleanup permission mode, `[scm.default]` config wiring in `Registry::default_platform` (recon found it's still the v0 first-registered fallback — NOT this plan's scope).
