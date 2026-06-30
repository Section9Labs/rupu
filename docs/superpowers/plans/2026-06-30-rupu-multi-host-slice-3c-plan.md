# Multi-host Slice 3c — Cross-Host Workspace Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a workflow step opt into having the coordinator's file workspace synced to the remote host it runs on (git-bundle round-trip when the workspace is a git repo, tar otherwise), and propagate the resulting file changes back so downstream steps see them.

**Architecture:** The orchestrator stays hexagonal — it gains an opt-in `workspace` mode on steps/defaults, passes the coordinator `workspace_path` through the existing `UnitDispatcher` port as opaque material, receives an opaque `WorkspaceDelta` back, and calls a new mode-aware `apply_workspace_deltas` port method that performs all merge/conflict logic. The git/tar codec lives in `rupu-workspace`; the per-transport staging lives on `rupu-cp`'s `HostConnector`; `rupu-cli`'s `FleetUnitDispatcher` composes them and bridges the orchestrator's opaque types to the codec.

**Tech Stack:** Rust 2021 (MSRV 1.88), tokio, serde/serde_yaml, thiserror (libs) / anyhow (CLI), async-trait, `git2` (vendored, already a dep), `tar` + `ignore` (new), tracing.

## Global Constraints

- Backward compatible: no `workspace:` anywhere ⇒ byte-for-byte 3a/3b behavior; when the effective mode is not `Sync`, `UnitDispatch.workspace_path` is `None` and the dispatch path is unchanged.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code = "forbid"`.
- Libraries use `thiserror`; the CLI binary uses `anyhow`.
- **Workspace dependencies only**: pin `tar` and `ignore` in root `Cargo.toml` `[workspace.dependencies]`; reference with `{ workspace = true }` in crate `Cargo.toml`. `git2` is already pinned (vendored libgit2/openssl — no host git needed).
- Hexagonal: `rupu-orchestrator` depends on **neither** `rupu-workspace` nor `rupu-cp`; it knows only the `UnitDispatcher` trait + the opaque `WorkspaceDelta` / `WorkspaceConflict` types it defines itself. All git/tar/merge logic lives in `rupu-workspace` (codec) and `rupu-cli`/`rupu-cp` (transport + bridge).
- No silent self-contained fallback: a `workspace: sync` step on an unsupported transport (Bucket/Tunnel in v1) fails fast with a clear error.
- gitignore respected in both directions (git: native; tar: the `ignore` crate). Scratch dirs cleaned up; transferred payload size-guarded. No new secrets.
- Per-file `rustfmt` only (`rustfmt <path>`); `main` is fmt-dirty under the worktree toolchain — never a package-wide `cargo fmt`.
- Clippy: the worktree's Homebrew clippy **1.95** denies a pre-existing `items_after_test_module` in `crates/rupu-orchestrator/src/runner.rs` (present identically on `main`; pinned CI **1.88** is clean). Scope clippy to changed crates with `--no-deps`; do **not** fix that pre-existing error. `rupu-cli` also has pre-existing unrelated 1.95 clippy errors and `cmd::session::tests` failures — scope test runs to the changed modules.
- `build_dispatcher_if_needed` already fires for `distribute || host` — **no trigger change** (workspace sync only ever rides an already-placed/distributed step).

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `crates/rupu-orchestrator/src/workflow.rs` | `WorkspaceMode` enum; `defaults.workspace` + `Step.workspace`; effective-mode resolution; validation | 1 |
| `crates/rupu-orchestrator/src/runner.rs` | port surface (`UnitDispatch.workspace_path`, `UnitOutcome.workspace_delta`, `WorkspaceDelta`, `WorkspaceConflict`, `apply_workspace_deltas`); routing in `run_linear_step`/`run_fanout_step` | 2, 5 |
| `Cargo.toml` (root) | pin `tar`, `ignore` | 3 |
| `crates/rupu-workspace/src/workspace_sync.rs` (new) | git+tar codec: `pack` / `collect_delta` / `apply_deltas` + mode detect | 3, 4 |
| `crates/rupu-workspace/src/lib.rs` | export the codec | 3 |
| `crates/rupu-cp/src/host/connector.rs` | `stage_workspace` / `collect_workspace_delta` on `HostConnector`; `Unsupported` error | 6 |
| `crates/rupu-cp/src/host/{local,ssh,http}.rs` | per-transport staging impls; Bucket/Tunnel `Unsupported` | 6 |
| `crates/rupu-cli/src/fleet_unit_dispatcher.rs` | compose stage→launch→collect; implement `apply_workspace_deltas` (bridge) | 6 |
| `crates/rupu-orchestrator/tests/workspace_sync_e2e.rs` (new) | e2e: git placed-linear, tar variant, fan-out disjoint | 7 |

**Increment boundaries:** T1–T2 add surface (no behavior). T3 (tar) and T4 (git) are independently shippable codec increments. T5 wires routing (testable with fakes). T6 makes real transports work. T7 proves end-to-end.

---

## Task 1: Workflow model — `workspace` mode + validation

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (`WorkflowDefaults` ~line 518; `Step` struct ~line 533; `WorkflowParseError` ~line 116; `validate_step_shape` ~line 818)
- Test: same file

**Interfaces:**
- Produces: `pub enum WorkspaceMode { Sync, None }` (serde `rename_all = "lowercase"`); `WorkflowDefaults.workspace: Option<WorkspaceMode>`; `Step.workspace: Option<WorkspaceMode>`; `pub fn effective_workspace_mode(step: &Step, defaults: &WorkflowDefaults) -> WorkspaceMode`; `WorkflowParseError::WorkspaceSyncOnLocalStep { step: String }`.

- [ ] **Step 1: Write the failing tests**

Append a `mod workspace_mode_tests` at the bottom of `crates/rupu-orchestrator/src/workflow.rs`:

```rust
#[cfg(test)]
mod workspace_mode_tests {
    use super::*;

    #[test]
    fn workspace_parses_on_placed_step() {
        let wf = Workflow::parse(
            r#"
name: ws
steps:
  - id: build
    agent: a
    prompt: p
    host: worker-1
    workspace: sync
"#,
        )
        .unwrap();
        assert_eq!(wf.steps[0].workspace, Some(WorkspaceMode::Sync));
    }

    #[test]
    fn workspace_default_and_override_resolve() {
        let wf = Workflow::parse(
            r#"
name: ws
defaults:
  workspace: sync
steps:
  - id: a
    agent: a
    prompt: p
    host: w1
  - id: b
    agent: a
    prompt: p
    host: w2
    workspace: none
"#,
        )
        .unwrap();
        // step a inherits the default; step b overrides to none.
        assert_eq!(
            effective_workspace_mode(&wf.steps[0], &wf.defaults),
            WorkspaceMode::Sync
        );
        assert_eq!(
            effective_workspace_mode(&wf.steps[1], &wf.defaults),
            WorkspaceMode::None
        );
    }

    #[test]
    fn workspace_absent_resolves_to_none() {
        let wf = Workflow::parse(
            r#"
name: ws
steps:
  - id: a
    agent: a
    prompt: p
    host: w1
"#,
        )
        .unwrap();
        assert_eq!(
            effective_workspace_mode(&wf.steps[0], &wf.defaults),
            WorkspaceMode::None
        );
        assert_eq!(wf.steps[0].workspace, None);
    }

    #[test]
    fn workspace_none_skipped_in_serialize() {
        let wf = Workflow::parse(
            r#"
name: ws
steps:
  - id: a
    agent: a
    prompt: p
"#,
        )
        .unwrap();
        let out = serde_yaml::to_string(&wf).unwrap();
        assert!(!out.contains("workspace"), "None must be skipped: {out}");
    }

    #[test]
    fn workspace_sync_rejected_on_local_step() {
        let err = Workflow::parse(
            r#"
name: ws
steps:
  - id: a
    agent: a
    prompt: p
    workspace: sync
"#,
        )
        .expect_err("sync on a local step is invalid");
        assert!(matches!(
            err,
            WorkflowParseError::WorkspaceSyncOnLocalStep { .. }
        ));
    }

