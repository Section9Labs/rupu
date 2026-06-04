# Coverage Harness Slice B — Plan 3: Run Manifest + Rerun Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture a `RunManifest` for every coverage run on all four surfaces, and add `rupu coverage rerun <target_id> <run_id>` that faithfully replays an **agent-surface** run — producing a new run on the same target so it is immediately diffable, closing the `rerun → diff` loop.

**Architecture:** All four surfaces (workflow / agent / autoflow / session) funnel through `rupu-agent`'s `run_agent`, which initialises the coverage target in one place — so manifest *capture* is a single seam in `runner.rs`, not four wirings. The manifest is an append-only `runs.jsonl` next to the three existing ledgers. `rerun` reads a manifest, validates the surface (pure `plan_rerun` in `rupu-coverage`), and — for the agent surface — reconstructs a `rupu run` invocation and dispatches it through the existing `cmd::run::handle`. Agent runs scope their coverage `target_id` off the agent name, so the replay lands on the same target with no override plumbing. Session/workflow/autoflow `rerun` return an explicit "not yet supported" error (never a silent no-op).

**Tech Stack:** Rust 2021, `serde`/`serde_json` (JSONL), `chrono`, `clap`. Tests use `tempfile` + the existing `CapturingMockProvider`/`BypassDecider` harness.

**Spec:** `docs/superpowers/specs/2026-06-02-rupu-coverage-harness-slice-b-design.md` (Plan B-3 section).

**Depends on:** B-2 (deterministic prompt construction — so a replay renders the same catalog). Uses B-1's `latest` run selector for the diff hint.

---

## Scope note — read before implementing

The spec lists v1 `rerun` *dispatch* for **agent + session** surfaces. This plan implements **agent-surface dispatch only**; session/workflow/autoflow return the explicit `rerun of <surface> runs not yet supported` error. Reason: an agent rerun reuses the existing `rupu run` path unchanged (agent runs derive their coverage `target_id` from the agent name, so the replay accumulates on the same target automatically). Session reruns derive `target_id` from the `session_id`, which `rupu run` does not set — they need a `scope_name` override threaded through the run path, a larger change deferred to a fast-follow. **Manifest *capture* still happens on all four surfaces**, so every run is replay-describable and the session fast-follow only adds the dispatch path. This narrowing was flagged to matt at plan hand-off.

---

## File Structure

**`rupu-coverage` crate:**
- `src/ledger/paths.rs` *(modify)* — add `pub runs: PathBuf` (`runs.jsonl`) to `CoveragePaths`.
- `src/ledger/manifest.rs` *(create)* — `RunManifest` type + `append_manifest` / `read_manifests` / `find_manifest` I/O over `runs.jsonl`.
- `src/ledger/mod.rs` *(modify)* — `pub mod manifest;` + re-exports.
- `src/rerun.rs` *(create)* — `RerunInvocation`, `RerunError`, `plan_rerun` (pure surface-gated reconstruction).
- `src/lib.rs` *(modify)* — module decl + crate-root re-exports.

**`rupu-agent` crate:**
- `src/runner.rs` *(modify)* — append a `RunManifest` in the coverage-init block (the all-surfaces seam).
- `tests/coverage_integration.rs` *(modify)* — assert the manifest is captured.

**`rupu-cli` crate:**
- `src/cmd/coverage.rs` *(modify)* — `Rerun` subcommand + `run_rerun_in` (reconstruct → dispatch via `cmd::run::handle` → diff hint).

---

## Task 1: `RunManifest` type + `runs.jsonl` I/O

**Files:**
- Modify: `crates/rupu-coverage/src/ledger/paths.rs`
- Create: `crates/rupu-coverage/src/ledger/manifest.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`

- [ ] **Step 1: Add the `runs` path to `CoveragePaths`**

In `crates/rupu-coverage/src/ledger/paths.rs`, the `CoveragePaths` struct has fields `root`, `files`, `concerns`, `findings`, `catalog`, each a `PathBuf`, built in `CoveragePaths::new`. Add a `runs` field. Change the struct to include:

```rust
    pub runs: PathBuf,
```

(place it after `pub catalog: PathBuf,`) and in `CoveragePaths::new`, add to the constructed value (before `root,`):

