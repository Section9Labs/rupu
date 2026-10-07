# Customers — Plan 2A: CP backend (attribution, API, rollups, filters, launch preview) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Everything the CP web needs to show, filter and manage customers: every run records the customer it ran under, a customers API (CRUD, assignment, per-customer rollups, the customer config layer), a `?customer=` filter on the CP's list/aggregate endpoints that is honest about hosts that cannot report a customer, and a launch preview that says which customer and accounts a run will use.

**Architecture:** The config-path resolution Plan 1 put in `rupu-cli::paths` moves to `rupu-workspace::config_paths` so the CLI and the CP resolve a run's layers identically (and the CP's duplicate global-file check goes). Runs are attributed at launch: `RunRecord.customer` for workflow and standalone runs (carried through `StepFactory::customer()` and `ToolContext.customer`, so neither `OrchestratorRunOpts` nor `AgentRunOpts` gains a field), `RunStart.customer` in every agent transcript (standalone runs, session turns, workflow steps), and `SessionRecord.customer`. Runs recorded before this plan derive their customer from the current assignment and say so (`customer_derived`). The CP gets an `api/customers.rs` module, per-customer pricing, a shared `CustomerFilter`, and `POST /api/launch/preview` built on a pure `rupu_runtime::credential_manifest`.

**Tech Stack:** Rust 2021, axum, serde, `rupu-workspace`, `rupu-config`, `rupu-runtime`, `rupu-orchestrator`, `rupu-transcript`, `rupu-scm`, `rupu-codename`; tests with `tempfile`/`assert_fs`, CP integration tests in `crates/rupu-cp/tests/it/` (in-process axum + `reqwest`).