    #[test]
    fn workspace_sync_allowed_on_distribute_step() {
        // distribute => remote => sync is valid.
        Workflow::parse(
            r#"
name: ws
steps:
  - id: a
    for_each: "x\ny"
    agent: a
    prompt: p
    workspace: sync
    distribute:
      hosts: [w1, w2]
"#,
        )
        .expect("sync on a distribute step is valid");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-orchestrator workspace_mode_tests`
Expected: FAIL — `cannot find type WorkspaceMode` / no field `workspace`.

- [ ] **Step 3: Add the `WorkspaceMode` enum**

In `crates/rupu-orchestrator/src/workflow.rs`, near the other small enums (e.g. above `Distribute` ~line 527), add:

```rust
/// Whether a step's file workspace is synced to the remote host it runs on.
/// `None` (the default) keeps the self-contained behavior of Slices 3a/3b:
/// the remote step sees only its rendered prompt + prior-step string outputs.
/// `Sync` makes the coordinator's workspace available on the host and brings
/// the file changes back (Slice 3c). Only meaningful on a remote step
/// (`host:` or `distribute:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMode {
    Sync,
    None,
}
```

- [ ] **Step 4: Add the fields**

In `WorkflowDefaults` (~line 518), after `continue_on_error`:

```rust
    /// Workflow-wide default workspace mode for remote steps. A step's
    /// `workspace:` overrides this. Absent ⇒ `None` (self-contained).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceMode>,
```

In `Step` (~line 533), after the `host` field (added in 3b):

```rust
    /// Per-step workspace mode override. When the effective mode (this, else
    /// `defaults.workspace`, else `None`) is `Sync`, the coordinator's
    /// workspace is synced to the remote host this step runs on. Valid only
    /// on a remote step (`host:` or `distribute:`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceMode>,
```

- [ ] **Step 5: Add the resolution helper**

Near `validate_step_shape` in the same file, add a public free function:

```rust
/// Resolve a step's effective workspace mode: the step's own `workspace`,
/// else the workflow `defaults.workspace`, else `WorkspaceMode::None`.
pub fn effective_workspace_mode(step: &Step, defaults: &WorkflowDefaults) -> WorkspaceMode {
    step.workspace
        .or(defaults.workspace)
        .unwrap_or(WorkspaceMode::None)
}
```

- [ ] **Step 6: Add the error variant + validation**

In `WorkflowParseError` (after the 3b `HostEmpty` variant ~line 120):

```rust
    #[error("step `{step}`: `workspace: sync` is only valid on a remote step (`host:` or `distribute:`)")]
    WorkspaceSyncOnLocalStep { step: String },
```

In `validate_step_shape`, after the `host` validation block (added in 3b, ~line 925), add:

```rust
    // `workspace: sync` is only meaningful on a remote step — one with `host:`
    // (3b) or `distribute:` (3a). On a purely-local step it would be a no-op
    // and signals author confusion, so reject it.
    if step.workspace == Some(WorkspaceMode::Sync)
        && step.host.is_none()
        && step.distribute.is_none()
    {
        return Err(WorkflowParseError::WorkspaceSyncOnLocalStep {
            step: step.id.clone(),
        });
    }
```

- [ ] **Step 7: Run tests, format, lint**

Run:
```bash
cargo test -p rupu-orchestrator workspace_mode_tests
rustfmt crates/rupu-orchestrator/src/workflow.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
cargo test -p rupu-orchestrator
```
Expected: 6 new tests pass; clippy clean for the change; full suite green (additive serde-default field).

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-orchestrator/src/workflow.rs
git commit -m "feat(multi-host): workspace mode on step/defaults + validation (3c T1)"
```

---

## Task 2: Orchestrator port surface (opaque workspace types)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`UnitDispatch` ~line 43, `UnitOutcome` ~line 53, `UnitDispatcher` trait ~line 67; construction sites in `run_fanout_step` and `dispatch_placed_step`; test fakes ~line 2531, ~2755)
- Modify: `crates/rupu-orchestrator/tests/distributed_fanout_e2e.rs` (`RecordingDispatcher` ~line 147), `crates/rupu-orchestrator/tests/placed_step_e2e.rs` (its `RecordingDispatcher`)
- Modify: `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (real `dispatch_unit` `UnitOutcome` construction + test fakes' `UnitOutcome`)
- Test: `crates/rupu-orchestrator/src/runner.rs`

**Interfaces:**
- Produces:
  - `UnitDispatch.workspace_path: Option<PathBuf>` (new field).
  - `UnitOutcome.workspace_delta: Option<WorkspaceDelta>` (new field).
  - `pub struct WorkspaceDelta { pub changed: Vec<String>, pub deleted: Vec<String>, pub payload: Vec<u8> }` (opaque carrier; `payload` is self-describing git/tar bytes the orchestrator never interprets; `changed`/`deleted` for observability).
  - `#[derive(Debug, Error)] #[error("workspace conflict on: {0:?}")] pub struct WorkspaceConflict(pub Vec<String>);`
  - `UnitDispatcher::apply_workspace_deltas(&self, workspace_path: &Path, deltas: &[WorkspaceDelta]) -> Result<(), WorkspaceConflict>` — a **defaulted** trait method (default returns `Ok(())`) so existing fakes need no impl; the real one comes in T6.
- Consumes: `RunError`, `async_trait` (already imported).

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod` at the bottom of `runner.rs` (where `FakeUnitDispatcher` lives):

```rust
    #[test]
    fn workspace_delta_carries_paths_and_payload() {
        let d = WorkspaceDelta {
            changed: vec!["src/lib.rs".into()],
            deleted: vec!["old.txt".into()],
            payload: vec![1, 2, 3],
        };
        assert_eq!(d.changed, vec!["src/lib.rs".to_string()]);
        assert_eq!(d.deleted, vec!["old.txt".to_string()]);
        assert_eq!(d.payload, vec![1, 2, 3]);
    }

    #[test]
    fn workspace_conflict_displays_paths() {
        let c = WorkspaceConflict(vec!["src/shared.rs".into()]);
        assert!(c.to_string().contains("src/shared.rs"));
    }

