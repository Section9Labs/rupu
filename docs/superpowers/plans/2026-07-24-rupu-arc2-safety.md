# Arc 2 — Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close `ISSUES.md` **I-22 … I-27** — the safety cluster: an agent can read outside its workspace, a readonly run's cleanup runs with writes enabled, a list command has external side effects, and the documented action-protocol check doesn't exist.

**Architecture:** Six independent fixes, no shared plumbing. Two are one-line guards applied via an existing helper (`path_scope::is_inside`). One persists a field on `RunRecord` so the cleanup path can inherit the run's permission mode instead of defaulting to `ask`. One removes chain execution from a listing path. One corrects a lying comment and adds real narrowing. One deletes dead code. Ordered by severity, and by putting the `RunRecord` change (which other tasks read) before the paths that consume it.

**Tech Stack:** Rust (rupu-tools, rupu-orchestrator, rupu-cli), `cargo test`.

## Global Constraints

- **Validation bar (charter §3.2):** a parse or construction test closes nothing. Each fix needs a test observing the behavior **at the boundary that matters** — an escape actually refused, a Write tool actually denied, a chain actually not executed.
- **Two behavior decisions are already made by the operator — do not re-litigate:**
  - **I-25:** the listing path **finalizes state only**. It still expires overdue gates so status stays truthful, but chain execution moves entirely to the `cp serve` gate sweep (which exists and is default-on).
  - **I-23:** **document only, no default change.** Tightening the default would silently stop every existing autoflow from firing on outside contributors. Document the fields, state the open default plainly, and recommend `authors_from: collaborators` for unattended autoflows.
- Workspace deps only; `#![deny(clippy::all)]`; `unsafe_code` forbidden. thiserror in libraries, anyhow in `rupu-cli`.
- Never run package-wide `cargo fmt` — per-file only.
- **Known-red baseline, do not chase:** 4 `linear_runner.rs` tests (I-4), ~11 `output::printer::tests::*` ANSI assertions. Compare against a clean checkout before treating a failure as yours.
- **Tests touching `RUPU_MOCK_PROVIDER_SCRIPT` must acquire `crate::test_support::ENV_LOCK`** — it is a process-global env seam and Rust runs tests in parallel threads. Arc 1 fixed a flake caused by exactly this.
- **Close each issue in the same commit that fixes it:** move its `ISSUES.md` row to `## Fixed` with the PR number and the validation that proves it, and move its Tier-2 write-up too. Never delete a row.
- Line refs are `main` at `022776dc`; re-locate by the quoted code if drifted.

---

### Task 1: I-22 — contain `grep` and `ast_grep` to the workspace

**Files:**
- Modify: `crates/rupu-tools/src/grep.rs:70-75`, `crates/rupu-tools/src/ast_grep.rs:146-151`
- Test: same two files (in-file `#[cfg(test)] mod tests` — check whether one exists; if not, add one)

**Interfaces:**
- Consumes: `crate::path_scope::is_inside(root: &Path, candidate: &Path) -> bool` (`crates/rupu-tools/src/path_scope.rs:9`), already `pub(crate)` and already used by `read_file.rs:60`, `write_file.rs`, `edit_file.rs`.
- Produces: nothing new. Both tools return the same shape `read_file` does on refusal.

Both tools currently build their search root as:
```rust
let search_path = i
    .path
    .as_deref()
    .map(|p| ctx.workspace_path.join(p))
    .unwrap_or_else(|| ctx.workspace_path.clone());
```
`Path::join` **replaces** the base for an absolute argument and does not normalize `..`, so `path: "/etc"` and `path: "../.."` both escape. `glob` is NOT affected — it takes no user path (`glob.rs:54`).

- [ ] **Step 1: Write the failing tests**