```rust
            runs: root.join("runs.jsonl"),
```

Update the existing `paths_layout_under_dotrupu_coverage` test to also assert the new field — add after the `catalog` assertion:

```rust
        assert_eq!(paths.runs, paths.root.join("runs.jsonl"));
```

- [ ] **Step 2: Write the failing test for manifest I/O**

Create `crates/rupu-coverage/src/ledger/manifest.rs`:

```rust
use crate::catalog::types::ConcernsBlock;
use crate::ledger::events::Surface;
use crate::ledger::paths::CoveragePaths;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;

/// The defining inputs of a coverage run, captured at run start so the run
/// can be described and (for agent runs) replayed. Appended one-per-run to
/// `runs.jsonl` alongside the three event ledgers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunManifest {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub surface: Surface,
    pub agent_name: String,
    pub provider: String,
    pub model: String,
    pub permission_mode: String,
    pub user_prompt: String,
    /// The resolved concerns block the run was configured with (record of
    /// the catalog at run time).
    pub concerns: ConcernsBlock,
    /// The scope name used to derive this run's `target_id`
    /// (agent name for agent runs, session id for session runs, etc.).
    pub scope_name: String,
    pub workspace_path: std::path::PathBuf,
}

/// Append a manifest row to `runs.jsonl` (creates the file if absent).
pub fn append_manifest(paths: &CoveragePaths, manifest: &RunManifest) -> std::io::Result<()> {
    if let Some(parent) = paths.runs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.runs)?;
    let line = serde_json::to_string(manifest)?;
    writeln!(file, "{line}")
}

/// Read all manifests from `runs.jsonl` (empty vec if the file is absent).
/// Malformed lines are skipped, matching the other ledger readers.
pub fn read_manifests(paths: &CoveragePaths) -> std::io::Result<Vec<RunManifest>> {
    if !paths.runs.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&paths.runs)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<RunManifest>(l).ok())
        .collect())
}

/// Find the manifest for a specific run id, if present.
pub fn find_manifest(
    paths: &CoveragePaths,
    run_id: &str,
) -> std::io::Result<Option<RunManifest>> {
    Ok(read_manifests(paths)?.into_iter().find(|m| m.run_id == run_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::{ConcernsEntry, IncludeDirective};

    fn sample(run_id: &str) -> RunManifest {
        RunManifest {
            run_id: run_id.to_string(),
            started_at: DateTime::<Utc>::from_timestamp(1000, 0).unwrap(),
            surface: Surface::Agent,
            agent_name: "reviewer".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            permission_mode: "bypass".to_string(),
            user_prompt: "Review for security issues.".to_string(),
            concerns: ConcernsBlock {
                entries: vec![ConcernsEntry::Include(IncludeDirective {
                    include: "stride".to_string(),
                    overrides: vec![],
                    mode: crate::catalog::types::CatalogMode::Auto,
                    filter: None,
                })],
            },
            scope_name: "reviewer".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp/repo"),
        }
    }

    #[test]
    fn append_then_read_round_trips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        append_manifest(&paths, &sample("run_a")).unwrap();
        append_manifest(&paths, &sample("run_b")).unwrap();
        let all = read_manifests(&paths).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].run_id, "run_a");
        assert_eq!(all[1].run_id, "run_b");
        assert_eq!(all[0], sample("run_a"));
    }

    #[test]
    fn find_manifest_by_run_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        append_manifest(&paths, &sample("run_a")).unwrap();
        append_manifest(&paths, &sample("run_b")).unwrap();
        assert_eq!(find_manifest(&paths, "run_b").unwrap().unwrap().run_id, "run_b");
        assert!(find_manifest(&paths, "nope").unwrap().is_none());
    }

    #[test]
    fn read_absent_file_is_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        assert!(read_manifests(&paths).unwrap().is_empty());
    }
}
```

- [ ] **Step 3: Wire the module + re-exports**

In `crates/rupu-coverage/src/ledger/mod.rs`, add `pub mod manifest;` (next to the other `pub mod` lines) and add to the ledger re-export block:

```rust
pub use manifest::{append_manifest, find_manifest, read_manifests, RunManifest};
```