    #[tokio::test]
    async fn default_apply_workspace_deltas_is_noop_ok() {
        // The 3a FakeUnitDispatcher does not override apply; the default is Ok.
        let d = FakeUnitDispatcher::new();
        let tmp = tempfile::tempdir().unwrap();
        let res = d.apply_workspace_deltas(tmp.path(), &[]).await;
        assert!(res.is_ok());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-orchestrator -- workspace_delta_carries workspace_conflict_displays default_apply_workspace`
Expected: FAIL — `WorkspaceDelta` / `WorkspaceConflict` / `apply_workspace_deltas` not found.

- [ ] **Step 3: Define the opaque types**

In `runner.rs`, just above the `UnitDispatch` struct (~line 41), add:

```rust
/// Opaque file-change set a synced unit returns. The orchestrator never
/// interprets `payload` — a self-describing git patch/bundle or tar delta
/// produced by the workspace codec. `changed` / `deleted` are the affected
/// repo-relative paths, carried for observability/logging only.
#[derive(Debug, Clone)]
pub struct WorkspaceDelta {
    pub changed: Vec<String>,
    pub deleted: Vec<String>,
    pub payload: Vec<u8>,
}

/// Returned by `apply_workspace_deltas` when two units' changes conflict —
/// overlapping files (tar mode) or a conflicting hunk (git mode). Surfaced
/// as a step failure honoring `continue_on_error`.
#[derive(Debug, Error)]
#[error("workspace conflict on: {0:?}")]
pub struct WorkspaceConflict(pub Vec<String>);
```

- [ ] **Step 4: Extend `UnitDispatch` and `UnitOutcome`**

`UnitDispatch` — add (and ensure `use std::path::PathBuf;`/`Path` are imported at the top of runner.rs):
```rust
    /// Set to `Some(coordinator workspace path)` when this unit's effective
    /// workspace mode is `Sync`. `None` ⇒ self-contained (unchanged).
    pub workspace_path: Option<std::path::PathBuf>,
```

`UnitOutcome` — add:
```rust
    /// The unit's file changes when it ran with a synced workspace; `None`
    /// for a self-contained unit.
    pub workspace_delta: Option<WorkspaceDelta>,
```

- [ ] **Step 5: Add the defaulted trait method**

In the `UnitDispatcher` trait body, after `dispatch_unit`:
```rust
    /// Apply collected unit workspace deltas to the coordinator workspace at
    /// `workspace_path`. Mode-aware (git 3-way merge / tar disjoint-copy);
    /// conflicts return `WorkspaceConflict`. Default is a no-op for
    /// dispatchers without workspace support.
    async fn apply_workspace_deltas(
        &self,
        _workspace_path: &std::path::Path,
        _deltas: &[WorkspaceDelta],
    ) -> Result<(), WorkspaceConflict> {
        Ok(())
    }
```

- [ ] **Step 6: Update every `UnitDispatch` / `UnitOutcome` construction site**

A struct literal requires all fields. Add the new field to each:

`UnitDispatch` literals in `runner.rs`:
- `run_fanout_step` remote dispatch: the `UnitDispatch { .. }` at ~line 1249 and the retry one at ~line 1280 → add `workspace_path: None,` (T5 will set it for sync).
- `dispatch_placed_step` (3b, ~line 966): its `UnitDispatch { .. }` → add `workspace_path: None,`.

`UnitOutcome` literals:
- `runner.rs` test `FakeUnitDispatcher` (~line 2566) and `AlwaysFailedOutcomeDispatcher` (~line 2755) → add `workspace_delta: None,`.
- `crates/rupu-orchestrator/tests/distributed_fanout_e2e.rs` `RecordingDispatcher::dispatch_unit` → add `workspace_delta: None,`.
- `crates/rupu-orchestrator/tests/placed_step_e2e.rs` `RecordingDispatcher::dispatch_unit` → add `workspace_delta: None,`.
- `crates/rupu-cli/src/fleet_unit_dispatcher.rs`: the real `dispatch_unit` `UnitOutcome` construction(s) → add `workspace_delta: None,` (real population in T6); any test-fake `UnitOutcome` likewise.

- [ ] **Step 7: Run the new tests + full suites for touched crates**

Run:
```bash
cargo test -p rupu-orchestrator -- workspace_delta_carries workspace_conflict_displays default_apply_workspace
cargo test -p rupu-orchestrator
cargo test -p rupu-cli --lib fleet_unit_dispatcher
```
Expected: 3 new tests pass; both crates compile and their suites green (additive fields + defaulted method).

- [ ] **Step 8: Format, lint, commit**

```bash
rustfmt crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/tests/distributed_fanout_e2e.rs crates/rupu-orchestrator/tests/placed_step_e2e.rs crates/rupu-cli/src/fleet_unit_dispatcher.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
git add crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/tests crates/rupu-cli/src/fleet_unit_dispatcher.rs
git commit -m "feat(multi-host): UnitDispatcher workspace port surface (3c T2)"
```

---

## Task 3: `rupu-workspace` tar codec + mode detection

**Files:**
- Modify: `Cargo.toml` (root) — pin `tar`, `ignore`
- Modify: `crates/rupu-workspace/Cargo.toml` — depend on `tar`, `ignore`, `sha2` (already pinned), `thiserror`
- Create: `crates/rupu-workspace/src/workspace_sync.rs`
- Modify: `crates/rupu-workspace/src/lib.rs` — `pub mod workspace_sync;` + re-export
- Test: `crates/rupu-workspace/src/workspace_sync.rs` (unit tests)

**Interfaces:**
- Produces (rupu-workspace's OWN representation, distinct from the orchestrator's opaque type):
  - `pub enum SyncMode { Git, Tar }`
  - `pub fn detect_mode(workspace_path: &Path) -> SyncMode` — `Git` if a git repo is found at/above `workspace_path`, else `Tar`.
  - `pub struct Payload { pub mode: SyncMode, pub bytes: Vec<u8> }` — staged baseline (self-describing).
  - `pub struct Delta { pub mode: SyncMode, pub changed: Vec<String>, pub deleted: Vec<String>, pub bytes: Vec<u8> }`
  - `#[derive(Debug, Error)] pub enum SyncError { Io(...), Conflict(Vec<String>), Git(...) }`
  - `pub fn pack(workspace_path: &Path) -> Result<Payload, SyncError>` (this task: the `Tar` arm; `Git` arm added in T4)
  - `pub fn stage(payload: &Payload, scratch_dir: &Path) -> Result<Baseline, SyncError>` — extract into `scratch_dir`, return a `Baseline` (tar: path→sha256 manifest).
  - `pub fn collect_delta(scratch_dir: &Path, baseline: &Baseline) -> Result<Delta, SyncError>`
  - `pub fn apply_deltas(workspace_path: &Path, deltas: &[Delta]) -> Result<(), SyncError>` (tar: union changed paths → `Conflict` on overlap, else write/remove)
  - `pub struct Baseline { /* tar: BTreeMap<String,[u8;32]>; git: the staged commit oid */ }`

- [ ] **Step 1: Pin the new deps**

In root `Cargo.toml` `[workspace.dependencies]`, after the `object_store` line:
```toml
# Workspace sync (multi-host Slice 3c) — tar fallback + gitignore-aware walk
tar = "0.4"
ignore = "0.4"
```
In `crates/rupu-workspace/Cargo.toml` `[dependencies]`, add:
```toml
tar = { workspace = true }
ignore = { workspace = true }
sha2 = { workspace = true }
thiserror = { workspace = true }
git2 = { workspace = true }
```
(Confirm which are already present; add only the missing ones.)

- [ ] **Step 2: Write the failing tests**

Create `crates/rupu-workspace/src/workspace_sync.rs` with tests first:

```rust
//! Cross-host workspace sync codec (multi-host Slice 3c). Git mode for git
//! repos (T4), tar mode for everything else. Pack on the coordinator, stage +
//! collect_delta on the remote, apply_deltas back on the coordinator.

// (implementation added in later steps)

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &std::path::Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn tar_mode_detected_for_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "hello");
        assert_eq!(detect_mode(dir.path()), SyncMode::Tar);
    }

    #[test]
    fn tar_round_trip_create_modify_delete() {
        // coordinator workspace
        let ws = tempfile::tempdir().unwrap();
        write(ws.path(), "keep.txt", "keep");
        write(ws.path(), "mod.txt", "before");
        write(ws.path(), "gone.txt", "remove me");

        // pack + stage to a remote scratch
        let payload = pack(ws.path()).unwrap();
        assert_eq!(payload.mode, SyncMode::Tar);
        let scratch = tempfile::tempdir().unwrap();
        let baseline = stage(&payload, scratch.path()).unwrap();

        // "remote agent" mutates the scratch tree
        write(scratch.path(), "mod.txt", "after");
        write(scratch.path(), "new.txt", "created");
        fs::remove_file(scratch.path().join("gone.txt")).unwrap();

        // collect the delta
        let delta = collect_delta(scratch.path(), &baseline).unwrap();
        assert!(delta.changed.contains(&"mod.txt".to_string()));
        assert!(delta.changed.contains(&"new.txt".to_string()));
        assert!(delta.deleted.contains(&"gone.txt".to_string()));
        assert!(!delta.changed.contains(&"keep.txt".to_string()));

        // apply back to the coordinator workspace
        apply_deltas(ws.path(), &[delta]).unwrap();
        assert_eq!(fs::read_to_string(ws.path().join("mod.txt")).unwrap(), "after");
        assert_eq!(fs::read_to_string(ws.path().join("new.txt")).unwrap(), "created");
        assert!(!ws.path().join("gone.txt").exists());
        assert_eq!(fs::read_to_string(ws.path().join("keep.txt")).unwrap(), "keep");
    }

    #[test]
    fn tar_pack_respects_gitignore() {
        let ws = tempfile::tempdir().unwrap();
        write(ws.path(), ".gitignore", "target/\n*.log\n");
        write(ws.path(), "src.rs", "code");
        write(ws.path(), "target/junk.o", "binary");
        write(ws.path(), "run.log", "noise");

        let payload = pack(ws.path()).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        stage(&payload, scratch.path()).unwrap();
        assert!(scratch.path().join("src.rs").exists());
        assert!(!scratch.path().join("target/junk.o").exists());
        assert!(!scratch.path().join("run.log").exists());
    }

    #[test]
    fn tar_apply_disjoint_deltas_merges() {
        let ws = tempfile::tempdir().unwrap();
        write(ws.path(), "base", "x");
        let d1 = Delta { mode: SyncMode::Tar, changed: vec!["a.txt".into()], deleted: vec![], bytes: tar_one("a.txt", "AAA") };
        let d2 = Delta { mode: SyncMode::Tar, changed: vec!["b.txt".into()], deleted: vec![], bytes: tar_one("b.txt", "BBB") };
        apply_deltas(ws.path(), &[d1, d2]).unwrap();
        assert_eq!(fs::read_to_string(ws.path().join("a.txt")).unwrap(), "AAA");
        assert_eq!(fs::read_to_string(ws.path().join("b.txt")).unwrap(), "BBB");
    }

    #[test]
    fn tar_apply_overlapping_deltas_conflicts() {
        let ws = tempfile::tempdir().unwrap();
        let d1 = Delta { mode: SyncMode::Tar, changed: vec!["shared.txt".into()], deleted: vec![], bytes: tar_one("shared.txt", "A") };
        let d2 = Delta { mode: SyncMode::Tar, changed: vec!["shared.txt".into()], deleted: vec![], bytes: tar_one("shared.txt", "B") };
        let err = apply_deltas(ws.path(), &[d1, d2]).unwrap_err();
        match err {
            SyncError::Conflict(paths) => assert!(paths.contains(&"shared.txt".to_string())),
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    /// Helper: build a one-file tar payload (matches the delta bytes format).
    fn tar_one(path: &str, body: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            b.append_data(&mut header, path, body.as_bytes()).unwrap();
            b.finish().unwrap();
        }
        buf
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-workspace workspace_sync`
Expected: FAIL — module items not defined.

- [ ] **Step 4: Implement the tar codec + detection**

In `crates/rupu-workspace/src/workspace_sync.rs` (above the test module) implement:

```rust
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    Git,
    Tar,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("workspace sync io: {0}")]
    Io(#[from] std::io::Error),
    #[error("workspace conflict on: {0:?}")]
    Conflict(Vec<String>),
    #[error("workspace sync git: {0}")]
    Git(String),
}

#[derive(Debug, Clone)]
pub struct Payload {
    pub mode: SyncMode,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Delta {
    pub mode: SyncMode,
    pub changed: Vec<String>,
    pub deleted: Vec<String>,
    pub bytes: Vec<u8>,
}

/// Baseline captured at stage time so `collect_delta` can diff against it.
/// Tar: a path→sha256 manifest. Git: the staged commit oid (see T4).
#[derive(Debug, Clone)]
pub struct Baseline {
    pub mode: SyncMode,
    pub tar_manifest: BTreeMap<String, [u8; 32]>,
    pub git_commit: Option<String>,
}

/// `Git` when a git repo is found at/above `workspace_path`, else `Tar`.
pub fn detect_mode(workspace_path: &Path) -> SyncMode {
    match git2::Repository::discover(workspace_path) {
        Ok(_) => SyncMode::Git,
        Err(_) => SyncMode::Tar,
    }
}

pub fn pack(workspace_path: &Path) -> Result<Payload, SyncError> {
    match detect_mode(workspace_path) {
        SyncMode::Git => pack_git(workspace_path), // implemented in T4
        SyncMode::Tar => pack_tar(workspace_path),
    }
}

pub fn stage(payload: &Payload, scratch_dir: &Path) -> Result<Baseline, SyncError> {
    match payload.mode {
        SyncMode::Git => stage_git(payload, scratch_dir), // T4
        SyncMode::Tar => stage_tar(payload, scratch_dir),
    }
}

pub fn collect_delta(scratch_dir: &Path, baseline: &Baseline) -> Result<Delta, SyncError> {
    match baseline.mode {
        SyncMode::Git => collect_delta_git(scratch_dir, baseline), // T4
        SyncMode::Tar => collect_delta_tar(scratch_dir, baseline),
    }
}

pub fn apply_deltas(workspace_path: &Path, deltas: &[Delta]) -> Result<(), SyncError> {
    if deltas.is_empty() {
        return Ok(());
    }
    match deltas[0].mode {
        SyncMode::Git => apply_deltas_git(workspace_path, deltas), // T4
        SyncMode::Tar => apply_deltas_tar(workspace_path, deltas),
    }
}

// ── tar mode ────────────────────────────────────────────────────────────────

fn pack_tar(workspace_path: &Path) -> Result<Payload, SyncError> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        // `ignore` walks the tree honoring .gitignore + global ignores.
        for entry in ignore::WalkBuilder::new(workspace_path).hidden(false).build() {
            let entry = entry.map_err(|e| SyncError::Io(std::io::Error::other(e)))?;
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let abs = entry.path();
            let rel = abs
                .strip_prefix(workspace_path)
                .map_err(|e| SyncError::Io(std::io::Error::other(e)))?;
            builder.append_path_with_name(abs, rel)?;
        }
        builder.finish()?;
    }
    Ok(Payload { mode: SyncMode::Tar, bytes: buf })
}

fn stage_tar(payload: &Payload, scratch_dir: &Path) -> Result<Baseline, SyncError> {
    fs::create_dir_all(scratch_dir)?;
    let mut ar = tar::Archive::new(payload.bytes.as_slice());
    ar.unpack(scratch_dir)?;
    Ok(Baseline {
        mode: SyncMode::Tar,
        tar_manifest: hash_tree(scratch_dir)?,
        git_commit: None,
    })
}

fn collect_delta_tar(scratch_dir: &Path, baseline: &Baseline) -> Result<Delta, SyncError> {
    let after = hash_tree(scratch_dir)?;
    let mut changed = Vec::new();
    let mut deleted = Vec::new();
    for (path, hash) in &after {
        match baseline.tar_manifest.get(path) {
            Some(old) if old == hash => {}
            _ => changed.push(path.clone()),
        }
    }
    for path in baseline.tar_manifest.keys() {
        if !after.contains_key(path) {
            deleted.push(path.clone());
        }
    }
    changed.sort();
    deleted.sort();
    // Pack only the changed files into a tar.
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        for rel in &changed {
            builder.append_path_with_name(scratch_dir.join(rel), rel)?;
        }
        builder.finish()?;
    }
    Ok(Delta { mode: SyncMode::Tar, changed, deleted, bytes: buf })
}

fn apply_deltas_tar(workspace_path: &Path, deltas: &[Delta]) -> Result<(), SyncError> {
    // Conflict = the same path changed/deleted by more than one delta.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut conflicts = Vec::new();
    for d in deltas {
        for p in d.changed.iter().chain(d.deleted.iter()) {
            let n = seen.entry(p.clone()).or_insert(0);
            *n += 1;
            if *n == 2 {
                conflicts.push(p.clone());
            }
        }
    }
    if !conflicts.is_empty() {
        conflicts.sort();
        conflicts.dedup();
        return Err(SyncError::Conflict(conflicts));
    }
    // No overlap — apply each delta: extract changed files, remove deleted.
    for d in deltas {
        let mut ar = tar::Archive::new(d.bytes.as_slice());
        ar.unpack(workspace_path)?;
        for rel in &d.deleted {
            let p = workspace_path.join(rel);
            if p.exists() {
                fs::remove_file(p)?;
            }
        }
    }
    Ok(())
}

/// Map of repo-relative path → sha256 of file contents, for the whole tree
/// under `root` (used as the tar baseline manifest).
fn hash_tree(root: &Path) -> Result<BTreeMap<String, [u8; 32]>, SyncError> {
    let mut map = BTreeMap::new();
    for entry in walkdir_files(root)? {
        let rel = entry
            .strip_prefix(root)
            .map_err(|e| SyncError::Io(std::io::Error::other(e)))?
            .to_string_lossy()
            .replace('\\', "/");
        let mut hasher = Sha256::new();
        hasher.update(fs::read(&entry)?);
        map.insert(rel, hasher.finalize().into());
    }
    Ok(map)
}

/// Recursively list regular files under `root` (no ignore filtering — the
/// scratch tree was already filtered at pack time).
fn walkdir_files(root: &Path) -> Result<Vec<PathBuf>, SyncError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
            }
        }
    }
    Ok(out)
}
```

Add stubs for the git arms so the crate compiles before T4 (they `unimplemented!`-free — return a clear error so an accidental git-mode call in this task fails loudly rather than silently):

```rust
fn pack_git(_p: &Path) -> Result<Payload, SyncError> {
    Err(SyncError::Git("git mode not yet implemented (3c T4)".into()))
}
fn stage_git(_p: &Payload, _s: &Path) -> Result<Baseline, SyncError> {
    Err(SyncError::Git("git mode not yet implemented (3c T4)".into()))
}
fn collect_delta_git(_s: &Path, _b: &Baseline) -> Result<Delta, SyncError> {
    Err(SyncError::Git("git mode not yet implemented (3c T4)".into()))
}
fn apply_deltas_git(_w: &Path, _d: &[Delta]) -> Result<(), SyncError> {
    Err(SyncError::Git("git mode not yet implemented (3c T4)".into()))
}
```

> NOTE: the tar tests build a non-git tempdir, but `git2::Repository::discover` walks **upward** and may find the rupu repo above `/var/folders/...`? No — system temp dirs are not under a git repo. If CI runs temp under a repo, `detect_mode` could wrongly pick Git. Guard the tar tests by forcing tar mode: the tests call `pack`/`stage` etc. which dispatch on `detect_mode`. To keep the tar tests hermetic, the implementer should make the tar unit tests call the `*_tar` functions directly **OR** assert `detect_mode` first and skip if Git. Prefer calling `pack_tar`/`stage_tar`/`collect_delta_tar`/`apply_deltas_tar` directly in the tar unit tests (they are crate-private, same module) so they never depend on the temp dir's git ancestry. Update the test bodies in Step 2 to call the `_tar` variants directly except `tar_mode_detected_for_non_git_dir`, which asserts `detect_mode` and may be marked `#[ignore]` if the CI temp root is inside a repo.

- [ ] **Step 5: Export from lib.rs**

In `crates/rupu-workspace/src/lib.rs`, add with the other `pub mod` lines:
```rust
pub mod workspace_sync;
```
and a re-export with the other `pub use` lines:
```rust
pub use workspace_sync::{
    apply_deltas, collect_delta, detect_mode, pack, stage, Baseline, Delta, Payload, SyncError,
    SyncMode,
};
```

- [ ] **Step 6: Run tests, format, lint, commit**

```bash
cargo test -p rupu-workspace workspace_sync
rustfmt crates/rupu-workspace/src/workspace_sync.rs crates/rupu-workspace/src/lib.rs
cargo clippy -p rupu-workspace --all-targets --no-deps
git add Cargo.toml Cargo.lock crates/rupu-workspace/Cargo.toml crates/rupu-workspace/src/workspace_sync.rs crates/rupu-workspace/src/lib.rs
git commit -m "feat(multi-host): rupu-workspace tar sync codec + mode detect (3c T3)"
```
Expected: tar tests pass; clippy clean. (Cargo.lock changes from the new deps — commit it.)

---

## Task 4: `rupu-workspace` git codec (git2)

**Files:**
- Modify: `crates/rupu-workspace/src/workspace_sync.rs` (replace the four `*_git` stubs; add tests)
- Test: same file

**Interfaces:**
- Consumes: `Payload`, `Delta`, `Baseline`, `SyncMode`, `SyncError` (T3).
- Produces: working `pack_git` / `stage_git` / `collect_delta_git` / `apply_deltas_git`. Git delta `bytes` = a unified diff patch (the default per the spec; a commit-bundle is the documented alternative — use the patch). `Baseline.git_commit` = the staged snapshot commit oid.

**Resolved spec open questions:** (a) dirty-tree snapshot — capture tracked + modified + untracked-not-ignored via `git add -A` into a throwaway in-memory commit so the remote sees the real current tree; (b) return encoding — a unified diff **patch** applied with 3-way semantics.

- [ ] **Step 1: Write the failing tests**

Add a `mod git_tests` inside the file's test module (or a sibling `#[cfg(test)] mod git_sync_tests`):

```rust
#[cfg(test)]
mod git_sync_tests {
    use super::*;
    use std::fs;

    fn git_init(dir: &std::path::Path) {
        let repo = git2::Repository::init(dir).unwrap();
        // identity for commits
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "t").unwrap();
        cfg.set_str("user.email", "t@e").unwrap();
        fs::write(dir.join("a.txt"), "line1\nline2\nline3\n").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("a.txt")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
    }

    #[test]
    fn git_mode_detected_and_round_trips() {
        let ws = tempfile::tempdir().unwrap();
        git_init(ws.path());
        assert_eq!(detect_mode(ws.path()), SyncMode::Git);

        let payload = pack(ws.path()).unwrap();
        assert_eq!(payload.mode, SyncMode::Git);
        let scratch = tempfile::tempdir().unwrap();
        let baseline = stage(&payload, scratch.path()).unwrap();
        assert_eq!(fs::read_to_string(scratch.path().join("a.txt")).unwrap(), "line1\nline2\nline3\n");

        // remote edits line2
        fs::write(scratch.path().join("a.txt"), "line1\nEDITED\nline3\n").unwrap();
        let delta = collect_delta(scratch.path(), &baseline).unwrap();
        assert!(delta.changed.contains(&"a.txt".to_string()));

        apply_deltas(ws.path(), &[delta]).unwrap();
        assert_eq!(fs::read_to_string(ws.path().join("a.txt")).unwrap(), "line1\nEDITED\nline3\n");
    }

    #[test]
    fn git_non_overlapping_hunks_in_same_file_merge() {
        let ws = tempfile::tempdir().unwrap();
        git_init(ws.path()); // a.txt = line1/line2/line3

        let payload = pack(ws.path()).unwrap();
        // unit A edits line1; unit B edits line3 — same file, disjoint hunks.
        let sa = tempfile::tempdir().unwrap();
        let ba = stage(&payload, sa.path()).unwrap();
        fs::write(sa.path().join("a.txt"), "AAA\nline2\nline3\n").unwrap();
        let da = collect_delta(sa.path(), &ba).unwrap();

        let sb = tempfile::tempdir().unwrap();
        let bb = stage(&payload, sb.path()).unwrap();
        fs::write(sb.path().join("a.txt"), "line1\nline2\nBBB\n").unwrap();
        let db = collect_delta(sb.path(), &bb).unwrap();

        apply_deltas(ws.path(), &[da, db]).unwrap();
        assert_eq!(fs::read_to_string(ws.path().join("a.txt")).unwrap(), "AAA\nline2\nBBB\n");
    }

    #[test]
    fn git_conflicting_hunks_error() {
        let ws = tempfile::tempdir().unwrap();
        git_init(ws.path());
        let payload = pack(ws.path()).unwrap();

        // both units edit line2 differently — a real conflict.
        let sa = tempfile::tempdir().unwrap();
        let ba = stage(&payload, sa.path()).unwrap();
        fs::write(sa.path().join("a.txt"), "line1\nAAA\nline3\n").unwrap();
        let da = collect_delta(sa.path(), &ba).unwrap();

        let sb = tempfile::tempdir().unwrap();
        let bb = stage(&payload, sb.path()).unwrap();
        fs::write(sb.path().join("a.txt"), "line1\nBBB\nline3\n").unwrap();
        let db = collect_delta(sb.path(), &bb).unwrap();

        let err = apply_deltas(ws.path(), &[da, db]).unwrap_err();
        assert!(matches!(err, SyncError::Conflict(_)), "got {err:?}");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-workspace git_sync_tests`
Expected: FAIL — the git arms return the "not yet implemented" error.

- [ ] **Step 3: Implement the git arms**

Replace the four stubs. Use `git2`. Algorithm (the implementer should verify exact git2 0.19 method names against the docs; the shape is):

```rust
fn pack_git(workspace_path: &Path) -> Result<Payload, SyncError> {
    let repo = git2::Repository::discover(workspace_path).map_err(|e| SyncError::Git(e.to_string()))?;
    // Snapshot the working tree (tracked + modified + untracked-not-ignored)
    // into a throwaway commit WITHOUT touching the user's index/HEAD: build a
    // tree from a temporary in-memory index seeded from the workdir.
    let mut index = repo.index().map_err(|e| SyncError::Git(e.to_string()))?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(|e| SyncError::Git(e.to_string()))?;
    let tree_oid = index.write_tree().map_err(|e| SyncError::Git(e.to_string()))?;
    let tree = repo.find_tree(tree_oid).map_err(|e| SyncError::Git(e.to_string()))?;
    let sig = git2::Signature::now("rupu-sync", "sync@rupu").map_err(|e| SyncError::Git(e.to_string()))?;
    // Parent = current HEAD if any (so the bundle carries history for 3-way base).
    let parent = repo.head().ok().and_then(|h| h.target()).and_then(|oid| repo.find_commit(oid).ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let snap_oid = repo
        .commit(None, &sig, &sig, "rupu-sync snapshot", &tree, &parents)
        .map_err(|e| SyncError::Git(e.to_string()))?;
    // Bundle the snapshot commit (and ancestry) into bytes. git2 has no bundle
    // writer; produce a packfile of the snapshot's reachable objects via
    // `PackBuilder`, plus record the snapshot oid in a small self-describing
    // header so `stage_git` can check it out.
    let mut pb = repo.packbuilder().map_err(|e| SyncError::Git(e.to_string()))?;
    pb.insert_commit(snap_oid).map_err(|e| SyncError::Git(e.to_string()))?;
    let mut packbuf: Vec<u8> = Vec::new();
    pb.foreach(|chunk| { packbuf.extend_from_slice(chunk); true }).map_err(|e| SyncError::Git(e.to_string()))?;
    // header: 40-byte oid hex + packfile
    let mut bytes = snap_oid.to_string().into_bytes();
    bytes.extend_from_slice(&packbuf);
    Ok(Payload { mode: SyncMode::Git, bytes })
}
```

> If `PackBuilder`/`foreach` packing proves awkward in git2 0.19, the documented fallback is to shell out to `git bundle create` / `git clone` via `std::process::Command` (git is NOT guaranteed on the remote, but `pack_git`/`apply_deltas_git` run on the **coordinator**, which is a git repo with git available; only `stage_git`/`collect_delta_git` run remote, and those use git2 against the unpacked objects — no host git). The implementer should pick whichever git2 mechanism actually compiles and round-trips; the **contract** (these four functions, the `Payload`/`Delta`/`Baseline` shapes, patch-encoded delta, `SyncError::Conflict` on conflicting hunks) is fixed, the internal git2 calls are the implementer's to get right. Prove it with the Step-1 tests.

```rust
fn stage_git(payload: &Payload, scratch_dir: &Path) -> Result<Baseline, SyncError> {
    // Parse header (40-byte oid hex) + packfile; init a repo in scratch_dir,
    // import the pack, checkout the snapshot commit into the workdir.
    let oid_hex = std::str::from_utf8(&payload.bytes[..40]).map_err(|e| SyncError::Git(e.to_string()))?.to_string();
    let pack = &payload.bytes[40..];
    let repo = git2::Repository::init(scratch_dir).map_err(|e| SyncError::Git(e.to_string()))?;
    let mut odb = repo.odb().map_err(|e| SyncError::Git(e.to_string()))?;
    let mut writer = odb.packwriter().map_err(|e| SyncError::Git(e.to_string()))?;
    std::io::Write::write_all(&mut writer, pack).map_err(SyncError::Io)?;
    writer.commit().map_err(|e| SyncError::Git(e.to_string()))?;
    let oid = git2::Oid::from_str(&oid_hex).map_err(|e| SyncError::Git(e.to_string()))?;
    let commit = repo.find_commit(oid).map_err(|e| SyncError::Git(e.to_string()))?;
    repo.branch("rupu-sync", &commit, true).map_err(|e| SyncError::Git(e.to_string()))?;
    repo.set_head("refs/heads/rupu-sync").map_err(|e| SyncError::Git(e.to_string()))?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).map_err(|e| SyncError::Git(e.to_string()))?;
    Ok(Baseline { mode: SyncMode::Git, tar_manifest: Default::default(), git_commit: Some(oid_hex) })
}

fn collect_delta_git(scratch_dir: &Path, baseline: &Baseline) -> Result<Delta, SyncError> {
    // diff the staged commit's tree vs the current workdir; emit a unified
    // patch + the changed/deleted path lists.
    let repo = git2::Repository::open(scratch_dir).map_err(|e| SyncError::Git(e.to_string()))?;
    let oid = git2::Oid::from_str(baseline.git_commit.as_ref().unwrap()).map_err(|e| SyncError::Git(e.to_string()))?;
    let tree = repo.find_commit(oid).map_err(|e| SyncError::Git(e.to_string()))?.tree().map_err(|e| SyncError::Git(e.to_string()))?;
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let diff = repo.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts)).map_err(|e| SyncError::Git(e.to_string()))?;
    let mut changed = Vec::new();
    let mut deleted = Vec::new();
    diff.deltas().for_each(|d| {
        let path = d.new_file().path().or_else(|| d.old_file().path()).map(|p| p.to_string_lossy().replace('\\', "/"));
        if let Some(p) = path {
            if d.status() == git2::Delta::Deleted { deleted.push(p); } else { changed.push(p); }
        }
    });
    let mut patch = Vec::new();
    diff.print(git2::DiffFormat::Patch, |_d, _h, line| {
        match line.origin() { '+' | '-' | ' ' => patch.push(line.origin() as u8), _ => {} }
        patch.extend_from_slice(line.content());
        true
    }).map_err(|e| SyncError::Git(e.to_string()))?;
    changed.sort(); deleted.sort();
    Ok(Delta { mode: SyncMode::Git, changed, deleted, bytes: patch })
}

fn apply_deltas_git(workspace_path: &Path, deltas: &[Delta]) -> Result<(), SyncError> {
    // Apply each unit patch to the coordinator repo's workdir with 3-way
    // semantics. git2's `apply` with ApplyLocation::WorkDir applies a Diff;
    // parse each patch into a Diff via `Diff::from_buffer`. A failed apply
    // (overlapping/conflicting hunk) becomes SyncError::Conflict listing the
    // delta's changed paths.
    let repo = git2::Repository::discover(workspace_path).map_err(|e| SyncError::Git(e.to_string()))?;
    for d in deltas {
        let diff = git2::Diff::from_buffer(&d.bytes).map_err(|e| SyncError::Git(e.to_string()))?;
        repo.apply(&diff, git2::ApplyLocation::WorkDir, None)
            .map_err(|_| SyncError::Conflict(d.changed.clone()))?;
    }
    Ok(())
}
```

> The implementer must make the three Step-1 git tests pass, including the **3-way merge** behaviors: `apply` of disjoint hunks in the same file across two patches must both land; two patches touching the same line must yield `SyncError::Conflict`. If git2's plain `apply` does not give 3-way conflict semantics across sequential patches, apply each patch and re-derive the next against the updated workdir, or use `git2::merge_*` / a 3-way `apply` option — whatever makes the tests pass while preserving the contract. Do not weaken the tests.

- [ ] **Step 4: Run tests, format, lint, commit**

```bash
cargo test -p rupu-workspace workspace_sync
rustfmt crates/rupu-workspace/src/workspace_sync.rs
cargo clippy -p rupu-workspace --all-targets --no-deps
git add crates/rupu-workspace/src/workspace_sync.rs
git commit -m "feat(multi-host): rupu-workspace git sync codec via git2 (3c T4)"
```
Expected: all `workspace_sync` tests (tar + git) green; clippy clean.

---

## Task 5: Orchestrator routing — dispatch with workspace, apply deltas

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`run_linear_step`/`dispatch_placed_step` ~line 949; `run_fanout_step` ~line 1044; tests)
- Test: `crates/rupu-orchestrator/src/runner.rs`

**Interfaces:**
- Consumes: `effective_workspace_mode` (T1), `WorkspaceMode` (T1), `UnitDispatch.workspace_path` / `UnitOutcome.workspace_delta` / `apply_workspace_deltas` / `WorkspaceConflict` (T2), `opts.workflow.defaults`, `opts.workspace_path`.
- Produces: placed/fan-out steps with effective mode `Sync` set `workspace_path`, collect deltas, and call `apply_workspace_deltas` (once) before `StepCompleted`; a `WorkspaceConflict` becomes a step failure honoring `continue_on_error`.

- [ ] **Step 1: Write the failing tests**

Add to the runner test module. Extend the existing `FakeUnitDispatcher` (or add a `WorkspaceFakeDispatcher`) so it: records whether each `UnitDispatch.workspace_path` was `Some`; returns a `UnitOutcome.workspace_delta = Some(WorkspaceDelta{changed: vec![format!("u{idx}.txt")], deleted: vec![], payload: vec![]})`; and overrides `apply_workspace_deltas` to record the deltas it was asked to apply (and return `WorkspaceConflict` when constructed in "conflict" mode).

```rust
    const WF_PLACED_SYNC: &str = r#"
name: placed-sync
steps:
  - id: edit
    agent: coder
    prompt: "edit"
    host: worker-1
    workspace: sync
"#;

    #[tokio::test]
    async fn placed_sync_step_sends_workspace_and_applies_delta() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::new());
        let wf = Workflow::parse(WF_PLACED_SYNC).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), disp.clone());
        let res = run_workflow(opts).await.expect("ok");
        assert!(res.step_results[0].success);
        // dispatched WITH a workspace_path
        assert_eq!(disp.saw_workspace_path(), vec![true]);
        // applied exactly one delta set (single writer)
        assert_eq!(disp.applied_delta_counts(), vec![1]);
    }

    #[tokio::test]
    async fn no_sync_step_sends_no_workspace_path() {
        // 3b WF_PLACED (host: but no workspace:) must not set workspace_path.
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::new());
        let wf = Workflow::parse(WF_PLACED).unwrap(); // from 3b tests, host only
        let mut opts = make_opts(wf, dir.path().to_path_buf(), disp.clone());
        opts.inputs.insert("what".into(), "x".into());
        run_workflow(opts).await.expect("ok");
        assert_eq!(disp.saw_workspace_path(), vec![false]);
        assert!(disp.applied_delta_counts().is_empty()); // apply never called
    }

    #[tokio::test]
    async fn fanout_sync_collects_all_deltas_and_applies_once() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::new());
        let wf = Workflow::parse(
            r#"
name: fan-sync
steps:
  - id: edit
    for_each: "a\nb\nc"
    agent: coder
    prompt: "edit {{ item }}"
    max_parallel: 3
    workspace: sync
    distribute:
      hosts: [w1, w2]
"#,
        ).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), disp.clone());
        let res = run_workflow(opts).await.expect("ok");
        assert!(res.step_results[0].success);
        assert_eq!(disp.saw_workspace_path(), vec![true, true, true]);
        // applied once, with all 3 deltas together
        assert_eq!(disp.applied_delta_counts(), vec![3]);
    }

    #[tokio::test]
    async fn workspace_conflict_aborts_without_continue_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::with_conflict());
        let wf = Workflow::parse(WF_PLACED_SYNC).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), disp);
        let err = run_workflow(opts).await.expect_err("conflict must abort");
        assert!(matches!(err, RunWorkflowError::Agent { ref step, .. } if step == "edit"));
    }

    #[tokio::test]
    async fn workspace_conflict_tolerated_with_continue_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let disp = Arc::new(WorkspaceFakeDispatcher::with_conflict());
        let wf = Workflow::parse(
            r#"
name: placed-sync-tol
steps:
  - id: edit
    agent: coder
    prompt: "edit"
    host: worker-1
    workspace: sync
    continue_on_error: true
"#,
        ).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), disp);
        let res = run_workflow(opts).await.expect("tolerated");
        assert!(!res.step_results[0].success);
    }
```

The implementer writes `WorkspaceFakeDispatcher` (in the test module) implementing `UnitDispatcher`: `dispatch_unit` records `unit.workspace_path.is_some()` and returns `UnitOutcome { output: format!("out-{}", unit.index), success: true, error: None, workspace_delta: Some(WorkspaceDelta { changed: vec![format!("u{}.txt", unit.index)], deleted: vec![], payload: vec![] }) }`; `apply_workspace_deltas` records `deltas.len()` and returns `Ok(())` normally or `Err(WorkspaceConflict(vec!["shared".into()]))` in conflict mode. Accessors `saw_workspace_path()`/`applied_delta_counts()` return the recorded vectors.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-orchestrator -- placed_sync_step_sends no_sync_step_sends fanout_sync_collects workspace_conflict`
Expected: FAIL — routing not implemented (no `workspace_path` set; `apply_workspace_deltas` never called).

- [ ] **Step 3: Route the placed (linear) step**

In `dispatch_placed_step` (3b, ~line 951), the function builds the `UnitDispatch`. Thread in the effective mode: change its signature to accept `sync: bool` (computed by the caller as `effective_workspace_mode(step, &opts.workflow.defaults) == WorkspaceMode::Sync`), set `workspace_path: sync.then(|| opts.workspace_path.clone())` in the `UnitDispatch`, and on a successful outcome collect `outcome.workspace_delta` and, if `Some`, call `opts.unit_dispatcher`'s `apply_workspace_deltas(&opts.workspace_path, &[delta])`, mapping a `WorkspaceConflict` into the same failure path (`placed_failure(step, host, msg, RunError::Provider(conflict.to_string()), continue_on_error)`). Apply happens inside `dispatch_placed_step` before it returns `(output, success)`, so it precedes `StepCompleted`.

Concretely, after the `Ok(outcome) if outcome.success` arm obtains the output, before returning `Ok((output, true))`:
```rust
        Ok(outcome) if outcome.success => {
            if let Some(delta) = outcome.workspace_delta {
                if let (Some(dispatcher), Some(ws)) = (opts.unit_dispatcher.as_ref(), workspace_path_opt.as_ref()) {
                    if let Err(conflict) = dispatcher.apply_workspace_deltas(ws, &[delta]).await {
                        let src = RunError::Provider(conflict.to_string());
                        return placed_failure(step, host, conflict.to_string(), src, continue_on_error);
                    }
                }
            }
            Ok((outcome.output, true))
        }
```
where `workspace_path_opt = sync.then(|| opts.workspace_path.clone())` is also what populated `UnitDispatch.workspace_path`. `run_linear_step` computes `sync` and passes it (and keeps `opts` available to `dispatch_placed_step` — it already receives `opts`).

- [ ] **Step 4: Route the fan-out step**

In `run_fanout_step` (~line 1044): compute `let sync = effective_workspace_mode(step, &opts.workflow.defaults) == WorkspaceMode::Sync;` once. In each unit's `UnitDispatch { .. }` (the remote-path literals ~lines 1249/1280), set `workspace_path: sync.then(|| opts.workspace_path.clone())`. The per-unit `UnitOutcome.workspace_delta` is carried back on each `FanoutItemOutcome` (add a `workspace_delta: Option<WorkspaceDelta>` field to `FanoutItemOutcome`, populated from the outcome on the remote arm, `None` on the local arm). After all items join and the existing `continue_on_error` success check passes, if `sync`, collect `Some` deltas in unit-index order and call `apply_workspace_deltas(&opts.workspace_path, &deltas)` ONCE; map a `WorkspaceConflict` to the step's failure honoring `continue_on_error` (return a `RunWorkflowError::Agent { step: step.id.clone(), source: RunError::Provider(conflict.to_string()) }` when not tolerated; when tolerated, mark the step result `success=false` and proceed). Apply precedes the function's `Ok(StepResult { .. })` return, hence before `StepCompleted`.

- [ ] **Step 5: Run tests, full suite, format, lint, commit**

```bash
cargo test -p rupu-orchestrator -- placed_sync no_sync fanout_sync workspace_conflict
cargo test -p rupu-orchestrator
rustfmt crates/rupu-orchestrator/src/runner.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
git add crates/rupu-orchestrator/src/runner.rs
git commit -m "feat(multi-host): route workspace-sync steps + apply deltas (3c T5)"
```
Expected: 5 new tests pass; full orchestrator suite green (no-sync path byte-for-byte: the 3a/3b tests still pass, `workspace_path` defaults to `None`).

---

## Task 6: Transports + FleetUnitDispatcher wiring

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (`HostConnector` trait + `HostConnectorError`)
- Modify: `crates/rupu-cp/src/host/{local.rs,ssh.rs,http.rs,bucket.rs,tunnel.rs}` (impls)
- Modify: `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (compose + bridge + `apply_workspace_deltas`)
- Modify: `crates/rupu-cli/Cargo.toml` — ensure it depends on `rupu-workspace` (it already does for HostStore) 
- Test: `crates/rupu-cli/src/fleet_unit_dispatcher.rs`

**Interfaces:**
- Produces on `HostConnector`:
  - `async fn stage_workspace(&self, payload: Vec<u8>) -> Result<String, HostConnectorError>` — returns the remote working dir path. Default impl: `Err(HostConnectorError::Unsupported("workspace sync".into()))`.
  - `async fn collect_workspace_delta(&self, working_dir: &str) -> Result<Vec<u8>, HostConnectorError>` — default `Err(Unsupported)`.
  - new error variant `HostConnectorError::Unsupported(String)`.
- `FleetUnitDispatcher`: when `unit.workspace_path.is_some()`, `pack` (rupu-workspace) → `stage_workspace` → `launch_agent(working_dir = staged)` → poll → `collect_workspace_delta` → decode to a `rupu-workspace` `Delta` → convert to the orchestrator's `WorkspaceDelta`; implement `apply_workspace_deltas` by converting orchestrator deltas → `rupu-workspace::Delta` and calling `workspace_sync::apply_deltas`, mapping `SyncError::Conflict(paths)` → `WorkspaceConflict(paths)`.

- [ ] **Step 1: Write the failing tests**

In the `fleet_unit_dispatcher.rs` test module, add:

```rust
    /// A transport that does not support workspace sync (default trait impls)
    /// surfaces a clear Unsupported error through the dispatcher.
    #[tokio::test]
    async fn workspace_sync_on_unsupported_transport_errors() {
        // UnreachableConnector inherits the default stage_workspace = Unsupported.
        let conn = Arc::new(UnreachableConnector);
        let d = FleetUnitDispatcher::from_connector(conn);
        let mut unit = make_unit();
        unit.workspace_path = Some(std::path::PathBuf::from("/tmp/whatever"));
        let err = d.dispatch_unit(unit, "h1").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("workspace sync") || msg.contains("unsupported") || msg.contains("unreachable"));
    }

    /// apply_workspace_deltas bridges to the rupu-workspace tar codec: two
    /// disjoint tar deltas apply cleanly; overlap returns WorkspaceConflict.
    #[tokio::test]
    async fn apply_bridges_to_workspace_codec() {
        let conn = Arc::new(FakeConnector::completed());
        let d = FleetUnitDispatcher::from_connector(conn);
        let ws = tempfile::tempdir().unwrap();
        // build two disjoint tar-mode orchestrator deltas (payload = one-file tar)
        let a = rupu_orchestrator::runner::WorkspaceDelta { changed: vec!["a.txt".into()], deleted: vec![], payload: tar_one("a.txt", "A") };
        let b = rupu_orchestrator::runner::WorkspaceDelta { changed: vec!["b.txt".into()], deleted: vec![], payload: tar_one("b.txt", "B") };
        d.apply_workspace_deltas(ws.path(), &[a, b]).await.unwrap();
        assert!(ws.path().join("a.txt").exists());
        assert!(ws.path().join("b.txt").exists());
    }
```

(Reuse the `tar_one` helper pattern from T3, and confirm the exact import path of `WorkspaceDelta` — it is defined in the orchestrator runner module; use whatever the crate re-exports, e.g. `rupu_orchestrator::runner::WorkspaceDelta` or `rupu_orchestrator::WorkspaceDelta`. The bridge in the dispatcher must construct/consume the SAME type.)

> The bridge needs the orchestrator delta `payload` to be the codec's `Delta.bytes` plus enough to reconstruct `mode`/`changed`/`deleted`. Decide the payload encoding in this task: encode the `rupu-workspace::Delta` (mode tag + changed + deleted + bytes) into `WorkspaceDelta.payload` (e.g. a length-prefixed/`serde_json` header + raw bytes), and mirror `changed`/`deleted` into the orchestrator `WorkspaceDelta.changed`/`deleted` for observability. `apply_workspace_deltas` decodes `payload` back to `rupu-workspace::Delta`. Keep the encoding private to the dispatcher (the orchestrator stays opaque). The test above uses a tar payload — make `tar_one` produce whatever the bridge's decode expects (simplest: have the test build the delta via the same encode helper the dispatcher uses, exported `#[cfg(test)]` or via a small `pub(crate)` encode fn).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli --lib fleet_unit_dispatcher -- workspace_sync_on_unsupported apply_bridges`
Expected: FAIL — `stage_workspace`/`Unsupported`/the bridge not present.

- [ ] **Step 3: Add the trait methods + error variant**

In `crates/rupu-cp/src/host/connector.rs`, add to `HostConnectorError`:
```rust
    /// The operation is not supported on this transport (e.g. workspace sync
    /// over a Bucket/Tunnel host).
    #[error("unsupported on this transport: {0}")]
    Unsupported(String),
```
Add to the `HostConnector` trait (with default `Unsupported` impls so existing impls compile unchanged):
```rust
    /// Stage a packed workspace on the host; returns the remote working dir.
    async fn stage_workspace(&self, _payload: Vec<u8>) -> Result<String, HostConnectorError> {
        Err(HostConnectorError::Unsupported("workspace sync".into()))
    }
    /// Collect the workspace change-delta from a staged working dir.
    async fn collect_workspace_delta(&self, _working_dir: &str) -> Result<Vec<u8>, HostConnectorError> {
        Err(HostConnectorError::Unsupported("workspace sync".into()))
    }
```
Bucket (`bucket.rs`) and Tunnel (`tunnel.rs`) keep the defaults (Unsupported). Local/SSH/HttpCp override them (Step 4).

- [ ] **Step 4: Implement Local / SSH / HttpCp staging**

- **Local** (`local.rs`): `stage_workspace` writes the payload to a fresh temp dir under the CP cache, calls `rupu_workspace::stage` (it returns a `Baseline`; persist the baseline alongside, e.g. a sidecar file keyed by the working dir, so `collect_workspace_delta` can reload it), returns the dir. `collect_workspace_delta` reloads the baseline and calls `rupu_workspace::collect_delta`, encodes the `Delta` to bytes.
- **SSH** (`ssh.rs`): ship the payload to the host (scp/sftp to a temp path), run `rupu` on the host to stage (the remote rupu carries the codec — add a hidden helper subcommand `rupu __workspace stage <payload> <dir>` / `__workspace collect <dir>` OR reuse the existing remote-exec path the SSH transport already uses for launch_agent). Return the remote dir. `collect_workspace_delta` runs the remote collect and streams the delta bytes back.
- **HttpCp** (`http.rs`): POST the payload to a new CP endpoint `POST /api/workspace/stage` (returns `{working_dir}`) and `GET /api/workspace/delta?dir=...` (returns the delta bytes). Add the corresponding handlers in the CP API (`crates/rupu-cp/src/api/`) backed by `rupu_workspace::{stage,collect_delta}` with a baseline sidecar.

> Pick the minimal correct mechanism per transport. The baseline (for tar: the path→hash manifest; for git: the snapshot oid) must persist between `stage_workspace` and `collect_workspace_delta` since they are separate calls — store it in a sidecar file inside (or next to) the working dir. For SSH, prefer reusing the remote `rupu` rather than requiring host-side git/tar binaries (the codec is in the binary). Scratch dirs are cleaned after `collect_workspace_delta` returns (or on dispatcher drop). Enforce a payload size guard (a `const MAX_WORKSPACE_BYTES: usize` with a clear over-limit `HostConnectorError::Invalid`).

- [ ] **Step 5: Compose in FleetUnitDispatcher**

In `dispatch_unit` (fleet_unit_dispatcher.rs), when `unit.workspace_path` is `Some(ws)`:
```rust
// 1. pack on the coordinator
let payload = rupu_workspace::pack(&ws).map_err(|e| RunError::Provider(e.to_string()))?;
// (size guard)
// 2. stage on the host
let working_dir = connector.stage_workspace(encode_payload(&payload)).await.map_err(map_host_err)?;
// 3. launch the agent against the staged dir
let run_id = connector.launch_agent(AgentLaunchRequest { /* ... */ working_dir: Some(working_dir.clone()), .. }).await?;
// 4. poll get_run (unchanged)
// 5. collect the delta
let delta_bytes = connector.collect_workspace_delta(&working_dir).await.map_err(map_host_err)?;
let delta = decode_delta(&delta_bytes); // rupu_workspace::Delta
// 6. carry it back, converting to the orchestrator's opaque WorkspaceDelta
let workspace_delta = Some(to_orchestrator_delta(delta));
```
and set `workspace_delta` on the returned `UnitOutcome` (currently `None`). When `unit.workspace_path` is `None`, the path is byte-for-byte today (no staging, `workspace_delta: None`).

Implement `apply_workspace_deltas`:
```rust
async fn apply_workspace_deltas(&self, workspace_path: &Path, deltas: &[WorkspaceDelta]) -> Result<(), WorkspaceConflict> {
    let codec: Vec<rupu_workspace::Delta> = deltas.iter().map(from_orchestrator_delta).collect();
    match rupu_workspace::apply_deltas(workspace_path, &codec) {
        Ok(()) => Ok(()),
        Err(rupu_workspace::SyncError::Conflict(paths)) => Err(WorkspaceConflict(paths)),
        Err(e) => Err(WorkspaceConflict(vec![e.to_string()])), // non-conflict apply failure surfaces as a conflict-class step failure
    }
}
```
(`encode_payload`/`decode_delta`/`to_orchestrator_delta`/`from_orchestrator_delta` are private helpers in this file defining the wire encoding chosen in Step 1's note.)

- [ ] **Step 6: Run tests, format, lint, commit**

```bash
cargo test -p rupu-cli --lib fleet_unit_dispatcher
cargo test -p rupu-cp --lib host
rustfmt crates/rupu-cp/src/host/connector.rs crates/rupu-cp/src/host/local.rs crates/rupu-cp/src/host/ssh.rs crates/rupu-cp/src/host/http.rs crates/rupu-cli/src/fleet_unit_dispatcher.rs
cargo clippy -p rupu-cp --no-deps
cargo clippy -p rupu-cli --no-deps
git add crates/rupu-cp crates/rupu-cli
git commit -m "feat(multi-host): workspace staging transports + fleet wiring (3c T6)"
```
Expected: the two new fleet tests pass + connector default/override tests; scope test runs to the changed modules (ignore pre-existing unrelated `rupu-cli` `cmd::session::tests` failures and 1.95 clippy noise in untouched files).

---

## Task 7: End-to-end workspace-sync tests

**Files:**
- Create: `crates/rupu-orchestrator/tests/workspace_sync_e2e.rs`
- Test: that file

**Interfaces:**
- Consumes: the public runner API + the `RecordingDispatcher` pattern from `tests/distributed_fanout_e2e.rs` and `tests/placed_step_e2e.rs`; `WorkspaceDelta` / the `UnitDispatcher` trait (incl. `apply_workspace_deltas`).
- Produces: e2e proof that a synced placed step's file edit reaches a downstream step, plus a fan-out disjoint-merge case.

- [ ] **Step 1: Write the e2e tests**

First read the two sibling e2e files for the exact harness. Create `crates/rupu-orchestrator/tests/workspace_sync_e2e.rs` with a `RecordingDispatcher` that simulates a real workspace round-trip against an on-disk workspace dir: `dispatch_unit` writes the "remote edit" into a returned `WorkspaceDelta` (changed file + payload bytes the test's `apply_workspace_deltas` understands), and `apply_workspace_deltas` writes those changes into the coordinator `workspace_path` (so a later local step reading the file sees it).

```rust
//! End-to-end Slice 3c: a synced placed step's file edit reaches a later
//! local step; a fan-out disjoint-edit case merges without conflict.

// <copy imports + a RecordingDispatcher from the sibling e2e tests; the
//  dispatcher here ALSO implements apply_workspace_deltas to write changed
//  files into workspace_path, simulating the codec apply.>

#[tokio::test]
async fn synced_placed_step_edit_is_visible_to_downstream_local_step() {
    // workspace dir with an initial file
    // step 1: host: worker-1, workspace: sync, "agent" edits foo.txt
    //   -> dispatcher returns a delta; apply writes foo.txt = "EDITED" into workspace_path
    // step 2: LOCAL step whose prompt embeds the file contents (via a factory
    //   that reads workspace_path/foo.txt) -> asserts it sees "EDITED"
    // Assert: foo.txt on disk == "EDITED" after the run; step 2 saw it.
}

#[tokio::test]
async fn fanout_sync_disjoint_edits_merge() {
    // for_each over [x,y,z] with distribute + workspace: sync; each unit's
    // delta touches a disjoint file (x.txt/y.txt/z.txt); apply_workspace_deltas
    // is called once with all three and writes all three.
    // Assert: all three files exist in workspace_path; step succeeded.
}
```

> Fill the bodies concretely mirroring `tests/placed_step_e2e.rs` (placed/local chaining + real `RunStore`) and `tests/distributed_fanout_e2e.rs` (fan-out). The dispatcher's `apply_workspace_deltas` writes the changed files into `workspace_path`; the local downstream step's factory reads them. Use exact names from the sibling files — they are the source of truth for the harness shape.

- [ ] **Step 2: Run, format, lint, commit**

```bash
cargo test -p rupu-orchestrator --test workspace_sync_e2e
cargo test -p rupu-orchestrator
rustfmt crates/rupu-orchestrator/tests/workspace_sync_e2e.rs
cargo clippy -p rupu-orchestrator --all-targets --no-deps
git add crates/rupu-orchestrator/tests/workspace_sync_e2e.rs
git commit -m "test(multi-host): e2e workspace sync — placed edit + fanout merge (3c T7)"
```
Expected: both e2e tests pass; whole orchestrator suite green.

---

## Self-Review

**Spec coverage:**
- Auto git-or-tar (spine 1) → T3 (detect + tar) + T4 (git). ✅
- Changed-files delta return (spine 2) → T3 `collect_delta_tar` / T4 `collect_delta_git`. ✅
- Opt-in default+override (spine 3) → T1 (`defaults.workspace` + `Step.workspace` + `effective_workspace_mode`). ✅
- v1 placed-linear + fan-out (spine 4) → T5 (both routes). ✅
- Mode-aware conflict behind apply port (spine 5) → T2 (port) + T3 (tar disjoint) + T4 (git 3-way) + T5 (surface as step failure honoring continue_on_error). ✅
- No host git (spine 6) → T4 uses vendored git2; new deps tar+ignore only → T3. ✅
- Seam/opaque orchestrator → T2 types + defaulted method; bridge in T6. ✅
- Transports Local/SSH/HttpCp + Bucket/Tunnel Unsupported + clean failure → T6. ✅
- Errors/footprint (scratch cleanup, size guard, gitignore both ways, typed conflict) → T3/T4/T6. ✅
- Backward compat (no workspace ⇒ byte-for-byte) → T2 (defaults), T5 (None path unchanged), tests in T5. ✅
- Testing section (model, packer git+tar, conflict, routing, transport failure, backward compat, e2e) → T1/T3/T4/T5/T6/T7. ✅

**Placeholder scan:** The git2 internals (T4 Step 3) and the per-transport staging (T6 Step 4) are given as concrete algorithms with the real API calls plus an explicit contract and passing tests, with a note that the implementer verifies exact git2 0.19 method names — this is deliberate (library-integration code that must compile against a specific API) and is bounded by exact tests, not a vague "handle it." The e2e bodies (T7) direct copying the harness from two named sibling files. No "TBD"/"add error handling"/vacuous-test placeholders.

**Type consistency:** `WorkspaceMode {Sync,None}` (T1) used in T5 routing; `effective_workspace_mode` (T1) called in T5; orchestrator `WorkspaceDelta {changed,deleted,payload}` + `WorkspaceConflict(Vec<String>)` + `apply_workspace_deltas(&Path,&[WorkspaceDelta])->Result<(),WorkspaceConflict>` (T2) consumed in T5/T6/T7; rupu-workspace `pack/stage/collect_delta/apply_deltas` + `Payload/Delta/Baseline/SyncMode/SyncError` (T3) extended in T4 and bridged in T6; `HostConnector::{stage_workspace,collect_workspace_delta}` + `HostConnectorError::Unsupported` (T6) composed in the same task. Names are consistent across tasks.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-30-rupu-multi-host-slice-3c-plan.md`. Build via subagent-driven-development: fresh implementer per task, task review (spec + quality) after each, broad whole-branch review at the end, then a single PR to `main` (no self-merge — matt reviews before merge).