**Spec:** `docs/superpowers/specs/2026-10-06-rupu-customers-design.md` (§2 "Which runs get it", "Credential manifest"; §4 API). Plan 1: `docs/superpowers/plans/2026-10-06-rupu-customers-plan-1-model-config-cli.md`. Plan 2B (web UI) follows this plan and the approved mockup (https://claude.ai/artifact/88UtngsmAaJ6Nci6drCzfe).

## Global Constraints

- Hexagonal rule: `rupu-cli` is thin; business logic lives in libraries. CP handlers stay thin over library calls.
- Workspace deps only: in-workspace deps are `{ path = "../<crate>" }` (or `{ workspace = true }` where the crate already uses that form for it); no versions in crate `Cargo.toml` files.
- `#![deny(clippy::all)]`; no `unsafe`. CI clippy is 1.95: avoid `!x.is_some_and(..)` and `match` arms with only an `if`.
- Integration tests: ONE binary per crate under `crates/<c>/tests/it/` (listed in `main.rs`); `rupu-cli` env/cwd-mutating tests in `tests/serial/`, holding `ENV_LOCK`, run with `< /dev/null`.
- Never run package-wide `cargo fmt`; never rustfmt a whole file that is rustfmt-dirty on `main`.
- Never `git stash`; never push; never amend.
- CP load-time rules: no per-request full rescans beyond what list handlers already do; reuse `FileCache` / `UsageIndex` / `head_of`; never add a per-request SSH probe.
- Fail closed: a project with a customer never silently runs on, or is counted under, global/no customer. A remote host that cannot report customers is shown as such (HTTP 501 on a single-host request), never counted as zero.
- New serde fields on persisted records: `#[serde(default, skip_serializing_if = "Option::is_none")]` (old readers and records unaffected) — EXCEPT the CP row DTOs named below, which always serialize `customer` (`null` when none) so a coordinator can tell "no customer" from "peer too old to say".
- Run only targeted tests (`cargo test -p <crate> --test it <module>::`, `--lib <path>::`).

## Decisions made while planning (rulings)

1. **Plan 2 split into 2A (backend, this file) and 2B (web).** 2B is written after the mockup is approved.
2. **Attribution without option-struct churn.** `OrchestratorRunOpts` (167 literals) and `AgentRunOpts` (78) get no field. The workflow run record takes its customer from `StepFactory::customer()` — a defaulted trait method, the same precedent as `StepFactory::permission_mode()` (ISSUES.md I-24) — and `DefaultStepFactory` (13 literals) gains `customer`. Agent transcripts take it from `ToolContext.customer` (`ToolContext: Default`, most literals use `..Default::default()`). `RunRecord` (67 literals) and `Event::RunStart` (44) do gain the field — on purpose: `run.json` is read through `FileCache` by every lister, so the field is free to list, and the transcript is what the CP's usage sources read.
3. **Resume uses the recorded slug** when the run has one (a moved checkout no longer matters), else the Plan 1 `customer_dir` sidecar, else the workspace path. Sidecar writes stay (old binaries resuming new runs, and runs launched with no customer).
4. **Legacy runs** (no recorded `customer`) derive it from the current assignment of their `workspace_id` and carry `customer_derived: true` (same pattern as `codename_derived`).
5. **Remote rows.** List endpoints filter remote rows on the coordinator by the row's `customer` key. A remote row WITHOUT the key comes from a peer too old to report customers → that single-host request answers **501** ("`<host>` can't report customers — upgrade rupu there"), which the web's per-host engine already shows as "unavailable". Aggregates computed remotely (`/api/usage?host=<remote>`, `/api/dashboard?host=<remote>`) cannot be filtered by an old peer and are not filtered remotely in 2A: with `customer` set they answer 501 for remote hosts. Remote aggregate filtering is a later plan (TODO.md).
6. **Findings and projects** carry no recorded customer; `?customer=` filters them by the project's CURRENT assignment (findings via their `ws_id`). Documented in the API doc comments.
7. **`customer=none`** selects work with no customer (the picker's "Unassigned").
8. **Customer color.** `CustomerMeta.color` when set; otherwise derived from the slug with `rupu_codename::crew_for` + `crew_tint` (light/dark pair), so the web never invents colors.
9. **Per-customer pricing.** Rollups price a run with the pricing resolved for the customer it recorded (global + customer layer), cached per slug and re-validated by the stat of the customer's `config.toml` and the global `config.toml`.

## File map

| File | Change |
|---|---|
| `crates/rupu-workspace/src/config_paths.rs` | **new** — `project_root_for`, `ConfigPaths`, `config_paths`, `config_paths_for_customer`, R5 `project_config_path` |
| `crates/rupu-workspace/Cargo.toml`, `src/lib.rs` | `rupu-config` dep; module + re-exports |
| `crates/rupu-workspace/tests/it/config_paths.rs` | **new** tests |
| `crates/rupu-cli/src/paths.rs` | delegate to `rupu_workspace::config_paths` (keep the CLI function names); `ConfigPaths` re-exported |
| `crates/rupu-cp/src/api/config.rs` | `get_config` via shared paths; delete `is_same_file`; `?customer=`; `PUT /api/config/customer/:slug`; `put_project` rejects customer-locked keys |
| `crates/rupu-orchestrator/src/runs.rs` | `RunRecord.customer` |
| `crates/rupu-orchestrator/src/runner.rs` | `StepFactory::customer()`; record creation sets it |
| `crates/rupu-orchestrator/src/step_factory.rs` | `DefaultStepFactory.customer`; `tool_context.customer` |
| `crates/rupu-tools/src/tool.rs` | `ToolContext.customer` |
| `crates/rupu-transcript/src/event.rs`, `reader.rs` | `RunStart.customer`, `RunHead.customer` |
| `crates/rupu-agent/src/runner.rs` | `RunStart.customer` from `opts.tool_context.customer` |
| `crates/rupu-cli/src/cmd/{workflow,run,session,dispatch}.rs`, `src/resume.rs` | set the customer at launch; resume by slug |
| `crates/rupu-cp/src/customers.rs` | **new** — `CustomerRef`/DTO, tint, `CustomerFilter`, derived-customer helper, per-customer pricing cache |
| `crates/rupu-cp/src/api/customers.rs` | **new** — CRUD, assignment, list with rollups, detail |
| `crates/rupu-cp/src/usage_sources.rs` | `Head.customer`, `ExtraSource.customer` |
| `crates/rupu-cp/src/api/{runs,run_streams,sessions,findings,projects,usage,dashboard}.rs` | `customer` on rows; `?customer=` |
| `crates/rupu-runtime/src/credential_manifest.rs` | **new** pure manifest |
| `crates/rupu-cp/src/api/launch_preview.rs` | **new** `POST /api/launch/preview` |
| `crates/rupu-cp/src/api/mod.rs`, `src/server.rs` | register modules |
| `docs/…`, `TODO.md`, `CLAUDE.md` | docs |

---

### Task 1: One config-path resolver, shared by the CLI and the CP

**Files:**
- Create: `crates/rupu-workspace/src/config_paths.rs`
- Modify: `crates/rupu-workspace/Cargo.toml` (add `rupu-config = { path = "../rupu-config" }` under `[dependencies]` — check `rupu-config` is not already a dependency; it is reachable transitively through `rupu-runtime`, and depends on nothing in the workspace, so there is no cycle), `crates/rupu-workspace/src/lib.rs`
- Modify: `crates/rupu-cli/src/paths.rs` (delegate)
- Modify: `crates/rupu-cp/src/api/config.rs` (`get_config`, `put_project`, delete `is_same_file` and `project_customer_config_path` if fully replaced)
- Test: `crates/rupu-workspace/tests/it/config_paths.rs`, `crates/rupu-workspace/tests/it/main.rs`

**Interfaces:**
- Consumes: `rupu_workspace::CustomerStore::{new, customer_for_dir, config_path, get}` (Plan 1), `rupu_config::LayerPaths`.
- Produces:
  - `rupu_workspace::config_paths::project_root_for(pwd: &Path) -> std::io::Result<Option<PathBuf>>`
  - `rupu_workspace::ConfigPaths { pub global: PathBuf, pub customer: Option<PathBuf>, pub customer_slug: Option<String>, pub project: Option<PathBuf> }` with `fn layers(&self) -> rupu_config::LayerPaths<'_>`
  - `rupu_workspace::config_paths(home: &Path, project_root: Option<&Path>, run_dir: &Path) -> Result<ConfigPaths, CustomerError>` — the Plan 1 rule exactly (run_dir walk first, project_root fallback, R5: no project layer when `<project_root>/.rupu` is `home`).
  - `rupu_workspace::config_paths_for_customer(home: &Path, slug: Option<&str>, project_root: Option<&Path>) -> Result<ConfigPaths, CustomerError>` — layers for a known slug (resume by recorded slug); `Some(slug)` that has no customer directory → `CustomerError::NotFound(slug)`.
  - `rupu-cli::paths::{config_paths, config_paths_for_display, load_config_for_display}` keep their signatures and behaviour (now delegating); `crate::paths::ConfigPaths` becomes `pub use rupu_workspace::ConfigPaths`.

- [ ] **Step 1: Move the Plan 1 tests first (they must keep passing)**

Read `crates/rupu-cli/src/paths.rs` in full: the `ConfigPaths` type, `project_config_path` (the R5 rule), `config_paths`, `config_paths_for_display`, `load_config_for_display`, and their unit tests (`customer_layer_tests`, `the_global_dir_is_never_also_the_project_layer`, including the `#[cfg(unix)]` symlink case). Copy the tests that exercise `config_paths` / `project_config_path` (not the display loader) into `crates/rupu-workspace/tests/it/config_paths.rs`, rewritten against `rupu_workspace::{config_paths, ConfigPaths, CustomerStore, NewCustomer, ProjectRef}` (the error type is now `CustomerError`; assert on `.to_string()` contents exactly as the originals do). Add `mod config_paths;` to `tests/it/main.rs` (alphabetical). Add these new tests to the same file:

```rust
#[test]
fn config_paths_reports_the_customer_slug() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    let store = rupu_workspace::CustomerStore::new(&home);
    store
        .create("acme", &rupu_workspace::NewCustomer { name: "Acme".into(), ..Default::default() })
        .unwrap();
    store.assign("acme", rupu_workspace::ProjectRef::Path(&repo)).unwrap();
    let p = rupu_workspace::config_paths(&home, None, &repo).unwrap();
    assert_eq!(p.customer_slug.as_deref(), Some("acme"));
    assert_eq!(p.customer, Some(store.config_path("acme")));
}

#[test]
fn config_paths_for_customer_uses_the_slug_and_refuses_an_unknown_one() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let store = rupu_workspace::CustomerStore::new(&home);
    store
        .create("acme", &rupu_workspace::NewCustomer { name: "Acme".into(), ..Default::default() })
        .unwrap();
    let p = rupu_workspace::config_paths_for_customer(&home, Some("acme"), None).unwrap();
    assert_eq!(p.customer, Some(store.config_path("acme")));
    assert_eq!(p.customer_slug.as_deref(), Some("acme"));
    let none = rupu_workspace::config_paths_for_customer(&home, None, None).unwrap();
    assert_eq!(none.customer, None);
    assert!(matches!(
        rupu_workspace::config_paths_for_customer(&home, Some("gone"), None),
        Err(rupu_workspace::CustomerError::NotFound(_))
    ));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rupu-workspace --test it config_paths::`
Expected: compile errors (`config_paths`, `ConfigPaths` not in `rupu_workspace`).

- [ ] **Step 3: Write `config_paths.rs`**

Move the code, don't rewrite it: `project_root_for` (body identical, error type `std::io::Error`), the R5 `project_config_path` helper (verbatim, including its doc comment), `ConfigPaths` (+ `customer_slug`), `without_customer`, `layers()`, and `config_paths`. Shape:

```rust
//! Which config files one load layers — global, the project's customer,
//! and the project's `.rupu/config.toml` — shared by the CLI (every config
//! load) and the CP (Settings, the launch preview) so the two can never
//! resolve a run differently. Spec:
//! `docs/superpowers/specs/2026-10-06-rupu-customers-design.md` §1–2.

use crate::customers::{CustomerError, CustomerStore};
use std::path::{Path, PathBuf};

/// Walk up from `pwd` to the first directory containing `.rupu/`.
///
/// NOTE: `~/.rupu` (the global dir) counts, so from any directory under
/// `$HOME` without its own `.rupu/` this returns `$HOME`. Never key
/// per-repo state off it; see [`config_paths`] for how the customer and
/// project layers cope.
pub fn project_root_for(pwd: &Path) -> std::io::Result<Option<PathBuf>> { /* moved verbatim */ }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub global: PathBuf,
    pub customer: Option<PathBuf>,
    /// The slug `customer` belongs to — what a run records as its customer.
    pub customer_slug: Option<String>,
    pub project: Option<PathBuf>,
}

impl ConfigPaths {
    pub fn layers(&self) -> rupu_config::LayerPaths<'_> { /* as in rupu-cli */ }
    pub fn without_customer(home: &Path, project_root: Option<&Path>) -> Self { /* as in rupu-cli, customer_slug: None */ }
}

pub fn config_paths(home: &Path, project_root: Option<&Path>, run_dir: &Path) -> Result<ConfigPaths, CustomerError> {
    let store = CustomerStore::new(home);
    let mut slug = store.customer_for_dir(run_dir)?;
    if slug.is_none() {
        if let Some(root) = project_root {
            slug = store.customer_for_dir(root)?;
        }
    }
    Ok(ConfigPaths {
        customer: slug.as_deref().map(|s| store.config_path(s)),
        customer_slug: slug,
        ..ConfigPaths::without_customer(home, project_root)
    })
}

pub fn config_paths_for_customer(home: &Path, slug: Option<&str>, project_root: Option<&Path>) -> Result<ConfigPaths, CustomerError> {
    let store = CustomerStore::new(home);
    let customer = match slug {
        Some(s) => {
            store.get(s)?; // NotFound when the customer directory is gone
            Some(store.config_path(s))
        }
        None => None,
    };
    Ok(ConfigPaths {
        customer,
        customer_slug: slug.map(str::to_string),
        ..ConfigPaths::without_customer(home, project_root)
    })
}
```

`CustomerStore::get` returns `CustomerError::NotFound` for a missing customer (Plan 1, Task 1) — confirm, and that `customer_for_dir` already returns `Option<String>`.

In `lib.rs`: `pub mod config_paths;` and `pub use config_paths::{config_paths, config_paths_for_customer, project_root_for, ConfigPaths};`. Do not name the module and the function identically in a way that conflicts at the use site — if `pub use config_paths::config_paths` clashes with `pub mod config_paths`, keep the module public and re-export only the type and functions under the crate root (Rust allows a module and a function of the same name in different namespaces; verify it compiles).

- [ ] **Step 4: Delegate from the CLI**

In `crates/rupu-cli/src/paths.rs`: `pub use rupu_workspace::ConfigPaths;`, delete the local `ConfigPaths`, `project_config_path` and the moved tests; make `project_root_for` call `rupu_workspace::project_root_for(pwd).with_context(|| format!("canonicalize {}", pwd.display()))`; make `config_paths` call `rupu_workspace::config_paths(global, project_root, run_dir)?` (the `?` converts `CustomerError` into `anyhow`, keeping its message as the outermost text — Plan 1 F3); keep `config_paths_for_display` and `load_config_for_display` as they are (they call `config_paths` / `ConfigPaths::without_customer`). Keep the display-loader unit tests in the CLI.

- [ ] **Step 5: Use it in the CP**

In `crates/rupu-cp/src/api/config.rs` `get_config` with `?project=<ws_id>`: load the workspace record (`load_ws`). The CP's project layer has always been `<ws.path>/.rupu/config.toml` with no walk-up — keep that: call `rupu_workspace::config_paths(&s.global_dir, Some(Path::new(&ws.path)), Path::new(&ws.path))` and use the returned `customer` and `project` (R5 now lives in `config_paths`). Delete the CP's own `is_same_file` / global-dir check and route `put_project`'s "this project's config is the global config" refusal through the same rule (`rupu_workspace::config_paths(..).project.is_none()` while `<ws.path>/.rupu` exists ⇒ refuse). Keep every existing `api::config::` test passing unchanged.

- [ ] **Step 6: Run the tests**

```bash
cargo test -p rupu-workspace --test it
cargo test -p rupu-cli --lib paths::
cargo test -p rupu-cp --lib api::config::
cargo test -p rupu-cli --test serial customer_ < /dev/null
cargo check --workspace --all-targets
```
Expected: all pass; `cargo check` clean (except the pre-existing `rupu-agent` lib-test `extra_tools` error if `main` still has it — note it, do not fix it here).

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-workspace crates/rupu-cli/src/paths.rs crates/rupu-cp/src/api/config.rs
git commit -m "refactor(workspace): one config-path resolver shared by the CLI and the CP"
```

---

### Task 2: Workflow and standalone runs record their customer

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs` (`RunRecord.customer`)
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`StepFactory::customer`, record creation ~1173)
- Modify: `crates/rupu-orchestrator/src/step_factory.rs` (`DefaultStepFactory.customer`, `customer()`, `tool_context.customer`)
- Modify: `crates/rupu-tools/src/tool.rs` (`ToolContext.customer`)
- Modify: every `RunRecord { … }` literal (~67, compiler-listed) and `DefaultStepFactory { … }` literal (13)
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (factory gets the slug), `crates/rupu-cli/src/cmd/run.rs` (~1401 record + ~1177 `tool_context.customer`), `crates/rupu-cli/src/cmd/autoflow.rs` (factory)
- Modify: `crates/rupu-cli/src/resume.rs`, `crates/rupu-cli/src/cmd/workflow.rs` `resume_run` (resume by slug)
- Test: `crates/rupu-orchestrator/tests/it/` (new module `run_customer.rs`), `crates/rupu-cli/tests/serial/customer_layer.rs` (extend)

**Interfaces:**
- Consumes: Task 1's `ConfigPaths.customer_slug`, `config_paths_for_customer`.
- Produces:
  - `RunRecord.customer: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`), doc: "The customer this run ran under, recorded at launch (`None` = no customer, or a run recorded before customers existed)."
  - `trait StepFactory { fn customer(&self) -> Option<&str> { None } … }`
  - `DefaultStepFactory { …, pub customer: Option<String> }`
  - `ToolContext.customer: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`, `Default` = `None`)