In `crates/rupu-coverage/src/lib.rs`, the `pub use ledger::{...}` block re-exports ledger items at the crate root. Add `append_manifest, find_manifest, read_manifests, RunManifest` to that list (keep it alphabetical-ish — e.g. after `Attribution`).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-coverage --lib ledger::manifest ledger::paths`
Expected: PASS (3 manifest tests + the updated paths test). Also `cargo build -p rupu-coverage` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/ledger/paths.rs crates/rupu-coverage/src/ledger/manifest.rs crates/rupu-coverage/src/ledger/mod.rs crates/rupu-coverage/src/lib.rs
git commit -m "feat(coverage): RunManifest type + runs.jsonl append/read/find"
```

---

## Task 2: Capture the manifest in `runner.rs` (all-surfaces seam)

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs`
- Modify: `crates/rupu-agent/tests/coverage_integration.rs`

**Context:** In `runner.rs`, the coverage-init block runs inside `if let Some(block) = opts.concerns.clone() { ... }`. It already computes `let resolved_scope = opts.scope_name.as_deref().unwrap_or(&opts.agent_name);`, `let target = target_id(&opts.workspace_path, resolved_scope);`, `let paths = CoveragePaths::new(&opts.workspace_path, &target);`, and calls `write_snapshot(&catalog, &paths.catalog)`. You append the manifest immediately after the snapshot write, while `block`, `resolved_scope`, and `paths` are in scope.

- [ ] **Step 1: Write the failing test (extend the existing capture test)**

In `crates/rupu-agent/tests/coverage_integration.rs`, the test `agent_run_with_concerns_writes_catalog_snapshot` already runs an agent with `concerns: Some(stride_block())`, `scope_name: None`, `surface_tag: None`, `run_id: "run_cov_test"`, `agent_name: "test-agent"`, `user_message: "Check coverage."`, then resolves `let target = target_id(&workspace, "test-agent"); let paths = CoveragePaths::new(&workspace, &target);`. Append these assertions to that test (after the existing tool-name assertions, before the closing brace). Add `RunManifest`, `read_manifests`, `Surface` to the `rupu_coverage::{...}` import at the top of the file:

```rust
    // Verify the run manifest was captured.
    let manifests = read_manifests(&paths).unwrap();
    assert_eq!(manifests.len(), 1, "exactly one manifest row expected");
    let m = &manifests[0];
    assert_eq!(m.run_id, "run_cov_test");
    assert_eq!(m.agent_name, "test-agent");
    assert_eq!(m.surface, Surface::Agent);
    assert_eq!(m.scope_name, "test-agent");
    assert_eq!(m.user_prompt, "Check coverage.");
    assert_eq!(m.workspace_path, workspace);
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-agent --test coverage_integration agent_run_with_concerns_writes_catalog_snapshot`
Expected: FAIL — `read_manifests` returns an empty vec (`manifests.len()` is 0), because nothing writes the manifest yet.

- [ ] **Step 3: Append the manifest in the coverage-init block**

In `crates/rupu-agent/src/runner.rs`, find the coverage-init block. Immediately after the `write_snapshot(&catalog, &paths.catalog).map_err(...)?;` call (and before `let handle = CoverageWriterHandle::spawn(...)`), insert:

```rust
            // Capture a run manifest describing this run's defining inputs.
            // This is the single all-surfaces seam (workflow / agent /
            // autoflow / session all reach run_agent), so every run becomes
            // replay-describable. Failure to write the manifest must not
            // abort the run — log and continue.
            let surface = match opts.surface_tag.as_deref() {
                Some("workflow") => rupu_coverage::Surface::Workflow,
                Some("autoflow") => rupu_coverage::Surface::Autoflow,
                Some("session") => rupu_coverage::Surface::Session,
                _ => rupu_coverage::Surface::Agent,
            };
            let manifest = rupu_coverage::RunManifest {
                run_id: opts.run_id.clone(),
                started_at: chrono::Utc::now(),
                surface,
                agent_name: opts.agent_name.clone(),
                provider: opts.provider_name.clone(),
                model: opts.model.clone(),
                permission_mode: opts.mode_str.clone(),
                user_prompt: opts.user_message.clone(),
                concerns: block.clone(),
                scope_name: resolved_scope.to_string(),
                workspace_path: opts.workspace_path.clone(),
            };
            if let Err(e) = rupu_coverage::append_manifest(&paths, &manifest) {
                tracing::warn!(error = %e, "failed to write coverage run manifest");
            }