Add to `crates/rupu-tools/src/grep.rs`. Mirror `read_file`'s refusal shape — read `read_file.rs:58-65` first and match how it builds `err_output` and phrases the message:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    fn ctx_in(dir: &std::path::Path) -> ToolContext {
        // Build a minimal ToolContext rooted at `dir`. Copy the construction
        // from an existing tool test in this crate (read_file.rs or
        // edit_file.rs have one) rather than inventing fields.
        todo!("copy from the sibling tool test")
    }

    #[tokio::test]
    async fn an_absolute_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("in.txt"), "needle\n").unwrap();
        let out = Grep
            .invoke(
                serde_json::json!({ "pattern": "needle", "path": "/etc" }),
                &ctx_in(dir.path()),
            )
            .await
            .unwrap();
        assert!(
            out.output.contains("escapes workspace"),
            "absolute path must be refused, got: {}",
            out.output
        );
    }

    #[tokio::test]
    async fn a_parent_traversal_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("work");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(dir.path().join("outside.txt"), "needle\n").unwrap();
        let out = Grep
            .invoke(
                serde_json::json!({ "pattern": "needle", "path": "../" }),
                &ctx_in(&inner),
            )
            .await
            .unwrap();
        assert!(out.output.contains("escapes workspace"), "got: {}", out.output);
        assert!(
            !out.output.contains("outside.txt"),
            "refusal must not leak results from outside the workspace: {}",
            out.output
        );
    }

    #[tokio::test]
    async fn an_in_workspace_path_still_searches() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.txt"), "needle\n").unwrap();
        let out = Grep
            .invoke(
                serde_json::json!({ "pattern": "needle", "path": "src" }),
                &ctx_in(dir.path()),
            )
            .await
            .unwrap();
        assert!(out.output.contains("a.txt"), "in-workspace search broke: {}", out.output);
    }
}
```

Replace the `todo!` with the real `ToolContext` construction copied from a sibling tool's test — do not invent fields. The struct name for the tool (`Grep`) must match what the file actually defines. Add the mirror-image trio to `ast_grep.rs` (its tests need `ast-grep` on PATH — if the binary is absent in this environment, gate those tests the way any existing ast_grep test does, and say so in the report).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-tools --lib grep 2>&1 | tail -6`
Expected: FAIL — the absolute/traversal cases currently search outside and return matches instead of a refusal.

- [ ] **Step 3: Implement**

In both files, add the guard immediately after computing `search_path`:

```rust
        let search_path = i
            .path
            .as_deref()
            .map(|p| ctx.workspace_path.join(p))
            .unwrap_or_else(|| ctx.workspace_path.clone());
        // Containment: `join` replaces the base for an absolute argument and
        // does not normalize `..`, so an agent-supplied path can leave the
        // workspace. Same guard the write tools use (ISSUES.md I-22).
        if !crate::path_scope::is_inside(&ctx.workspace_path, &search_path) {
            return Ok(err_output(
                started,
                format!("path {} escapes workspace", i.path.as_deref().unwrap_or(".")),
            ));
        }
```

Match each file's real error-return helper — `grep.rs` and `ast_grep.rs` may not both have `err_output`; use whatever shape that file already returns on a handled failure.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rupu-tools --lib 2>&1 | grep "test result"` → all pass.

- [ ] **Step 5: Close I-22 and commit**

```bash
git add crates/rupu-tools/src/grep.rs crates/rupu-tools/src/ast_grep.rs ISSUES.md
git commit -m "fix(tools): contain grep and ast_grep to the workspace (I-22)"
```

---

### Task 2: I-24 — cleanup inherits the run's permission mode

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs` (`RunRecord` — add the field; the run-creation path that populates it), `crates/rupu-cli/src/resume.rs` (`rebuild_opts_from_disk`'s `mode.unwrap_or("ask")`), `crates/rupu-cli/src/cmd/workflow.rs` (the reject/approve paths that build cleanup opts)
- Test: `crates/rupu-orchestrator/src/runs.rs` in-file tests + a `crates/rupu-cli/tests/` integration test

**Interfaces:**
- Produces: `RunRecord.permission_mode: Option<String>` — the effective mode the run was launched with, persisted at creation. `#[serde(default)]` so existing `run.json` files still load (they will read as `None`).
- Consumes: `rebuild_opts_from_disk` reads it and uses it in place of the `"ask"` default.

Today the run's `--mode` is never persisted (only `resume_mode`, set by the web-resume path), so `rebuild_opts_from_disk` falls back to `"ask"`, which permits Write tools. A `--mode readonly` run's `on_reject` chain therefore runs with writes enabled.

Precedence for the rebuilt opts, most specific first: an explicit `--mode` on the reject command (if one is added) → `record.resume_mode` → **`record.permission_mode`** (new) → `"ask"`.

- [ ] **Step 1: Write the failing test**

Integration test in `crates/rupu-cli/tests/` (model on `policy_lock.rs`, added in Arc 1, which drives `rupu_cli::run(...)` end to end):

```rust
// ISSUES.md I-24: a run launched --mode readonly must not have its on_reject
// cleanup execute with write tools enabled.
#[test]
fn reject_cleanup_inherits_a_readonly_run_mode() {
    // 1. Launch a gate workflow with --mode readonly under the mock provider
    //    (acquire crate::test_support::ENV_LOCK if you set RUPU_MOCK_PROVIDER_SCRIPT).
    //    Its on_reject chain contains a step whose agent attempts write_file.
    // 2. Reject the parked gate.
    // 3. Assert the cleanup step's write was DENIED — the file it would have
    //    created does not exist — and that the run still ends Rejected.
    // The pre-fix behavior is that the file IS created.
}
```