- [ ] **Step 1: Failing test (orchestrator)**

Create `crates/rupu-orchestrator/tests/it/run_customer.rs` (add `mod run_customer;` to its `main.rs`). Build the smallest `run_workflow` invocation the module's neighbours use (copy the fixture from `tests/it/linear_runner.rs`'s simplest one-step test — a mock factory) with a factory whose `customer()` returns `Some("acme")`, and assert the persisted `RunRecord` (via `RunStore::load(&run_id)`) has `customer == Some("acme")`; with the default factory (`customer()` not overridden) it is `None`. Also assert that a `run.json` written WITHOUT the key (write one by hand from a serialized record with the field removed) deserializes with `customer == None`.

- [ ] **Step 2: Run it — expected compile failure** (`customer` unknown).

`cargo test -p rupu-orchestrator --test it run_customer::`

- [ ] **Step 3: Add the fields**

1. `RunRecord.customer` (attributes and doc as in Interfaces).
2. `StepFactory::customer` with the default body, doc: "The customer this run is attributed to (the CLI resolves it once, at launch). Recorded on the run's `RunRecord` and passed to every step's `ToolContext`. Defaulted like `permission_mode` so test factories need no change."
3. `ToolContext.customer` (`impl Default for ToolContext` — add `customer: None`).
4. In `run_workflow`'s record creation: `customer: opts.factory.customer().map(str::to_string),`.
5. `DefaultStepFactory.customer` + `fn customer(&self) -> Option<&str> { self.customer.as_deref() }`, and in `build_opts_for_step` set `tool_context.customer = self.customer.clone()` wherever it builds the step's `ToolContext`.
6. `cargo check --workspace --all-targets` and add `customer: None,` to every `RunRecord { … }` and `DefaultStepFactory { … }` literal the compiler lists — EXCEPT the production sites below, which set it for real. (`crates/rupu-cp/src/api/runs.rs` `synthesize_unpersisted_run` and `crates/rupu-orchestrator/src/executor/in_process.rs`'s pre-disk stub: `None`; `crates/rupu-cp/src/node/mirror.rs` `create_run_blocking`: copy the remote record's `customer` if the mirrored payload carries one, else `None` — read the function first.)