```

(If `chrono` is not already imported in `runner.rs`, use the fully-qualified `chrono::Utc::now()` as shown — it is a workspace dependency of `rupu-agent`. `tracing` is already used in `runner.rs`.)

- [ ] **Step 4: Run to verify the test passes**

Run: `cargo test -p rupu-agent --test coverage_integration agent_run_with_concerns_writes_catalog_snapshot`
Expected: PASS.

- [ ] **Step 5: Verify the surface mapping with the workflow test**

The existing `surface_tag_override_is_respected` test runs with `surface_tag: Some("workflow".into())`, `scope_name: Some("...")`. Confirm nothing broke: run `cargo test -p rupu-agent --test coverage_integration`. Expected: all pass (the manifest append is additive).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-agent/src/runner.rs crates/rupu-agent/tests/coverage_integration.rs
git commit -m "feat(coverage): capture RunManifest on every run (all-surfaces seam)"
```

---

## Task 3: `plan_rerun` — surface-gated reconstruction

**Files:**
- Create: `crates/rupu-coverage/src/rerun.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-coverage/src/rerun.rs`:

```rust
//! Reconstruct a replayable invocation from a `RunManifest`, gated by
//! surface. v1 supports the agent surface; other surfaces return an
//! explicit error (never a silent no-op).

use crate::ledger::events::Surface;
use crate::ledger::manifest::RunManifest;

/// The validated subset of a manifest needed to replay an agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunInvocation {
    pub agent_name: String,
    pub user_prompt: String,
    pub permission_mode: String,
    pub workspace_path: std::path::PathBuf,
}

/// Why a run can't be replayed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RerunError {
    #[error("rerun of {0} runs not yet supported")]
    UnsupportedSurface(String),
}

/// Validate the manifest's surface and reconstruct the replay invocation.
/// v1: only `Surface::Agent` is dispatchable.
pub fn plan_rerun(manifest: &RunManifest) -> Result<RerunInvocation, RerunError> {
    match manifest.surface {
        Surface::Agent => Ok(RerunInvocation {
            agent_name: manifest.agent_name.clone(),
            user_prompt: manifest.user_prompt.clone(),
            permission_mode: manifest.permission_mode.clone(),
            workspace_path: manifest.workspace_path.clone(),
        }),
        Surface::Session => Err(RerunError::UnsupportedSurface("session".to_string())),
        Surface::Workflow => Err(RerunError::UnsupportedSurface("workflow".to_string())),
        Surface::Autoflow => Err(RerunError::UnsupportedSurface("autoflow".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};
    use chrono::{DateTime, Utc};

    fn manifest_with_surface(surface: Surface) -> RunManifest {
        RunManifest {
            run_id: "run_a".to_string(),
            started_at: DateTime::<Utc>::from_timestamp(1, 0).unwrap(),
            surface,
            agent_name: "reviewer".to_string(),
            provider: "anthropic".to_string(),
            model: "m".to_string(),
            permission_mode: "bypass".to_string(),
            user_prompt: "Review.".to_string(),
            concerns: ConcernsBlock {
                entries: vec![ConcernsEntry::Include(IncludeDirective {
                    include: "stride".to_string(),
                    overrides: vec![],
                    mode: crate::catalog::types::CatalogMode::Auto,
                    filter: None,
                })],
            },
            scope_name: "reviewer".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp/repo"),
        }
    }

    #[test]
    fn agent_surface_reconstructs_invocation() {
        let inv = plan_rerun(&manifest_with_surface(Surface::Agent)).unwrap();
        assert_eq!(inv.agent_name, "reviewer");
        assert_eq!(inv.user_prompt, "Review.");
        assert_eq!(inv.permission_mode, "bypass");
        assert_eq!(inv.workspace_path, std::path::PathBuf::from("/tmp/repo"));
    }

    #[test]
    fn session_surface_is_unsupported() {
        let err = plan_rerun(&manifest_with_surface(Surface::Session)).unwrap_err();
        assert_eq!(err, RerunError::UnsupportedSurface("session".to_string()));
        assert_eq!(err.to_string(), "rerun of session runs not yet supported");
    }

    #[test]
    fn workflow_and_autoflow_are_unsupported() {
        assert!(plan_rerun(&manifest_with_surface(Surface::Workflow)).is_err());
        assert!(plan_rerun(&manifest_with_surface(Surface::Autoflow)).is_err());
    }
}
```

