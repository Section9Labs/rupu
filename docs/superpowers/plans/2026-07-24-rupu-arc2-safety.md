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

Because the sweep also calls `expire_if_overdue`, and that call is what flips the run terminal, there is a real ordering consequence. **This has now been verified against the code — do not re-derive it, implement it:**

`sweep_decision` (`crates/rupu-cli/src/cmd/cp.rs:337`) branches on `RunStatus::AwaitingApproval` and falls through to `_ => SweepAction::Skip` (`:366`) for every other status. So once the listing expires a reject-timeout gate to `Rejected`, the sweep classifies it `Skip` on every later tick and **the `on_reject` chain never runs at all**. Simply deleting the listing's cleanup call while leaving its expiry call in place would therefore trade a side-effecting read for a silently-dropped cleanup — a worse bug.

The required shape, per `on_timeout`:

| `on_timeout` | Listing behavior | Why |
|---|---|---|
| `reject` | **Do not call `expire_if_overdue` at all.** Leave the run `AwaitingApproval`. | Preserves the `AwaitingApproval` status the sweep's `ExpireThenCleanupReject` arm requires. The sweep expires it *and* runs the chain. |
| `approve` | Call it; keep the existing informational `println!`. | `expire_if_overdue` deliberately leaves the record `AwaitingApproval` on this arm (see `cp.rs:311-313`), so the sweep still sees it. No side effect today beyond the print. |
| `fail` / unset | Call it. | Finalized inside the expire call itself; there is no chain to run. |

So the listing keeps lazy expiry for the two harmless cases, stops executing chains entirely, and hands the `reject` case to the sweep untouched. Note in the code comment that a `reject` gate is now resolved only by `rupu cp serve`'s sweep — and that if the sweep is disabled (`[cp].gate_sweep_enabled = false`) such a gate stays parked until an operator runs `rupu workflow reject`. State in your report what you found and what you did.

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

### Task 4: I-26 — make `action_dispatcher_for`'s doc comment true

> **Scope corrected 2026-07-27 after verification against the code. The original task ("narrow an action step by its own `actions:`") rested on a false premise and must NOT be implemented as written.** What follows is the verified replacement.

**Files:**
- Modify: `crates/rupu-cli/src/resume.rs` (the doc comment on `action_dispatcher_for`, `:19-26`)
- Test: none required — the invariant this documents is already enforced and already covered (see below)

**What was verified:**

1. **A step cannot carry both `action:` and a non-empty `actions:`.** `validate_step_actions` (`crates/rupu-orchestrator/src/workflow.rs:1389`) returns `WorkflowParseError::ActionsOnActionStep` for exactly that combination, with the message *"an `action:` step must not carry a non-empty `actions:` allowlist — its tool is already explicit"*. A parse test at `workflow.rs:2887` already asserts it. **The original Task 4 test workflow (`action: scm.prs.create` + `actions: ["issues.comment"]`) would fail to parse rather than produce a permission denial** — there is nothing to narrow, so the "narrowing" fix is not merely hard, it is meaningless.

2. **The `["*"]` allowlist is not reachable by an agent step.** `opts.action_dispatcher` has exactly three production consumers — `runner.rs:4293` (the action-step arm), `:4700` (`fire_notify_hooks`) and `:4923` (the second action arm) — and all three funnel into `execute_action_step`, whose only dispatch is `dispatcher.call(tool, args)` where `tool = step.action`. Agent-step tool calls go through the `rupu-tools` registry and the `DefaultStepFactory`'s own narrowing (`step_factory.rs:246,407`), never through this dispatcher.

3. Every tool named in an `action:` is validated against the live MCP catalog at parse time by `validate_action_step` (`workflow.rs:1324`, called at `:1587`, and at `:1536` for notify hooks).

Together those three make `["*"]` **sound**: the only tool this dispatcher can ever be asked for is one that is explicit in the workflow source and catalog-validated. The permission mode passed alongside it is still live and still enforced (a `readonly` run refuses Write-classified tools).