- [ ] **Step 4: Set it at launch (CLI)**

- `cmd/workflow.rs`: wherever the CLI constructs `DefaultStepFactory` for a run (`execute_workflow_invocation` and any other construction site the compiler shows), set `customer: cfg_paths.customer_slug.clone()` from the strict `paths::config_paths(..)` result already computed there (Plan 1 C1/R2 computed it at ~5518 for the config load — reuse that value; do not resolve twice). Autoflow constructions go through the same function; check `cmd/autoflow.rs` for direct `DefaultStepFactory` literals and set them from the same resolution.
- `cmd/run.rs` standalone `rupu run`: keep the `ConfigPaths` from its strict load (~580); set `tool_context.customer = cfg_paths.customer_slug.clone()` on the `AgentRunOpts.tool_context` it builds (~1177), and `customer: cfg_paths.customer_slug.clone()` on the `RunRecord` it writes (~1401).

- [ ] **Step 5: Resume by recorded slug**

In `crates/rupu-cli/src/resume.rs` (`rebuild_opts_from_disk`) and `cmd/workflow.rs` `resume_run`: if `record.customer` is `Some(slug)`, the config paths are `rupu_workspace::config_paths_for_customer(&global, Some(&slug), project_root.as_deref())?` (the run's recorded customer, even if the checkout moved or the project was reassigned since); else keep Plan 1's `customer_lookup_dir` path (sidecar, then workspace path). The rebuilt `DefaultStepFactory.customer` is the record's slug (or the looked-up one for an old run). Doc-comment the order. Keep writing the `customer_dir` sidecar at launch (unchanged).

- [ ] **Step 6: Serial test (end to end)**

Extend `crates/rupu-cli/tests/serial/customer_layer.rs`: after `workflow_run_takes_the_customer_default_provider`'s run, load the run with `rupu_orchestrator::RunStore::new(home.join("runs")).list()` and assert `customer == Some("acme")`. Add `rupu_run_records_the_customer` (a `rupu run` in an assigned project → the standalone `RunRecord` the run writes has `customer == Some("acme")`; find it the way the CP does — `RunStore::list()` filtered by `workflow_name` starting `agent:`). Add `resume_uses_the_recorded_customer_even_after_the_checkout_moves`: run a workflow that parks at an approval gate (copy the gate fixture from `tests/serial/approve_resume_run_step.rs`), rename the project directory, approve, and assert the resume does not fail with the R-B "launch directory no longer exists" error and the step transcript's provider is the customer's.

- [ ] **Step 7: Run and commit**

```bash
cargo test -p rupu-orchestrator --test it run_customer::
cargo test -p rupu-cli --test serial customer_layer:: < /dev/null
cargo test -p rupu-cli --test serial approve_ < /dev/null
cargo test -p rupu-cli --lib resume::
cargo clippy -p rupu-orchestrator -p rupu-tools -p rupu-cli -p rupu-cp --all-targets -- -D warnings
git add -A crates
git commit -m "feat(orchestrator,cli): runs record their customer; resume uses the recorded one"
```

---

### Task 3: Agent transcripts and sessions record their customer

**Files:**
- Modify: `crates/rupu-transcript/src/event.rs` (`RunStart.customer`), `crates/rupu-transcript/src/reader.rs` (`RunHead.customer`, `head()`)
- Modify: `crates/rupu-agent/src/runner.rs` (~1804 `RunStart` write)
- Modify: every `Event::RunStart { … }` literal (~44, compiler-listed)
- Modify: `crates/rupu-cli/src/cmd/session.rs` (`SessionRecord.customer`; turns set `tool_context.customer`), `crates/rupu-cli/src/cmd/dispatch.rs` (child inherits)
- Modify: `crates/rupu-cp/src/usage_sources.rs` (`Head.customer`, `ExtraSource.customer`), `crates/rupu-cp/src/api/sessions.rs` (`SessionDto.customer`)
- Test: `crates/rupu-transcript` unit test, `crates/rupu-cp/src/usage_sources.rs` unit test, `crates/rupu-cli/tests/serial/customer_layer.rs`

**Interfaces:**
- Consumes: `ToolContext.customer` (Task 2).
- Produces: `Event::RunStart { …, customer: Option<String> }` (`#[serde(skip_serializing_if = "Option::is_none", default)]`, like `codename`); `RunHead.customer: Option<String>`; `SessionRecord.customer: Option<String>` (serde default + skip-none); `ExtraSource.customer: Option<String>`; `SessionDto.customer: Option<String>`.

- [ ] **Step 1: Failing tests**

- `rupu-transcript`: in `event.rs`'s test module add a round trip: a `RunStart` with `customer: Some("acme")` serializes `"customer":"acme"` and reads back; a `run_start` line without the key reads `customer: None`. In `reader.rs`'s tests: `JsonlReader::head` on a transcript whose `RunStart` carries `customer` returns it.
- `rupu-cp` `usage_sources.rs` test module: an agent transcript whose `RunStart` has `customer: Some("acme")` yields an `ExtraSource` with `customer == Some("acme")` (follow the existing tests' fixture style there).

Run `cargo test -p rupu-transcript --lib` and `cargo test -p rupu-cp --lib usage_sources::` — expected compile failures.

- [ ] **Step 2: Add the fields and wire them**

1. `RunStart.customer` (+ doc: "Customer this run ran under (`rupu customer`). `None` on transcripts written before customers existed or for runs with no customer."), `RunHead.customer`, `head()` copies it.
2. `rupu-agent` runner: `customer: opts.tool_context.customer.clone(),` in the `RunStart` write.
3. Add `customer: None,` to every other `Event::RunStart { … }` literal the compiler lists (the session compaction pseudo-turn at `session.rs` ~7354: set it from the session's `customer`).
4. `SessionRecord.customer`: set at `start` from the strict `config_paths(..).customer_slug`; each turn / compact sets `opts.tool_context.customer` from THAT turn's `config_paths(..).customer_slug` (the turn already re-resolves config with `launch_dir` — reuse it) and updates `session.customer` if it changed (assignment changed mid-session; the record shows the current one, each transcript records its own).
5. `dispatch.rs`: a dispatched child's `ToolContext` inherits the parent's `customer` (check how the child's `tool_context` is built from the parent's and add the field there).
6. CP: `Head.customer`, `head_of` copies `RunHead.customer`; `ExtraSource.customer` = the head's; for a session turn whose transcript has no `RunStart.customer`, fall back to the session's `customer` from `session.json` (`SessionForRunsDto` — add `#[serde(default)] customer: Option<String>`). `SessionDto.customer` from `session.json`.

- [ ] **Step 3: Serial test**

Extend `customer_layer.rs`: `rupu run` in an assigned project → the transcript's first line contains `"customer":"acme"` (find the transcript via the `RunRecord`'s transcript dir or `<home>/transcripts`, as `first_step_run_start` does). A `rupu session start` + one turn (copy the minimal session fixture from `tests/serial/attach_loop_persists.rs` or the nearest session test) → `session.json` has `"customer":"acme"` and the turn transcript's `RunStart` too.

- [ ] **Step 4: Run and commit**

```bash
cargo test -p rupu-transcript --lib
cargo test -p rupu-cp --lib usage_sources::
cargo test -p rupu-cli --test serial customer_layer:: < /dev/null
cargo test -p rupu-cli --lib cmd::session::
cargo test -p rupu-agent --test it runner_basic::
cargo clippy -p rupu-transcript -p rupu-agent -p rupu-cli -p rupu-cp --all-targets -- -D warnings
git add -A crates
git commit -m "feat(transcript,cli,cp): agent transcripts and sessions record their customer"
```

---

### Task 4: CP customer model — DTOs, tint, filter, derived attribution, pricing

**Files:**
- Create: `crates/rupu-cp/src/customers.rs`
- Modify: `crates/rupu-cp/src/lib.rs` (`pub mod customers;`)
- Test: unit tests in `customers.rs`

**Interfaces:**
- Consumes: `rupu_workspace::{CustomerStore, Customer, CustomerMeta}`, `rupu_codename::{crew_for, crew_tint, Tint}`, `rupu_config::{resolve, LayerPaths, PricingConfig}`, `rupu_runtime::file_cache` stat helpers (or `std::fs::metadata` mtimes).
- Produces (used by Tasks 5–8):

```rust
/// What a row shows about its customer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomerRef {
    pub slug: String,
    pub name: String,
    pub tint: TintDto,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TintDto { pub light: String, pub dark: String }

#[derive(Debug, Clone, Serialize)]
pub struct CustomerDto {
    pub slug: String,
    pub name: String,
    pub notes: Option<String>,
    pub contact: Option<String>,
    pub color: Option<String>,
    pub tint: TintDto,
    pub archived: bool,
    pub created_at: String,
}

pub fn tint_for(slug: &str, color: Option<&str>) -> TintDto;          // color set ⇒ both = color; else crew tint of crew_for(slug); unknown crew ⇒ neutral #71717a/#a1a1aa
pub fn customer_ref(c: &rupu_workspace::Customer) -> CustomerRef;
pub fn customer_dto(c: &rupu_workspace::Customer) -> CustomerDto;

/// `?customer=<slug>` | `?customer=none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomerFilter { Slug(String), Unassigned }
impl CustomerFilter {
    /// `None` = no filter. Validates the slug with `rupu_workspace::validate_slug`
    /// (400 on a malformed one); `"none"` ⇒ `Unassigned`.
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, crate::error::ApiError>;
    pub fn matches(&self, customer: Option<&str>) -> bool;
}

/// A run's customer as rows report it: recorded, else derived from the
/// workspace's CURRENT assignment (`derived = true`), else none.
pub struct Attribution { pub slug: Option<String>, pub derived: bool }
pub fn attribute(store: &CustomerStore, recorded: Option<&str>, workspace_id: &str) -> Attribution;

/// Pricing for a customer's runs: global + that customer's layer, resolved
/// with `resolve` (locks honoured), cached per slug and re-validated by the
/// mtimes of the global and customer `config.toml`. `None` ⇒ global pricing.
pub struct CustomerPricing { /* Mutex<HashMap<String, (stamps, PricingConfig)>> */ }
impl CustomerPricing {
    pub fn new(global_dir: PathBuf, global: PricingConfig) -> Self;
    pub fn for_customer(&self, slug: Option<&str>) -> PricingConfig;     // a malformed/missing layer ⇒ warn once, global pricing
}
```

- [ ] **Step 1: Failing unit tests** (in `customers.rs` `#[cfg(test)] mod tests`):
  - `tint_for("acme", Some("#112233"))` ⇒ both `#112233`; `tint_for("acme", None)` equals the `crew_tint(&crew_for("acme"))` pair; stable across calls.
  - `CustomerFilter::parse(None)` ⇒ `Ok(None)`; `"acme"` ⇒ `Slug`; `"none"` ⇒ `Unassigned`; `"Bad Slug"` ⇒ `Err` (status 400); `matches`: `Slug("acme")` matches `Some("acme")` only; `Unassigned` matches `None` only.
  - `attribute`: recorded `Some` ⇒ that, `derived=false`; recorded `None` + workspace assigned to `acme` ⇒ `Some("acme")`, `derived=true`; unassigned ⇒ `None`, `derived=false`.
  - `CustomerPricing`: a customer layer with a `[pricing.anthropic."claude-x"]` entry prices differently from global; rewriting the layer file and moving its mtime forward with `std::fs::File::set_modified` (std ≥ 1.75; no sleeps in tests) invalidates the cache; `None` ⇒ global.
- [ ] **Step 2: Run → fail.** `cargo test -p rupu-cp --lib customers::`
- [ ] **Step 3: Implement** per the interface. Notes: `rupu_codename::crew_tint` returns `Option<Tint>` with `&'static str` fields. `attribute` uses `store.customer_of(workspace_id)` (Plan 1; validates the id — treat an `Err` as "unknown" ⇒ `None, derived=false`, logged once at debug). Never fail a listing because a legacy workspace id is odd.
- [ ] **Step 4: Run → pass. Commit** `feat(cp): customer refs, tints, filter, derived attribution, per-customer pricing`.

---

### Task 5: Customers API — CRUD, assignment, list with rollups, detail

**Files:**
- Create: `crates/rupu-cp/src/api/customers.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs`, `crates/rupu-cp/src/server.rs` (merge `customers::routes()`), `crates/rupu-cp/src/state.rs` (an `Arc<CustomerPricing>` on `AppState`, built where `pricing` is)
- Modify: `crates/rupu-cp/src/api/projects.rs` (`ProjectRow.customer: Option<CustomerRef>`)
- Test: `crates/rupu-cp/tests/it/customers.rs` (+ `main.rs`)

**Interfaces:**
- Consumes: Task 4; `rupu_workspace::CustomerStore`; `crate::usage::{rollup_by, summarize_run_usage, transcripts_usage, UsageSummary}`; `crate::usage_sources::unclaimed_extra_sources`; `crate::api::config::require_writable`; `crate::api::runs::blocking`.
- Produces (routes):

| Route | Body / query | Response |
|---|---|---|
| `GET /api/customers` | `?archived=1&range=7d\|30d\|all` (default `30d`) | `Vec<CustomerRow>` |
| `POST /api/customers` | `{slug, name, notes?, contact?, color?}` | `201 CustomerDto` (writable gate) |
| `GET /api/customers/:slug` | `?range=` | `CustomerDetail { customer: CustomerDto, rollup: CustomerRollup, projects: Vec<ProjectRow>, default_account: Option<DefaultAccount> }` |
| `PATCH /api/customers/:slug` | `{name?, notes?, contact?, color?}` (`""` clears) | `CustomerDto` |
| `POST /api/customers/:slug/archive`, `/unarchive` | — | `CustomerDto` |
| `DELETE /api/customers/:slug` | — | `204`, or `409 {"error": "...", "projects": [{ws_id, path}]}` |
| `PUT /api/customers/:slug/projects/:ws_id` | — | `ProjectRow` (assign) |
| `DELETE /api/customers/:slug/projects/:ws_id` | — | `204` (unassign; 404 if that project is not assigned to `:slug`) |

```rust
#[derive(Serialize)] pub struct CustomerRow { #[serde(flatten)] pub customer: CustomerDto, pub rollup: CustomerRollup, pub default_account: Option<DefaultAccount> }
#[derive(Serialize, Default)] pub struct CustomerRollup { pub projects: u64, pub run_count: u64, pub usage: crate::usage::UsageSummary, pub findings_open: u64, pub last_active: Option<String> }
#[derive(Serialize)] pub struct DefaultAccount { pub account: String, pub locked_by: Option<rupu_config::LockOwner>, pub inherited: bool }
```

Rules:
- Writes require `require_writable(&s)` (501 otherwise); `CustomerError` maps: `InvalidSlug`/`InvalidColor`/`EmptyName` ⇒ 400, `Exists` ⇒ 409, `NotFound`/`NoProject` ⇒ 404, `Archived` ⇒ 409, `HasProjects` ⇒ the 409 JSON above (build the `Response` directly: `(StatusCode::CONFLICT, Json(json!({...}))).into_response()`), anything else ⇒ 500. All store calls run in `blocking(..)`.
- **Rollups (list and detail):** in one blocking pass — `run_store.list()`, filtered to the range, each run's customer by `attribute(..)` (Task 4), priced with `CustomerPricing::for_customer(slug)` via `summarize_run_usage`, folded per slug (`run_count`, `usage`, `last_active`); plus `unclaimed_extra_sources` (their `customer`, else `attribute` by `workspace_id`) priced the same way, folded with `add_spend` (they add spend and activity, not `run_count` — same rule as `list_projects`). `projects` = `projects_of(slug).len()`; `findings_open` = open findings (`collect_all_findings`-style read the CP already uses for `count_open_findings`) whose `ws_id` is one of the customer's current projects. Compute everything once per request for all customers, not once per customer.
- **`default_account`**: `rupu_config::resolve(LayerPaths::new(Some(global), Some(customer_cfg), None))` ⇒ `config.default_provider` (`inherited = provenance["default_provider"].source != Customer`), `locked_by` from provenance. A malformed layer ⇒ `None` plus the row still listed (the detail endpoint returns the parse error in a `layer_error: Option<String>` field — add it to `CustomerDetail`).
- `ProjectRow.customer: Option<CustomerRef>` (always serialized) from the current assignment (`customer_of(ws_id)` + `get`); set in `project_row` callers (`list_projects`, `get_project`).

- [ ] **Step 1: Failing integration tests** — `crates/rupu-cp/tests/it/customers.rs`, using the `spawn_server` + tempdir pattern of `tests/it/projects.rs` (a writable state — check how other write tests build a launcher-bearing `AppState`; reuse that helper). Tests:
  1. POST creates (201, DTO with derived tint), duplicate ⇒ 409, bad slug ⇒ 400; non-writable state ⇒ 501.
  2. PATCH name/color, `""` clears contact.
  3. PUT assign → `GET /api/projects` row has `customer.slug == "acme"`; DELETE customer ⇒ 409 with `projects[0].ws_id`; unassign ⇒ 204; DELETE ⇒ 204.
  4. Rollups: seed two workflow runs (`RunStore::create`, one `customer: Some("acme")`, one `None` in an Acme-assigned workspace ⇒ derived) and one in another workspace; `GET /api/customers` ⇒ Acme `run_count == 2`, the other customer 0; a customer layer with distinct pricing makes Acme's `usage.cost` differ from global pricing of the same tokens (seed a transcript with usage the way `tests/it/usage.rs` does).
  5. `default_account`: layer `default_provider = "anthropic-acme"` + lock ⇒ `{account: "anthropic-acme", locked_by: "customer", inherited: false}`; no layer key ⇒ global's default with `inherited: true`.
  6. Archived customers hidden from the list unless `?archived=1`.
- [ ] **Step 2: Run → fail.** `cargo test -p rupu-cp --test it customers::`
- [ ] **Step 3: Implement** the module + registration + `ProjectRow.customer` + `AppState.customer_pricing`.
- [ ] **Step 4: Run → pass**, plus `cargo test -p rupu-cp --test it projects::` (row shape change) and `--lib api::projects::`.
- [ ] **Step 5: Commit** `feat(cp): customers API — CRUD, assignment, rollups, detail`.

---

### Task 6: Customer config layer over the API

**Files:**
- Modify: `crates/rupu-cp/src/api/config.rs`
- Test: `crates/rupu-cp/src/api/config.rs` test module (handler-level, like the existing ones) or `tests/it/customers.rs`

**Interfaces:**
- `GET /api/config?customer=<slug>` ⇒ `ConfigView` for global + that customer (no project): `effective`, `provenance` (with `Customer` source + `locked_by`), `raw_global`, plus NEW fields `raw_customer: Option<String>`, `customer: Option<CustomerRef>`, `customer_lock: Vec<String>`, `layer_error: Option<String>` (malformed layer ⇒ 200 with the error, effective = global only — the editor must still open to fix it). `?project=<ws_id>` also fills `customer`/`customer_lock` for the project's customer. `?project` and `?customer` together ⇒ 400.
- `PUT /api/config/customer/:slug` — body `ConfigWriteBody {raw?, patch?}` like `put_project`; writable gate; validate the candidate as a layer ON TOP OF GLOBAL (`layer_files_locked(LayerPaths::new(Some(global), Some(candidate_tmp), None))` — write the candidate to a temp file next to the target first, or validate via `validate_toml` then a resolve over a temp path; never write an invalid layer); `write_atomic`; 404 unknown slug. Its lock list is written with `patch: {"policy.lock": [...]}` exactly as `put_policy` does for global (the web lock toggle uses this).
- `put_project`: additionally reject keys locked by the project's customer (the deferred Plan 1 follow-up): `reject_locked_project_keys` takes the union of the global lock and the project's customer's `customer_lock` (from Task 1's `config_paths` + `resolve`), with an error naming which layer locks the key.

- [ ] **Step 1: Failing tests** — `?customer=acme` returns `raw_customer`, `customer.slug`, provenance `source: "customer"` for a key the layer sets; malformed layer ⇒ 200 with `layer_error`; PUT writes and the next GET reflects it; PUT invalid TOML ⇒ 400 and the file is unchanged; PUT with `patch {"policy.lock": ["default_provider"]}` ⇒ GET shows `customer_lock == ["default_provider"]`; `put_project` of a customer-locked key ⇒ 409/400 (match the existing global-lock rejection's status) mentioning "customer"; non-writable ⇒ 501.
- [ ] **Step 2: Run → fail.** `cargo test -p rupu-cp --lib api::config::`
- [ ] **Step 3: Implement.** Reuse `candidate_toml`, `validate_toml`, `write_atomic_blocking`, the dotted-key helpers (lockstep contract).
- [ ] **Step 4: Run → pass.** Also `cd crates/rupu-cp/web && npx tsc --noEmit` after adding the new optional `ConfigView` fields to `web/src/lib/api.ts` (types only).
- [ ] **Step 5: Commit** `feat(cp): customer config layer over the API; project writes honour customer locks`.

---

### Task 7: `?customer=` on the CP's lists and aggregates

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs` (`RunListRow.customer`, `customer_derived`; `/api/runs`, `/api/runs/workflows`), `crates/rupu-cp/src/api/run_streams.rs` (`AgentRunRow.customer`; `/api/runs/agents`), `crates/rupu-cp/src/api/sessions.rs` (`/api/sessions`), `crates/rupu-cp/src/api/findings.rs` (`/api/findings`), `crates/rupu-cp/src/api/projects.rs` (`/api/projects`), `crates/rupu-cp/src/api/usage.rs` (`/api/usage`, `/timeline`, `/runs`, `/outliers`), `crates/rupu-cp/src/api/dashboard.rs` (`/api/dashboard`)
- Test: `crates/rupu-cp/tests/it/customer_filter.rs` (+ `main.rs`)

**Interfaces / rules:**
- Every listed endpoint accepts `customer: Option<String>` in its query struct, parsed with `CustomerFilter::parse` (400 on a bad slug).
- `RunListRow` gains `customer: Option<String>` and `customer_derived: bool` — ALWAYS serialized (no `skip_serializing_if`), set from `attribute(..)` in `From<&RunRecord>`/`with_usage` (needs the store: compute in the list handlers, or pass the `CustomerStore` in — keep `From` pure and set the two fields after construction in the handler, as `host_id` is injected). Same for `AgentRunRow` (`customer` from the transcript head / `.meta.json`; derived via `workspace_id` when available) and the session rows (`SessionDto.customer`, Task 3).
- **Local rows:** filter after attribution, before pagination (`query_run_rows` must filter before it pages — otherwise a page can come back short or empty while matches exist; restructure so the filter applies to the list it pages).
- **Remote rows** (single-host `?host=<remote>`): page through the connector as today, then filter on the coordinator. If ANY returned row lacks the `customer` key (an old peer), answer **501** `{"error": "host <id> can't report customers (its rupu predates customers)"}` — through `host_list_error`'s 501 path so the web shows "unavailable". (An empty page is filterable — return it.) The fan-out variant (no `?host=`) skips such hosts and adds them to an `X-Rupu-Hosts-Without-Customer: <id>,<id>` response header.
- **Findings / projects:** by current assignment (ruling 6). Findings rows get `customer: Option<String>` (always serialized) from their `ws_id`.
- **Usage** (`/api/usage`, `/timeline`, `/runs`, `/outliers`): local only — filter sources by attribution before summarizing; price each source with `CustomerPricing::for_customer(its slug)` when a customer filter is set (and, for consistency, ALWAYS price per recorded customer — the spec's §1 pricing rule; this changes totals only for customers with their own `[pricing]`). With `customer` set and a remote `?host=` ⇒ 501 (ruling 5).
- **Dashboard** (`/api/dashboard`): local — filter the `&[RunRecord]` given to `build_summary` by attribution, `findings_open` by the customer's projects, autoflow `cycles` by repo ⇒ project ⇒ customer when resolvable (else excluded under a filter), `fleet` counts unfiltered (not run-scoped; say so in the doc comment). Remote host with `customer` ⇒ 501.

- [ ] **Step 1: Failing integration tests** (`customer_filter.rs`):
  1. Seed runs for two customers + one unassigned; `/api/runs?customer=acme` returns only Acme's; `?customer=none` only the unassigned; rows carry `customer` and `customer_derived`.
  2. Pagination: 30 runs where only the last 3 (oldest) are Acme's; `?customer=acme&limit=10&offset=0` returns those 3 (the filter applies before paging).
  3. `/api/projects?customer=acme`, `/api/findings?customer=acme` (seed a finding in an Acme project and one elsewhere), `/api/sessions?customer=acme`, `/api/usage?customer=acme` (only Acme's tokens), `/api/dashboard?customer=acme` (counts only Acme's runs).
  4. Bad slug ⇒ 400.
  5. Remote: register a fake HTTP host (follow the existing per-host tests' fake-peer pattern in `tests/it/` — find one with `grep -rn "fn spawn_fake_peer\|httpmock" crates/rupu-cp/tests/it | head`) whose `/api/runs` rows lack `customer` ⇒ `/api/runs?host=<id>&customer=acme` is 501; rows WITH `customer` are filtered; `/api/usage?host=<id>&customer=acme` ⇒ 501.
- [ ] **Step 2: Run → fail.** `cargo test -p rupu-cp --test it customer_filter::`
- [ ] **Step 3: Implement**, endpoint by endpoint; keep each handler's existing behaviour byte-identical when `customer` is absent (run the existing `runs::`, `sessions::`, `usage::`, `dashboard::`, `findings::`, `projects::` test modules after each).
- [ ] **Step 4: Run all affected modules:**
```bash
cargo test -p rupu-cp --test it customer_filter:: runs:: sessions:: usage:: dashboard:: findings:: projects::
cargo test -p rupu-cp --lib
cargo clippy -p rupu-cp --all-targets -- -D warnings
```
(If `cargo test --lib` for all of rupu-cp is slow, run the touched modules: `api::runs::`, `api::usage::`, `api::dashboard::`, `api::sessions::`, `api::findings::`, `api::projects::`.)
- [ ] **Step 5: web types** — add `customer: string | null` and `customer_derived: boolean` to `RunListRow`, `customer` to the session/agent-run/finding/project row types in `web/src/lib/api.ts`; `npx tsc --noEmit`.
- [ ] **Step 6: Commit** `feat(cp): ?customer= on runs, sessions, findings, projects, usage and dashboard`.

---

### Task 8: Launch preview and the credential manifest

**Files:**
- Create: `crates/rupu-runtime/src/credential_manifest.rs` (+ `pub mod` in `lib.rs`)
- Create: `crates/rupu-cp/src/api/launch_preview.rs` (+ registration)
- Test: unit tests in `credential_manifest.rs`; `crates/rupu-cp/tests/it/launch_preview.rs`

**Interfaces:**

```rust
// rupu-runtime
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRole { Provider, Fallback, Scm }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestEntry {
    pub role: AccountRole,
    pub account: String,
    /// The vendor the account authenticates against (`anthropic`, `openai`, `github`…), when known.
    pub kind: Option<String>,
    /// Which agent(s) use it (provider/fallback); empty for scm.
    pub agents: Vec<String>,
    /// Human-readable origin: "agent frontmatter", "customer default", "global default", "customer [recovery].fallbacks", "rule owner = acme-corp", …
    pub source: String,
}

/// What a run needs, per agent: its own `provider:`/`auth:`/`fallbacks:`.
pub struct AgentFacts<'a> { pub name: &'a str, pub provider: Option<&'a str>, pub fallbacks: Option<&'a [rupu_config::FallbackEntry]> }

/// Accounts a run will use. Pure: the caller resolves config (with its
/// provenance) and the SCM account. Deduplicates by (role, account), merging `agents`.
pub fn credential_manifest(
    cfg: &rupu_config::Config,
    provenance: &std::collections::BTreeMap<String, rupu_config::KeyProvenance>,
    agents: &[AgentFacts<'_>],
    scm: Option<ManifestEntry>,
) -> Vec<ManifestEntry>;
```

Rules: provider = `rupu_runtime::provider_factory::resolve_provider_name(agent.provider, cfg.default_provider)`; source "agent frontmatter" when the agent names it, else "customer default"/"global default"/"project default" by `provenance["default_provider"].source`, suffixed " · locked" when `locked_by` is set. Fallbacks = `cfg.recovery.chain_for(agent.fallbacks)` entries; an unnamed entry's account is the agent's provider (the `recovery_opts` rule); source by where the chain came from (agent vs `[recovery].fallbacks` provenance). `kind` = `cfg.providers[account].kind` else the account name when it parses as a vendor (`rupu_auth::account::resolve_provider_id` semantics — use the config's declared kinds; leave `None` when unknown).

```rust
// rupu-cp: POST /api/launch/preview
#[derive(Deserialize)]
pub struct PreviewBody {
    pub workflow: Option<String>,   // exactly one of workflow / agent
    pub agent: Option<String>,
    pub working_dir: Option<String>,
    pub scope_kind: Option<String>, // same semantics as LaunchBody
    pub scope_id: Option<String>,
    pub host: Option<String>,       // echoed; Plan 3 adds the per-host shipping plan
}
#[derive(Serialize)]
pub struct PreviewResponse {
    pub customer: Option<crate::customers::CustomerRef>,
    pub accounts: Vec<rupu_runtime::credential_manifest::ManifestEntry>,
    pub warnings: Vec<String>,      // resolver warnings, unknown agents, an unresolvable SCM account
}
```

Resolution (one blocking task): the launch directory = `working_dir`, else the directory `resolve_launch_scope` maps `scope_kind/scope_id` to (reuse the launch handler's helper; read `api/workflows.rs` `LaunchBody` handling), else the server's cwd (as launches do). `rupu_workspace::config_paths(global, project_root_for(dir), dir)` (strict — an error is a 409 `{"error": ...}` the launcher shows; the launch itself would fail the same way) ⇒ `rupu_config::resolve(paths.layers())`. Agents: for `workflow`, load the workflow definition the way the launch path finds it (project then global), collect its agent steps' agent names (including panel/parallel members and `for_each` agents — walk the parsed `rupu_orchestrator::Workflow`), load each agent spec with the same loader the CLI uses (`rupu_agent::spec` loader, project `.rupu` then global); unknown agents ⇒ a warning, not an error. SCM: the launch dir's `origin` remote (`git -C <dir> remote get-url origin`, the same detection `rupu_workspace::store` uses — make that helper `pub(crate)`→`pub` if needed) ⇒ `rupu_scm::weburl::parse_repo_remote` ⇒ `RepoRef { platform, owner, repo }` ⇒ `rupu_scm::rules::resolve_account(&rules, Some(&repo), Some(dir), home, None, &candidates, &candidates)` with `rules = cfg.scm.rules.iter().map(Rule::from_config)` and `candidates` = the config-declared accounts of that platform (bare `github`/`gitlab` plus `cfg.scm.platforms` keys whose `kind` — or name — parses to it, mirroring `cmd/scm.rs` `accounts_inner`); `Owner`/`Path`/`SoleAccount` ⇒ `ManifestEntry { role: Scm, source: "rule owner = <glob>" | "rule path = <glob>" | "only <platform> account" }`; anything else ⇒ a warning.

- [ ] **Step 1: Failing unit tests** (`credential_manifest.rs`): agent with own provider ⇒ "agent frontmatter"; agent without ⇒ customer default with " · locked" when locked; unnamed fallback entry ⇒ the agent's provider; two agents on one account ⇒ one entry listing both; scm entry appended.
- [ ] **Step 2: Failing integration test** (`tests/it/launch_preview.rs`): a project dir with `.rupu/workflows/w.yaml` (one step, agent `echo` without `provider:`) and `.rupu/agents/echo.md`, assigned to `acme` whose layer sets `default_provider = "anthropic-acme"` + `[[recovery.fallbacks]]`; a `git init` + `git remote add origin https://github.com/acme-corp/web.git` in the dir and a layer `[[scm.rules]] owner = "acme-corp" account = "github-acme"` + `[scm.github-acme] kind = "github"` in global; `POST /api/launch/preview {workflow:"w", working_dir}` ⇒ `customer.slug == "acme"`, accounts contain provider `anthropic-acme` ("customer default"), the fallback, scm `github-acme`. Dangling assignment ⇒ 409. Unknown agent ⇒ 200 + warning.
- [ ] **Step 3: Implement. Step 4: Run → pass:**
```bash
cargo test -p rupu-runtime --lib credential_manifest::
cargo test -p rupu-cp --test it launch_preview::
cargo clippy -p rupu-runtime -p rupu-cp --all-targets -- -D warnings
```
- [ ] **Step 5: web types** — `PreviewResponse` / `ManifestEntry` TS types + `api.launchPreview(body)` in `web/src/lib/api.ts`; `npx tsc --noEmit`.
- [ ] **Step 6: Commit** `feat(runtime,cp): credential manifest and POST /api/launch/preview`.

---

### Task 9: Docs and backlog

**Files:** `docs/configuration.md` (Customer layer section: runs record their customer; resume uses the recorded one), a new `docs/cp-customers-api.md` (route table from Tasks 5–8, the `?customer=` rules incl. 501 for remote/old peers, `customer=none`, derived attribution), `CLAUDE.md` (`rupu-cp` bullet: customers API, `?customer=` semantics, `credential_manifest`; `rupu-workspace` note for `config_paths`), `TODO.md` (move Plan 2 run attribution + manifest + `put_project` customer-lock items to ✅; ADD: remote aggregate filtering (usage/dashboard on remote hosts), customer on remote-mirrored tunnel runs if not carried; Plan 2B web UI).

- [ ] **Step 1:** Write the docs (accurate to what Tasks 1–8 shipped — read the final handlers for exact routes/status codes).
- [ ] **Step 2:** Commit `docs: customers CP API, attribution, backlog`.

---

### Task 10: Verify and review

- [ ] **Step 1: Targeted tests**

```bash
cargo test -p rupu-workspace --test it
cargo test -p rupu-transcript --lib
cargo test -p rupu-orchestrator --test it run_customer::
cargo test -p rupu-runtime --lib credential_manifest::
cargo test -p rupu-cp --test it customers:: customer_filter:: launch_preview:: projects:: runs:: usage:: dashboard::
cargo test -p rupu-cp --lib api::config:: customers:: usage_sources::
cargo test -p rupu-cli --lib paths:: resume:: cmd::session::
cargo test -p rupu-cli --test serial customer_ policy_lock:: approve_ resume_ < /dev/null
cd crates/rupu-cp/web && npx tsc --noEmit
```

- [ ] **Step 2:** `cargo clippy -p rupu-workspace -p rupu-config -p rupu-transcript -p rupu-tools -p rupu-agent -p rupu-orchestrator -p rupu-runtime -p rupu-cli -p rupu-cp --all-targets -- -D warnings`
- [ ] **Step 3:** `superpowers:requesting-code-review` on the branch diff; fix behaviour bugs in-branch.