- [ ] **Step 2: Wire the module + re-exports**

In `crates/rupu-coverage/src/lib.rs`, add `pub mod rerun;` (next to `pub mod audit;` etc.) and add a re-export line:

```rust
pub use rerun::{plan_rerun, RerunError, RerunInvocation};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p rupu-coverage --lib rerun`
Expected: 3 PASS. `cargo build -p rupu-coverage` clean (`thiserror` is already a dependency).

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage/src/rerun.rs crates/rupu-coverage/src/lib.rs
git commit -m "feat(coverage): plan_rerun — surface-gated replay reconstruction"
```

---

## Task 4: CLI `coverage rerun`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/coverage.rs`

**Context:** `coverage.rs`'s `handle` is `async`. Most arms are synchronous (`workspace().and_then(|ws| run_*_in(...))`), but `rerun` must `await` the existing `cmd::run::handle`. The dispatch reuses `rupu run`: build a `crate::cmd::run::Args` from the reconstructed invocation and call `crate::cmd::run::handle(args).await` (which returns `ExitCode`). The agent run derives its coverage `target_id` from the agent name, so the new run accumulates on the same target. The new run becomes `latest`, so the diff hint uses B-1's `latest` selector.

- [ ] **Step 1: Add the `Rerun` subcommand variant**

In `crates/rupu-cli/src/cmd/coverage.rs`, add to the `Action` enum (after `Runs`):

```rust
    /// Replay an agent run by id, appending a new run to the same target.
    Rerun {
        /// Target id (from `coverage list`).
        target_id: String,
        /// Run id to replay (from `coverage runs`).
        run_id: String,
    },
```

- [ ] **Step 2: Add the dispatch arm**

`rerun` is async and returns an `ExitCode` directly (it owns a full sub-run). Restructure `handle` to special-case it before the synchronous match. Change the body of `handle` so it reads:

```rust
pub async fn handle(action: Action, _format: Option<OutputFormat>) -> ExitCode {
    // `rerun` dispatches a full sub-run (async) and owns its own exit code.
    if let Action::Rerun { target_id, run_id } = action {
        return run_rerun_in(&target_id, &run_id).await;
    }
    let result = match action {
        Action::List => workspace().and_then(|ws| run_list_in(&ws)),
        Action::Templates { action } => run_templates(action),
        Action::Catalog { target_id } => workspace().and_then(|ws| run_catalog_in(&ws, &target_id)),
        Action::Show { target_id } => workspace().and_then(|ws| run_show_in(&ws, &target_id)),
        Action::Audit { target_id, json } => {
            workspace().and_then(|ws| run_audit_in(&ws, &target_id, json))
        }
        Action::Gap { target_id } => workspace().and_then(|ws| run_gap_in(&ws, &target_id)),
        Action::Diff {
            target_id,
            base,
            compare,
            json,
        } => workspace().and_then(|ws| run_diff_in(&ws, &target_id, base, compare, json)),
        Action::Runs { target_id, json } => {
            workspace().and_then(|ws| run_runs_in(&ws, &target_id, json))
        }
        Action::Rerun { .. } => unreachable!("handled above"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("coverage error: {e}");
            ExitCode::FAILURE
        }
    }
}
```

(Adjust the existing match to include whichever `Diff`/`Runs` arms already exist from B-1 — they are shown here for completeness; do not duplicate them.)

- [ ] **Step 3: Implement `run_rerun_in`**

Add after `run_runs_in`:

