# Step `actions:` enforcement + audit trail — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Turn the orphaned step `actions:` field into a real per-step permission narrowing over the agent's tool grant, authorable from the CP, with a `tool_audit` transcript trail — and delete the dead legacy action protocol.

**Architecture:** `actions:` entries are validated against the MCP tool catalog at parse time (mirroring the existing `validate_action_step`), and enforced at exactly one runtime point: `step_factory.rs`'s `agent_tools: spec.tools` becomes `spec.tools ∩ step.actions`. The agent runtime already denies out-of-roster calls at `runner.rs:835`, so narrowing composes with zero new enforcement machinery. A `tool_audit` event is emitted per catalog call from the existing `on_tool_call` hook (agent path) and the `ToolDispatcher` permission layer (action-node path).

**Tech Stack:** Rust 2021 (`rupu-orchestrator`, `rupu-agent`, `rupu-mcp`, `rupu-cp`), `thiserror`, `tokio`; TypeScript/React (`crates/rupu-cp/web`).

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-26-rupu-step-actions-enforcement-design.md` (decisions locked in §8). Branch: `step-actions-enforcement`, off `main`.
- **`actions:` semantics are fixed:** `effective = agent.tools ∩ step.actions` when `actions:` is NON-empty; `actions:` empty or absent ⇒ **unrestricted** (agent grant stands). A step may only narrow, never grant.
- **Compatibility is non-negotiable:** every existing workflow carries `actions: []` and must keep working unchanged. A parse test over `.rupu/workflows/*.yaml` guards every task.
- **`agent_tools: Option<Vec<String>>`** (`rupu-agent/src/runner.rs:536`): `None` = unrestricted. Mapping: empty `actions:` ⇒ pass `spec.tools` through untouched; non-empty `actions:` ⇒ `Some(spec.tools ∩ actions)` when `spec.tools` is `Some`, and `Some(actions)` when `spec.tools` is `None` (an unrestricted agent still gets narrowed).
- **The audit event is `tool_audit`. NEVER `action_emitted`** — the web already treats `action_emitted` as a dead/legacy shape and ignores it (`web/src/components/transcript/transcriptView.ts:353`), so reusing it would make the trail silently invisible.
- `#![deny(clippy::all)]`; `thiserror` for libs. Web: existing tokens/components, no new dep; `tsc` + vitest + `npm run build` clean.
- The 4 pre-existing `tests/linear_runner.rs` mock-provider flakes and the known `rupu-cli` toolchain-mismatch noise are baseline — verify against base, don't chase.

## File Structure

- `crates/rupu-orchestrator/src/workflow.rs` — `ActionsUnknownTool` + `ActionsOnActionStep` errors; validate each `actions:` entry against `rupu_mcp::tools::tool_catalog()`; the action-step shape rule.
- `crates/rupu-orchestrator/src/step_factory.rs` — the single enforcement point (`agent_tools` intersection) + the `tool_audit`-emitting `on_tool_call` closure.
- `crates/rupu-agent/src/runner.rs` — extend `OnToolCallCallback` with the call outcome so a denial is auditable; delete `action.rs`'s legacy protocol.
- `crates/rupu-agent/src/action.rs`, `crates/rupu-orchestrator/src/action_protocol.rs`, `crates/rupu-orchestrator/tests/action_allowlist.rs` — deleted in Task 5.
- `crates/rupu-mcp/src/dispatcher.rs` / `permission.rs` — emit `tool_audit` for action-node calls.
- `crates/rupu-cp/src/api/agents.rs` — expose the agent's frontmatter `tools:`.
- `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` — the `actions:` multi-select.

---

## T1 — make the field real

### Task 1: Validate `actions:` entries against the MCP catalog

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (error enum ~line 21; validation near `validate_action_step` ~1317 and its call sites ~1489/1539)
- Test: in-crate `#[cfg(test)]` in `workflow.rs`

**Interfaces:**
- Consumes: `rupu_mcp::tools::tool_catalog()` (already used by `validate_action_step`), `WorkflowParseError`.
- Produces: `WorkflowParseError::ActionsUnknownTool { step: String, tool: String }`, `WorkflowParseError::ActionsOnActionStep { step: String }`; a `fn validate_step_actions(step: &Step) -> Result<(), WorkflowParseError>` called from the same place `validate_step_shape`/`validate_action_step` are driven.

- [ ] **Step 1: Write the failing tests.** In `workflow.rs` tests:

```rust
#[test]
fn actions_entry_not_in_catalog_is_rejected() {
    let raw = r#"
name: w
steps:
  - id: s1
    agent: a
    prompt: p
    actions: ["issues.list", "open_pr"]
"#;
    match Workflow::parse(raw).unwrap_err() {
        WorkflowParseError::ActionsUnknownTool { step, tool } => {
            assert_eq!(step, "s1");
            assert_eq!(tool, "open_pr");
        }
        other => panic!("expected ActionsUnknownTool, got {other:?}"),
    }
}

#[test]
fn empty_string_action_entry_is_rejected() {
    let raw = "name: w\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n    actions: [\"\", \"\"]\n";
    assert!(matches!(
        Workflow::parse(raw).unwrap_err(),
        WorkflowParseError::ActionsUnknownTool { .. }
    ));
}

#[test]
fn valid_catalog_actions_are_accepted() {
    let raw = "name: w\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n    actions: [\"issues.list\", \"issues.get\"]\n";
    Workflow::parse(raw).expect("catalog names must be accepted");
}

#[test]
fn non_empty_actions_on_an_action_step_is_rejected() {
    let raw = r#"
name: w
steps:
  - id: s1
    action: issues.list
    with: { repo: "o/r" }
    actions: ["issues.list"]
"#;
    assert!(matches!(
        Workflow::parse(raw).unwrap_err(),
        WorkflowParseError::ActionsOnActionStep { .. }
    ));
}

#[test]
fn empty_actions_on_an_action_step_is_still_accepted() {
    let raw = "name: w\nsteps:\n  - id: s1\n    action: issues.list\n    with: { repo: \"o/r\" }\n    actions: []\n";
    Workflow::parse(raw).expect("empty actions on an action step must stay legal (compat)");
}

#[test]
fn every_sample_workflow_still_parses_after_actions_validation() {
    for entry in std::fs::read_dir(".rupu/workflows").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") { continue; }
        let raw = std::fs::read_to_string(&path).unwrap();
        Workflow::parse(&raw).unwrap_or_else(|e| panic!("{path:?} must still parse: {e:?}"));
    }
}
```

- [ ] **Step 2: Run them to verify they fail.** `cargo test -p rupu-orchestrator --lib actions_` — expect FAIL (`ActionsUnknownTool` does not exist).
- [ ] **Step 3: Add the error variants.** In `WorkflowParseError` (near the existing `ActionUnknownTool`):

```rust
#[error("step `{step}`: `actions:` entry `{tool}` is not a known MCP tool (see `rupu mcp` / GET /api/tools for the catalog)")]
ActionsUnknownTool { step: String, tool: String },
#[error("step `{step}`: an `action:` step must not carry a non-empty `actions:` allowlist — its tool is already explicit")]
ActionsOnActionStep { step: String },
```

- [ ] **Step 4: Implement the validation.**

```rust
/// Each `actions:` entry must name a tool in the MCP catalog. Mirrors
/// `validate_action_step`'s catalog lookup — same catalog, same crate dep.
/// `actions:` is a NARROWING allowlist for an AGENT step; an `action:`
/// step's tool is explicit, so a non-empty list there is a shape error
/// (an EMPTY list stays legal — it is already present repo-wide).
fn validate_step_actions(step: &Step) -> Result<(), WorkflowParseError> {
    if step.actions.is_empty() {
        return Ok(());
    }
    if step.action.is_some() {
        return Err(WorkflowParseError::ActionsOnActionStep { step: step.id.clone() });
    }
    let catalog = rupu_mcp::tools::tool_catalog();
    for tool in &step.actions {
        if !catalog.iter().any(|s| s.name == tool.as_str()) {
            return Err(WorkflowParseError::ActionsUnknownTool {
                step: step.id.clone(),
                tool: tool.clone(),
            });
        }
    }
    Ok(())
}
```

Call it for every step wherever `validate_step_shape(step)` is driven (including `parallel:` sub-steps, mirroring `validate_step_shape(sub)` at ~line 1520).

- [ ] **Step 5: Run the tests — all PASS**, including `every_sample_workflow_still_parses_after_actions_validation`.
- [ ] **Step 6: Full suite + commit.** `cargo test -p rupu-orchestrator` (only the 4 known flakes) + `cargo clippy -p rupu-orchestrator --all-targets`.

```bash
git add crates/rupu-orchestrator/src/workflow.rs
git commit -m "feat(orch): validate step actions: entries against the MCP tool catalog"
```

### Task 2: Enforce the narrowing at `step_factory`

**Files:**
- Modify: `crates/rupu-orchestrator/src/step_factory.rs` (`build_opts_for_step` ~123; `agent_tools: spec.tools` ~233)
- Test: in-crate `#[cfg(test)]` in `step_factory.rs`

**Interfaces:**
- Consumes: `Step.actions`, `spec.tools` (`Option<Vec<String>>`), the workflow lookup already performed in `build_opts_for_step`.
- Produces: `pub(crate) fn narrow_agent_tools(agent_tools: Option<Vec<String>>, step_actions: &[String]) -> Option<Vec<String>>` — the single intersection helper (unit-testable in isolation).

- [ ] **Step 1: Write the failing unit tests for the helper.**

```rust
#[test]
fn empty_actions_leaves_the_agent_grant_untouched() {
    let tools = Some(vec!["issues.list".to_string(), "issues.create".to_string()]);
    assert_eq!(narrow_agent_tools(tools.clone(), &[]), tools);
    assert_eq!(narrow_agent_tools(None, &[]), None);
}

#[test]
fn non_empty_actions_intersects_the_agent_grant() {
    let tools = Some(vec!["issues.list".into(), "issues.create".into()]);
    let got = narrow_agent_tools(tools, &["issues.list".to_string(), "issues.get".to_string()]);
    // `issues.get` is NOT granted by the agent -> dropped (narrow only, never grant)
    assert_eq!(got, Some(vec!["issues.list".to_string()]));
}

#[test]
fn actions_narrow_an_unrestricted_agent() {
    // agent_tools None == unrestricted; a step allowlist still narrows it
    let got = narrow_agent_tools(None, &["issues.list".to_string()]);
    assert_eq!(got, Some(vec!["issues.list".to_string()]));
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p rupu-orchestrator --lib narrow_agent_tools` — FAIL (function missing).
- [ ] **Step 3: Implement the helper.**

```rust
/// `actions:` is a NARROWING filter (spec §2): a step can only ever
/// reduce the agent's grant, never extend it. An EMPTY `actions:` means
/// "no extra restriction" — compat-critical, since every existing
/// workflow carries `actions: []` while relying on the agent's `tools:`.
/// `agent_tools == None` means "unrestricted"; a non-empty allowlist
/// still narrows that to exactly the listed tools.
pub(crate) fn narrow_agent_tools(
    agent_tools: Option<Vec<String>>,
    step_actions: &[String],
) -> Option<Vec<String>> {
    if step_actions.is_empty() {
        return agent_tools;
    }
    match agent_tools {
        None => Some(step_actions.to_vec()),
        Some(granted) => Some(
            granted
                .into_iter()
                .filter(|t| step_actions.iter().any(|a| a == t))
                .collect(),
        ),
    }
}
```

- [ ] **Step 4: Wire it at the single enforcement point.** In `build_opts_for_step`, resolve the parent step (the function already verifies it exists) and replace `agent_tools: spec.tools` with `agent_tools: narrow_agent_tools(spec.tools, &parent_step.actions)`. Nothing else changes — the agent runtime already denies out-of-roster calls at `rupu-agent/src/runner.rs:835`.
- [ ] **Step 5: Write the failing end-to-end test.** Using the crate's MockProvider harness: an agent granted `[issues.list, issues.create]` on a step with `actions: [issues.list]` produces `AgentRunOpts.agent_tools == Some(["issues.list"])`; the same agent on a step with `actions: []` produces `Some(["issues.list","issues.create"])`. Run → PASS after Step 4.
- [ ] **Step 6: Full suite + commit.**

```bash
git add crates/rupu-orchestrator/src/step_factory.rs
git commit -m "feat(orch): enforce step actions: as a narrowing filter over the agent tool grant"
```

### Task 3: Expose agent tools + the CP `actions:` picker

**Files:**
- Modify: `crates/rupu-cp/src/api/agents.rs` (add `tools` to the agent DTO)
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (agent type + `/api/tools` type if needed)
- Modify: `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx` (the field)
- Test: `crates/rupu-cp/src/api/agents.rs` in-crate test; `crates/rupu-cp/web/src/components/workflow-editor/StepForm.test.tsx`

**Interfaces:**
- Consumes: `GET /api/tools` (already mirrors `tool_catalog()`), the agent DTO.
- Produces: agent DTO field `tools: string[]`; a `StepForm` `actions:` multi-select that writes `StepNodeData.actions`.

- [ ] **Step 1: Failing Rust test — the agent DTO carries `tools`.** Assert `GET /api/agents` returns an agent whose `tools` array matches its frontmatter `tools:`. Run → FAIL.
- [ ] **Step 2: Add the field.** Additive `tools: Vec<String>` on the agent DTO in `api/agents.rs`, populated from the loaded agent spec. Run → PASS.
- [ ] **Step 3: Failing web test — the field renders and writes.** In `StepForm.test.tsx`: an agent step shows an `actions:` control listing catalog tools; selecting `issues.list` writes `actions: ["issues.list"]`; an `action:` step shows NO `actions:` control (spec §3b); with `actions: []` the control shows the text `unrestricted` and names the inherited agent. Run → FAIL.
- [ ] **Step 4: Implement the control.** In `StepForm`, for agent-bearing kinds only, render a multi-select fed by `/api/tools`:
  - each catalog tool as a toggle, grouped by prefix (`issues.*`, `scm.*`, `github.*`, `gitlab.*`);
  - a tool NOT in the selected agent's `tools` is still selectable but flagged `not granted by <agent>` (it will be narrowed away at runtime — spec §3c);
  - empty selection renders `unrestricted — inherits <agent>'s tools` (NOT "none");
  - writes through the existing `StepNodeData.actions` path (`workflowGraph.ts` already parses/emits it).
- [ ] **Step 5: Run web tests — PASS.** `npx tsc --noEmit`, `npx vitest run`, `npm run build` all clean (run from `crates/rupu-cp/web`).
- [ ] **Step 6: Commit.**

```bash
git add crates/rupu-cp/src/api/agents.rs crates/rupu-cp/web/src
git commit -m "feat(cp): expose agent tools + author step actions: from the workflow editor"
```

**T1 ships here.** At this point `actions:` validates, enforces, and is authorable. Merge + beta before starting T2.

---

## T2 — audit trail + legacy removal

### Task 4: The `tool_audit` event

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (`OnToolCallCallback` ~92; the deny point ~835)
- Modify: `crates/rupu-orchestrator/src/step_factory.rs` (the `on_tool_call` closure that emits)
- Modify: `crates/rupu-mcp/src/dispatcher.rs` (action-node path)
- Modify: `crates/rupu-cp/web/src/components/transcript/transcriptView.ts` (+ test) to render it
- Test: in-crate tests in `rupu-agent`, `rupu-orchestrator`, `rupu-mcp`; a vitest case for the web

**Interfaces:**
- Consumes: the existing `on_tool_call` hook (fires once per tool invocation — see `on_tool_call_fires_once_per_tool_invocation`), `narrow_agent_tools` (Task 2), the `ToolDispatcher` permission check.
- Produces: `pub type OnToolCallCallback = Arc<dyn Fn(&str, &str, bool) + Send + Sync>` (step_id, tool_name, **blocked**); a transcript event `{"type":"tool_audit","tool":..,"declared":bool,"granted":bool,"blocked":bool}`.

- [ ] **Step 1: Failing test — the callback reports a denial.** In `rupu-agent`: a run whose `agent_tools` excludes a tool the model calls fires `on_tool_call` with `blocked == true` and returns a tool error to the model (not a hard run failure). Run → FAIL (signature has no `blocked`).
- [ ] **Step 2: Extend the callback + emit at the deny point.** Change the type to `Fn(&str, &str, bool)`, pass `blocked` from the `let allowed = match &opts.agent_tools` site (~835), and update the 3 call sites (`step_factory` closure + the 2 test closures ~1435). Run → PASS.
- [ ] **Step 3: Failing test — the orchestrator emits `tool_audit`.** In `step_factory`: the `on_tool_call` closure (which captures the step's `actions` and the agent's granted `tools`) writes a `tool_audit` transcript line with correct `declared` (tool ∈ step.actions; `false` when `actions:` is empty), `granted` (tool ∈ agent tools), and `blocked`. Assert on the transcript JSONL. Run → FAIL.
- [ ] **Step 4: Implement the emit.** Build the event in the closure and append it to the step's transcript alongside existing events. Also emit `granted: false` + a `tracing::warn!` when the step declared a tool the agent does not grant (spec §3c — visible, not fatal). Run → PASS.
- [ ] **Step 5: Failing test — the action-node path audits too.** In `rupu-mcp`: a `ToolDispatcher::call` emits `tool_audit` (`declared: true`, `blocked` per the permission check) so an `action:` node's call is auditable. Run → implement → PASS.
- [ ] **Step 6: Failing web test — `tool_audit` renders.** In `transcriptView.test.ts`: a `tool_audit` line surfaces in the transcript view (and a `blocked: true` one is visibly marked). Confirm `action_emitted` remains ignored. Run → implement → PASS.
- [ ] **Step 7: Full suites + commit.** Rust suites + web `tsc`/vitest/build clean.

```bash
git add crates/rupu-agent/src crates/rupu-orchestrator/src crates/rupu-mcp/src crates/rupu-cp/web/src
git commit -m "feat: tool_audit transcript event for every catalog call (declared/granted/blocked)"
```

### Task 5: Delete the legacy action protocol

**Files:**
- Delete: `crates/rupu-agent/src/action.rs`, `crates/rupu-orchestrator/src/action_protocol.rs`, `crates/rupu-orchestrator/tests/action_allowlist.rs`
- Modify: `crates/rupu-agent/src/lib.rs` (drop `pub use action::{ActionEnvelope, ActionValidator}` ~29), `crates/rupu-orchestrator/src/lib.rs` (drop `pub use action_protocol::{validate_actions, ActionValidationResult}` ~22) and the `mod` declarations.

**Interfaces:** removes `ActionEnvelope`, `ActionValidator`, `validate_actions`, `ActionValidationResult` from both crates' public APIs.

- [ ] **Step 1: Confirm there are no remaining consumers.**

```bash
grep -rn "ActionEnvelope\|ActionValidator\|validate_actions\|ActionValidationResult" crates --include="*.rs"
```

Expected: only the files being deleted and their re-exports. (If anything else appears, STOP and report — do not delete a live dependency.)

- [ ] **Step 2: Delete the files + re-exports + `mod` lines.**
- [ ] **Step 3: Build the workspace.** `cargo build --workspace` — clean, no dangling references.
- [ ] **Step 4: Full suites.** `cargo test -p rupu-orchestrator -p rupu-agent` + `cargo clippy -p rupu-orchestrator -p rupu-agent --all-targets` clean; the `.rupu/workflows/*.yaml` parse test still green.
- [ ] **Step 5: Commit.**

```bash
git add -A
git commit -m "chore: delete the dead legacy action protocol (open_pr/file_write vocabulary)"
```

**T2 ships here.** Merge + beta.

## Operator gate (after T1, and again after T2)
matt authors a step narrowed to read-only `issues.*` on an agent that also has write tools, runs it, and confirms: (a) a write attempt is blocked and visible (T2: as a `tool_audit` with `blocked: true`), (b) the same agent on an `actions: []` step can still write, (c) a bogus verb is rejected at save with a clear message naming it.

## Self-review notes
- Spec coverage: §3a→T1/Task 1; §3b→Task 1 (shape rule); §3c→Task 2 (+ the warn/audit in Task 4 Step 4); §3d→Task 3; §4a/§4b→Task 4; §4c→Task 5; §5 compat→the sample-parse test in every task; §6 testing→distributed across tasks; §7 build order→T1 = Tasks 1-3, T2 = Tasks 4-5.
- Type consistency: `narrow_agent_tools` (Task 2) is consumed by Task 4's closure; `OnToolCallCallback`'s new 3-arg shape (Task 4) updates all 3 call sites; `ActionsUnknownTool`/`ActionsOnActionStep` (Task 1) are the only new error variants.
- The `action_emitted`-vs-`tool_audit` trap is called out in Global Constraints so no task can regress into the silently-ignored name.