So the single real defect is the doc comment's claim that an `action:` step *"sees exactly the same allow/deny surface a `tools:`-using agent step would"*. That is false — an agent step's surface is its `tools:` list narrowed by `actions:`; this one is `["*"]` constrained by the workflow source instead.

- [ ] **Step 1: Rewrite the doc comment** so it states the actual invariant: the allowlist is deliberately unconstrained because the dispatcher is only ever called with an explicit, parse-validated `step.action`; a non-empty `actions:` on an action step is a parse error (`ActionsOnActionStep`); and the **mode** half genuinely does match the agent path. Do not claim parity with the agent allowlist. Reference `ActionsOnActionStep` by name so the next reader can find the enforcement.

- [ ] **Step 2: Verify** — `cargo build --workspace`, `cargo test -p rupu-orchestrator -p rupu-cli`. No behavior changes, so no new test; cite `workflow.rs:2887` as the existing coverage of the invariant.

- [ ] **Step 3: File the defense-in-depth follow-up.** Narrowing the allowlist to `vec![step.action]` would remove reliance on that invariant, but the dispatcher is built once per run (`resume.rs:27`) while the tool is per-step, and `execute_action_step` receives `&ToolDispatcher` rather than the registry — so it needs a signature change across three call sites. Sound today, worth hardening later. File as a new issue in `ISSUES.md` (next free ID) marked P2, and reference it from the I-26 closure.

- [ ] **Step 4: Close I-26 and commit.** The closure must record that the narrowing half was withdrawn as based on a false premise, and name `ActionsOnActionStep` as the reason.

```bash
git commit -m "docs(cli): correct action_dispatcher_for's allow-surface comment (I-26)"
```

---

### Task 5: I-27 — delete the dead action-protocol validator

> **Already satisfied — verified 2026-07-27. No code change required; this task is a tracker closure only.**

Commit `28ec5cc3` ("chore: delete the dead legacy action protocol (open_pr/file_write vocabulary)"), landed during Arc 1 and confirmed an ancestor of this branch, already removed exactly the five files this task named:

```
crates/rupu-agent/src/action.rs                    | 21 ---
crates/rupu-agent/src/lib.rs                       |  3 --
crates/rupu-orchestrator/src/action_protocol.rs    | 33 ---
crates/rupu-orchestrator/src/lib.rs                | 13 +--
crates/rupu-orchestrator/tests/action_allowlist.rs | 35 ---
```

`grep -rn "validate_actions\|ActionValidator" --include="*.rs" crates` now returns **zero hits**. I-27 was fixed without being closed in the tracker.

Note for the reader: `validate_action_step` (`crates/rupu-orchestrator/src/workflow.rs:1324`) survives and is **live** — it is the catalog validator for `action:` steps and notify hooks, unrelated to the deleted `validate_actions`. Do not delete it.

- [ ] **Step 1: Close I-27** in `ISSUES.md` citing `28ec5cc3` as the fix and the empty grep as the validation. Note in the closure that the prose corrections (README ×2, `docs/agent-format.md`, `docs/triggers.md` all still describe the deleted check) are owned by **I-51 in Arc 6**, so the two do not collide.

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
- **Reject-timeout gates depend on `cp serve` after Task 3** — file as a new issue during close-out (next free ID) and mirror into `ISSUES.md`. Task 3 makes the sweep the *only* resolver of an `on_timeout: reject` gate. That is correct for the side-effect-free-read goal, but it means a CLI-only operator who never starts `rupu cp serve` (or who sets `[cp].gate_sweep_enabled = false`) now has such gates park indefinitely, where lazy expiry previously resolved them on the next `rupu workflow runs`. Not a blocker and not a regression in *safety*, but it is a real behavior change for a whole class of user. Candidate remedies to evaluate later, not in this arc: run the chain from `rupu workflow reject` only (already true) and document the dependency, or add an explicit opt-in `rupu workflow runs --resolve-expired`.