```rust
async fn run_rerun_in(target_id: &str, run_id: &str) -> ExitCode {
    let ws = match workspace() {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("coverage error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let paths = rupu_coverage::CoveragePaths::new(&ws, target_id);

    let manifest = match rupu_coverage::find_manifest(&paths, run_id) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!(
                "coverage error: no manifest for run '{run_id}' on target '{target_id}' \
                 (runs before Slice B are not replayable)"
            );
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("coverage error: reading manifests: {e}");
            return ExitCode::FAILURE;
        }
    };

    let invocation = match rupu_coverage::plan_rerun(&manifest) {
        Ok(inv) => inv,
        Err(e) => {
            // Explicit "not yet supported" for session/workflow/autoflow.
            eprintln!("coverage error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // The replay derives its target from the cwd; require the user to run
    // from the recorded workspace so the new run lands on the same target.
    let cwd = ws;
    if invocation.workspace_path != cwd {
        eprintln!(
            "coverage error: run '{run_id}' was recorded in workspace {:?}; \
             cd there and re-run `rupu coverage rerun {target_id} {run_id}`",
            invocation.workspace_path
        );
        return ExitCode::FAILURE;
    }

    println!(
        "rerun · replaying agent '{}' on target {} …",
        invocation.agent_name, target_id
    );

    let args = crate::cmd::run::Args {
        agent: invocation.agent_name.clone(),
        target: None,
        prompt: Some(invocation.user_prompt.clone()),
        mode: Some(invocation.permission_mode.clone()),
        no_stream: false,
        view: None,
        into: None,
        tmp: false,
    };
    let code = crate::cmd::run::handle(args).await;

    // The replay is now the most recent run on this target.
    println!();
    println!(
        "rerun complete · diff against the original with:\n  \
         rupu coverage diff {target_id} {run_id} latest"
    );
    code
}
```

- [ ] **Step 4: Write tests for the non-dispatch paths**