Write it concretely against the real harness. The binding assertion is a filesystem effect (the write did not happen), not a config value — a config assertion would not prove the mode reached the tool layer.

- [ ] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN**

Add the field, populate it where the run record is created (find it: `grep -n "RunRecord {" crates/rupu-orchestrator/src/runs.rs` and the CLI's run-start path), and consume it in `rebuild_opts_from_disk`. Existing `run.json` files must still deserialize — verify with a test that a record JSON lacking the field loads.

- [ ] **Step 5: Close I-24 and commit**

```bash
git commit -m "fix(cli): on_reject cleanup inherits the run's permission mode (I-24)"
```

---

### Task 3: I-25 — the listing path stops executing chains

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (the `runs()` listing path's lazy-expiry branch — find it via `grep -n "run_reject_cleanup" crates/rupu-cli/src/cmd/workflow.rs`)
- Test: `crates/rupu-cli/tests/`

**Interfaces:**
- Consumes: `RunStore::expire_if_overdue` (unchanged — it still finalizes state and appends the terminal event).
- Produces: no new API. The listing path calls `expire_if_overdue` and stops; it no longer calls `build_reject_cleanup_opts` / `run_reject_cleanup`.

**Operator decision (settled):** listing finalizes state only. The `cp serve` gate sweep — default-on at 60s — owns chain execution. Keep the `approve` and `reject` command paths executing chains as they do now; only the **listing** path changes.

Because the sweep also calls `expire_if_overdue`, and that call is what flips the run terminal, there is a real ordering consequence to handle: if the listing expires a gate to `Rejected` first, the sweep will later see an already-terminal run and skip it, so the chain never runs at all. **Verify this** — read `run_gate_sweep`'s decision logic in `crates/rupu-cli/src/cmd/cp.rs` — and if it holds, the listing path must *not* expire timeout-**reject** gates either; it should leave those for the sweep and only expire the `fail` case (which has no chain). State what you found and what you did.

- [ ] **Step 1: Write the failing test**

```rust
// ISSUES.md I-25: `rupu workflow runs` must not execute on_reject chains.
#[test]
fn listing_runs_does_not_execute_a_reject_cleanup_chain() {
    // Park a gate with timeout_seconds in the past and on_timeout: reject,
    // whose on_reject chain would create a marker file.
    // Invoke the runs listing.
    // Assert: the marker file does NOT exist (no chain ran).
    // Assert the listing still printed the run.
}
```

- [ ] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN**

Add a comment at the changed site explaining *why* the listing doesn't run chains (a read command must not have external side effects) and pointing at the sweep as the owner.

- [ ] **Step 5: Close I-25 and commit**

```bash
git commit -m "fix(cli): workflow runs listing no longer executes cleanup chains (I-25)"
```

---

### Task 4: I-26 — narrow an action step by its own `actions:`, and fix the lying comment

**Files:**
- Modify: `crates/rupu-cli/src/resume.rs` (`action_dispatcher_for` — the `vec!["*".into()]` allowlist and its doc comment), plus wherever the per-step dispatcher is selected in `crates/rupu-orchestrator/src/runner.rs`'s action arm
- Test: `crates/rupu-orchestrator/tests/action_step.rs` (exists, from the gate/action arc)

**Interfaces:**
- Consumes: `Step.actions: Vec<String>` — already enforced for *agent* steps via `step_factory::narrow_agent_tools` (`crates/rupu-orchestrator/src/step_factory.rs:246,407`) since #533/#537.
- Produces: an action step whose own `actions:` list is non-empty may only invoke a tool named in it; an empty `actions:` keeps today's behavior (any catalog tool).

The doc comment on `action_dispatcher_for` currently claims an `action:` step "sees exactly the same allow/deny surface a `tools:`-using agent step would" — false, since the agent path narrows and this one passes `["*"]`.

Scope judgment: **at minimum** correct the comment (that is a pure-truth fix and non-negotiable). The narrowing is the better fix and is preferred; if it turns out to require threading the step through a seam that doesn't exist, implement the comment fix, file the narrowing as a new issue, and say so — do not leave the comment lying either way.

- [ ] **Step 1: Write the failing test**

```rust
// ISSUES.md I-26: a step's own `actions:` must narrow what its `action:` can call.
#[tokio::test]
async fn a_step_actions_allowlist_narrows_its_own_action_call() {
    // A workflow step: action: scm.prs.create, actions: ["issues.comment"].
    // Assert the dispatch is REFUSED (permission denied naming the tool),
    // and the fake connector recorded zero create calls.
}

#[tokio::test]
async fn an_empty_actions_list_leaves_the_action_unnarrowed() {
    // Same step with actions: [] — the call succeeds (today's behavior).
}
```

Use the existing fake-connector + `ToolDispatcher` harness in `action_step.rs`.

- [ ] **Step 2: RED**, **Step 3: implement**, **Step 4: GREEN**
- [ ] **Step 5: Close I-26 and commit**

```bash
git commit -m "fix(orchestrator): a step's actions: narrows its own action: call (I-26)"
```

---

### Task 5: I-27 — delete the dead action-protocol validator

**Files:**
- Modify: `crates/rupu-orchestrator/src/action_protocol.rs` (delete `validate_actions`), `crates/rupu-orchestrator/src/lib.rs` (drop the re-export), `crates/rupu-agent/src/action.rs:21` (delete the `ActionValidator` stub), `crates/rupu-agent/src/lib.rs:29` (drop that re-export)
- Delete: `crates/rupu-orchestrator/tests/action_allowlist.rs` if it tests only the deleted fn — check first; keep any test that covers something still live

**Interfaces:** removes `validate_actions` and `ActionValidator` from both crates' public surfaces. Confirm no external caller with:
`grep -rn "validate_actions\|ActionValidator" --include="*.rs" crates`
Expected before deletion: only the declarations, the re-exports, and `action_allowlist.rs`'s tests.

**Do not touch the docs here** — README / `docs/agent-format.md` / `docs/triggers.md` all describe this non-existent check, but those corrections belong to **I-51 in Arc 6**, which owns the whole `actions:` documentation contradiction. Note in the I-27 closure that the prose fix is tracked there, so the two don't collide.

- [ ] **Step 1: Prove it's dead** — paste the grep output showing no production caller into the report. This is the validation for a deletion; there is no behavior to test.
- [ ] **Step 2: Delete**, **Step 3: verify** — `cargo build --workspace` and `cargo test -p rupu-orchestrator -p rupu-agent` clean.
- [ ] **Step 4: Close I-27 and commit**

```bash
git commit -m "refactor(orchestrator,agent): delete the never-called action-protocol validator (I-27)"
```

---

### Task 6: I-23 — document the autoflow selector's safety fields

**Files:**
- Modify: `docs/workflow-format.md` (the autoflow selector table, ~`:179-183`)
- Test: none (documentation)

**Operator decision (settled):** **document only; do not change the default.** Tightening it would silently stop existing autoflows from firing on outside contributors.

Document, in the selector table plus a short prose note:
- `authors` — explicit login allowlist. Default empty = **no restriction**.
- `authors_from` — `collaborators` | `org_members` (verify the exact serialized values against `AuthorScope` at `crates/rupu-orchestrator/src/workflow.rs:429`). Default unset = **no restriction**.
- `draft` — `include` | `exclude` | `only` (verify against `DraftFilter`).
- `base` — base-branch filter.
- `on_skip` — `skip` | `label_needs_human` (verify against `SkipAction`).
- `Autoflow.source`.

The prose note must state plainly: **with neither `authors` nor `authors_from` set, any author who opens a matching issue or PR can trigger the autoflow** — and that autoflows commonly run at `permission_mode: bypass`. Recommend `authors_from: collaborators` for any unattended autoflow. Read `author_allowed` (`workflow.rs:453`) and describe the *actual* precedence between the two fields; do not guess it.

- [ ] **Step 1: Verify every serialized value against the enums**, **Step 2: write the docs**, **Step 3: close I-23 and commit**

```bash
git commit -m "docs: document the autoflow selector's author allowlist and filters (I-23)"
```

---

### Task 7: Arc close-out

- [ ] **Step 1: Full verification**

```bash
cargo test -p rupu-tools -p rupu-orchestrator -p rupu-agent -p rupu-cli 2>&1 | grep "test result"
cargo build --workspace
```
Green modulo the known-red baseline. Compare any failure against a clean checkout.

- [ ] **Step 2: Confirm every Arc 2 issue is closed with evidence** — I-22 … I-27 each in `## Fixed` with a PR number and its validation, or still in triage with a recorded reason. No row silently deleted.

- [ ] **Step 3: Commit and open the Arc 2 PR.**

---

## Deferred out of this arc

- **The `actions:` documentation contradiction** (README ×2, `docs/agent-format.md`, `docs/triggers.md`) — Arc 6, **I-51**. Task 5 deletes the code; Arc 6 fixes the prose.
- **I-73** (`[scm.default].owner`/`.repo` and `[issues.default].project` still inert) — not safety; schedule with Arc 6 or a follow-up config pass.