Add to the `#[cfg(test)] mod tests` in `coverage.rs`. These exercise the error paths that do NOT spawn an agent (missing manifest, unsupported surface, workspace mismatch are covered by `plan_rerun`'s own tests + `find_manifest`; here we assert the manifest plumbing the CLI relies on):

```rust
    #[test]
    fn rerun_missing_manifest_is_detectable() {
        use rupu_coverage::CoveragePaths;
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();
        // No runs.jsonl written → find_manifest returns None, which the CLI
        // surfaces as the "not replayable" error.
        assert!(rupu_coverage::find_manifest(&paths, "run_x").unwrap().is_none());
    }

    #[test]
    fn rerun_unsupported_surface_errors() {
        use rupu_coverage::{
            append_manifest, find_manifest, plan_rerun, CatalogMode, ConcernsBlock, ConcernsEntry,
            CoveragePaths, IncludeDirective, RunManifest, Surface,
        };
        use chrono::{DateTime, Utc};
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        let m = RunManifest {
            run_id: "run_sess".to_string(),
            started_at: DateTime::<Utc>::from_timestamp(1, 0).unwrap(),
            surface: Surface::Session,
            agent_name: "a".to_string(),
            provider: "anthropic".to_string(),
            model: "m".to_string(),
            permission_mode: "bypass".to_string(),
            user_prompt: "go".to_string(),
            concerns: ConcernsBlock {
                entries: vec![ConcernsEntry::Include(IncludeDirective {
                    include: "stride".to_string(),
                    overrides: vec![],
                    mode: CatalogMode::Auto,
                    filter: None,
                })],
            },
            scope_name: "ses_1".to_string(),
            workspace_path: tmp.path().to_path_buf(),
        };
        append_manifest(&paths, &m).unwrap();
        let loaded = find_manifest(&paths, "run_sess").unwrap().unwrap();
        assert!(plan_rerun(&loaded).is_err(), "session rerun must be rejected");
    }
```

- [ ] **Step 5: Run the tests + build**

Run: `cargo test -p rupu-cli --lib cmd::coverage` and `cargo build -p rupu-cli`
Expected: all coverage CLI tests pass (including the two new ones); build clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/coverage.rs
git commit -m "feat(cli): rupu coverage rerun — replay an agent run (agent surface v1)"
```

---

## Task 5: Verification, manual end-to-end, review

**Files:** none (verification only)

- [ ] **Step 1: Whole-workspace build + tests**

Run: `cargo build --workspace --tests` then `cargo test -p rupu-coverage && cargo test -p rupu-cli --lib cmd::coverage && cargo test -p rupu-agent --test coverage_integration`
Expected: all PASS.

- [ ] **Step 2: Clippy + fmt on touched crates**

Run: `cargo clippy -p rupu-coverage --lib --tests` and `cargo clippy -p rupu-agent --lib`.
Expected: clean. Then `cargo fmt -p rupu-coverage -- --check`, `cargo fmt -p rupu-cli -- --check`, `cargo fmt -p rupu-agent -- --check` — format only files THIS plan created/changed if they show diffs (`manifest.rs`, `rerun.rs`, the `runner.rs` block, the `coverage.rs` additions); do NOT reformat pre-existing drift in untouched files (the repo's `main` is fmt-dirty under the pinned toolchain 1.88 / rustfmt 1.9.0).

- [ ] **Step 3: Manual end-to-end (the rerun → diff loop)**

This is the acceptance demonstration. It needs a real agent + provider, so it is a manual check, not an automated test. Document the result in the PR. With a configured provider and an agent that declares `concerns:` in its frontmatter, from inside a repo:

```bash
rupu run <agent> "review for security issues"      # first run
rupu coverage runs <target_id>                      # find the run id (the only/older one)
rupu coverage rerun <target_id> <that_run_id>       # replays; appends a new run
rupu coverage diff <target_id> <that_run_id> latest # shows what the replay did differently
```
Expected: `rerun` re-runs the agent, the new run appears in `coverage runs`, and `diff … latest` reports the delta. Also confirm `rupu coverage rerun <target_id> <a_session_run_id>` prints `rerun of session runs not yet supported`.

- [ ] **Step 4: Commit any formatting**

```bash
git add -A
git commit -m "style(coverage): rustfmt manifest + rerun additions" || echo "nothing to format"
```

---

## Self-Review (completed by plan author)

**1. Spec coverage (B-3 section of the Slice B spec):**
- `runs.jsonl` append-only manifest next to the three ledgers → Task 1 (`CoveragePaths.runs` + `append_manifest`). ✅
- `RunManifest` defining-inputs fields → Task 1. (Deviation: `run_shaping` (`effort`/`context_window`) is **deferred** — v1 agent replay via `rupu run` re-resolves those from agent frontmatter and cannot apply stored overrides, so storing-but-not-applying them would be a no-op field. Recorded as a deferral, not a silent gap.) ✅ with noted deferral
- Capture on **all four surfaces** → Task 2 (single `runner.rs` seam all surfaces reach). ✅
- `rupu coverage rerun <target> <run_id>` reads manifest → reconstructs (pure `plan_rerun`) → dispatches → appends new run to same target → prints diff hint → Tasks 3 + 4. ✅
- Reconstruction is a pure library function (thin CLI) → Task 3 `plan_rerun` in `rupu-coverage`. ✅
- v1 dispatch = agent surface; session/workflow/autoflow → explicit named error (no silent no-op) → Task 3 (`RerunError::UnsupportedSurface`) surfaced by Task 4. **Narrowed from spec's "agent + session"** — flagged in the Scope note and at hand-off. ✅ with flagged narrowing
- Error handling: missing manifest, unsupported surface, wrong-workspace → Task 4. ✅
- Diff hint uses B-1's `latest` selector → Task 4. ✅
- Testing: manifest round-trip + capture integration test + `plan_rerun` unit tests + CLI error-path tests + manual e2e for the actual dispatch → Tasks 1-5. ✅

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; every command has an expected result. The manual e2e (Task 5 Step 3) is explicitly a manual check (real provider required), not an un-written automated test. ✅

**3. Type consistency:** `RunManifest` fields are identical across Task 1 (definition), Task 2 (construction in `runner.rs`), Task 3 (`plan_rerun` test fixtures), and Task 4 (CLI test fixture). `CoveragePaths.runs`, `append_manifest`/`read_manifests`/`find_manifest`, `RerunInvocation`/`RerunError`/`plan_rerun`, and `crate::cmd::run::Args` field names all match their real definitions (verified against `run.rs:26-57`). `Surface` variants (`Agent`/`Session`/`Workflow`/`Autoflow`) match `events.rs`. ✅

**Deferred to fast-follows (not this plan):** session/workflow/autoflow `rerun` dispatch; `run_shaping` capture + application; sampling-parameter control (spec's "Level 2").
