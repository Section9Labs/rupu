# Customers — Plan 1: model, config layer, CLI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A customer record in `RUPU_HOME` that groups projects by explicit assignment and adds a config layer between global and project, honoured by every config load, managed by a new `rupu customer` subcommand.

**Architecture:** `rupu-workspace` gains a `customers` module (metadata files, assignment sidecars, the run-directory → customer-layer lookup). `rupu-config`'s three loaders (`layer_files`, `layer_files_locked`, `resolve`) take a `LayerPaths { global, customer, project }`; `resolve` gains the customer tier and customer locks. Every existing loader call site is converted through one CLI helper so none can skip the customer layer. `rupu customer` is a thin clap front end over the store.

**Tech Stack:** Rust 2021, `serde`/`toml`, `thiserror`, `clap`, `comfy_table` (via `crate::output::tables`), `assert_fs` for tests.

**Spec:** `docs/superpowers/specs/2026-10-06-rupu-customers-design.md`

## Global Constraints

- Hexagonal rule: `rupu-cli` is thin — `cmd/customer.rs` parses arguments and calls `rupu_workspace::CustomerStore`; no business logic in the CLI.
- Workspace deps only: no versions in crate `Cargo.toml` files; in-workspace deps use `{ path = "../<crate>" }` like their neighbours.
- `#![deny(clippy::all)]`; no `unsafe`.
- Integration tests: ONE binary per crate — add modules under `crates/<c>/tests/it/` (listed in `main.rs`); `rupu-cli` env/cwd-mutating tests go in `crates/rupu-cli/tests/serial/` and hold `ENV_LOCK` for their whole body. Never add a top-level `tests/*.rs`.
- Never run package-wide `cargo fmt`; format only the files you touched (`rustfmt --edition 2021 <file>`).
- Never use bare `git stash` / `git stash pop` (shared stash across worktrees).
- Slug grammar: `[a-z0-9][a-z0-9-]{0,62}`; immutable.
- Precedence (highest first): global lock → customer lock → project → customer → global → default.
- Arrays replace, never concatenate (unchanged).
- Dangling assignment or malformed customer layer **fails every launch path**; it never falls back to global config.
- Run tests narrowly: `cargo test -p <crate> --test it <module>::` (never a cold full-workspace `cargo test`).

## Deviations from the spec (decided while planning)

1. **Run attribution moves to Plan 2.** Recording `customer` on `RunRecord` and the transcript `RunStart` means threading a field through `AgentRunOpts`, which has ~150 struct-literal construction sites, and it has no consumer until Plan 2's rollups and filter. Plan 2 owns it together with the rollup design. Runs from between the two plans carry no slug; Plan 2 derives one from the current assignment for them and marks it derived (as `codename_derived` does).
2. **The credential manifest moves to Plan 2.** Its first consumer is the CP launch preview; shipping it now would be code with no caller.
3. **`rupu config get` is unchanged.** It reads only the global file today (its module doc says so) and does not resolve the project layer either; effective config with provenance is shown by `rupu customer show`.
4. **`KeyProvenance.locked` stays**, next to the new `locked_by`, rather than becoming a serialized alias — no consumer has to change and the CP keeps working unmodified.
5. **The customer of a run is found by walking up from the run directory.** Workspace records are keyed by the directory `rupu` ran in (`upsert(&ws_store, &pwd)`), not the project root, so a run from a subdirectory has its own record. The lookup takes the nearest ancestor (inclusive) whose workspace record has an assignment, so subdirectories inherit their project's customer.

## File map

| File | Change |
|---|---|
| `crates/rupu-workspace/src/customers.rs` | **new** — `CustomerMeta`, `Customer`, `NewCustomer`, `MetaPatch`, `ProjectRef`, `CustomerError`, `CustomerStore`, `validate_slug` |
| `crates/rupu-workspace/src/store.rs` | add `find_by_path`, `register`; `upsert` uses `find_by_path` |
| `crates/rupu-workspace/src/lib.rs` | `pub mod customers;` + re-exports |
| `crates/rupu-workspace/tests/it/customers.rs` | **new** test module |
| `crates/rupu-config/src/layer.rs` | `LayerPaths`; three-layer `layer_files` / `layer_files_locked`; `LayerError::Layered.customer_path` |
| `crates/rupu-config/src/resolve.rs` | three-layer `resolve`; `KeySource::Customer`; `LockOwner`; `KeyProvenance.locked_by`; `Resolved.customer_lock` / `warnings` |
| `crates/rupu-config/src/lib.rs` | re-export `LayerPaths`, `LockOwner` |
| `crates/rupu-config/tests/it/customer_layer.rs` | **new** test module |
| `crates/rupu-cli/src/paths.rs` | `ConfigPaths` + `config_paths(global, cwd, project_root)` |
| every loader call site (Task 3 inventory) | converted to `LayerPaths` |
| `crates/rupu-cp/src/...` call sites | converted to `LayerPaths` |
| `crates/rupu-cli/src/cmd/customer.rs` | **new** — `rupu customer` |
| `crates/rupu-cli/src/cmd/mod.rs`, `src/lib.rs` | register the subcommand |
| `crates/rupu-cli/tests/serial/customer_layer.rs` | **new** serial tests |
| `docs/configuration.md`, `CLAUDE.md` | docs |

---

### Task 1: Customer store in `rupu-workspace`

**Files:**
- Create: `crates/rupu-workspace/src/customers.rs`
- Modify: `crates/rupu-workspace/src/store.rs` (add `find_by_path`, `register`, make `detect_repo_remote` / `detect_initial_branch` reachable; refactor `upsert`)
- Modify: `crates/rupu-workspace/src/lib.rs`
- Test: `crates/rupu-workspace/tests/it/customers.rs`, `crates/rupu-workspace/tests/it/main.rs`

**Interfaces:**
- Produces (used by Tasks 3 and 4):
  - `rupu_workspace::CustomerStore { pub home: PathBuf }` with
    `fn new(home: impl Into<PathBuf>) -> Self`,
    `fn list(&self, include_archived: bool) -> Result<Vec<Customer>, CustomerError>`,
    `fn get(&self, slug: &str) -> Result<Customer, CustomerError>`,
    `fn create(&self, slug: &str, new: &NewCustomer) -> Result<Customer, CustomerError>`,
    `fn update_meta(&self, slug: &str, patch: &MetaPatch) -> Result<Customer, CustomerError>`,
    `fn set_archived(&self, slug: &str, archived: bool) -> Result<Customer, CustomerError>`,
    `fn delete(&self, slug: &str) -> Result<(), CustomerError>`,
    `fn assign(&self, slug: &str, project: ProjectRef<'_>) -> Result<Workspace, CustomerError>`,
    `fn unassign(&self, project: ProjectRef<'_>) -> Result<Workspace, CustomerError>`,
    `fn customer_of(&self, ws_id: &str) -> Result<Option<String>, CustomerError>`,
    `fn projects_of(&self, slug: &str) -> Result<Vec<Workspace>, CustomerError>`,
    `fn config_path(&self, slug: &str) -> PathBuf`,
    `fn customer_for_dir(&self, dir: &Path) -> Result<Option<String>, CustomerError>`,
    `fn customer_config_for_dir(&self, dir: &Path) -> Result<Option<PathBuf>, CustomerError>`.
  - `rupu_workspace::{Customer { slug: String, meta: CustomerMeta }, CustomerMeta, NewCustomer, MetaPatch, ProjectRef, CustomerError, validate_slug}`.
  - `rupu_workspace::store::{find_by_path, register}`:
    `pub fn find_by_path(store: &WorkspaceStore, path: &Path) -> Result<Option<Workspace>, StoreError>`,
    `pub fn register(store: &WorkspaceStore, path: &Path) -> Result<Workspace, StoreError>` (find or create; does **not** bump `last_run_at`).

- [ ] **Step 1: Write the failing tests**

Add `mod customers;` to `crates/rupu-workspace/tests/it/main.rs` (keep the list alphabetical: `customers`, `discover`, `record`, `upsert`). Create `crates/rupu-workspace/tests/it/customers.rs`:

```rust
use assert_fs::prelude::*;
use rupu_workspace::{
    upsert, CustomerError, CustomerStore, MetaPatch, NewCustomer, ProjectRef, WorkspaceStore,
};

fn home() -> assert_fs::TempDir {
    assert_fs::TempDir::new().unwrap()
}

fn acme() -> NewCustomer {
    NewCustomer {
        name: "Acme Corp".into(),
        notes: None,
        contact: Some("ops@acme.example".into()),
        color: Some("#3366cc".into()),
    }
}

fn ws_store(home: &std::path::Path) -> WorkspaceStore {
    WorkspaceStore {
        root: home.join("workspaces"),
    }
}

#[test]
fn slug_grammar_is_enforced() {
    for ok in ["acme", "a", "acme-2", "0day"] {
        rupu_workspace::validate_slug(ok).unwrap();
    }
    for bad in ["", "-acme", "Acme", "ac me", "acme_corp", &"a".repeat(64)] {
        assert!(
            matches!(
                rupu_workspace::validate_slug(bad),
                Err(CustomerError::InvalidSlug(_))
            ),
            "{bad:?} should be rejected"
        );
    }
}

#[test]
fn create_then_get_round_trips_and_writes_an_empty_layer() {
    let h = home();
    let store = CustomerStore::new(h.path());
    let c = store.create("acme", &acme()).unwrap();
    assert_eq!(c.slug, "acme");
    assert_eq!(c.meta.name, "Acme Corp");
    assert!(!c.meta.archived);
    assert_eq!(store.get("acme").unwrap(), c);
    h.child("customers/acme/customer.toml")
        .assert(predicates::path::is_file());
    // The layer file exists so `rupu customer edit` has something to open,
    // and it parses as an empty config.
    let layer = std::fs::read_to_string(store.config_path("acme")).unwrap();
    let v: toml::Value = toml::from_str(&layer).unwrap();
    assert!(v.as_table().unwrap().is_empty());
}

#[test]
fn create_refuses_duplicates_bad_colors_and_empty_names() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    assert!(matches!(
        store.create("acme", &acme()),
        Err(CustomerError::Exists(_))
    ));
    let mut bad = acme();
    bad.color = Some("blue".into());
    assert!(matches!(
        store.create("globex", &bad),
        Err(CustomerError::InvalidColor(_))
    ));
    let mut empty = acme();
    empty.name = "  ".into();
    assert!(matches!(
        store.create("globex", &empty),
        Err(CustomerError::EmptyName)
    ));
}

#[test]
fn list_is_sorted_and_hides_archived_unless_asked() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("zeta", &acme()).unwrap();
    store.create("acme", &acme()).unwrap();
    store.set_archived("zeta", true).unwrap();
    let live: Vec<_> = store.list(false).unwrap().into_iter().map(|c| c.slug).collect();
    assert_eq!(live, vec!["acme"]);
    let all: Vec<_> = store.list(true).unwrap().into_iter().map(|c| c.slug).collect();
    assert_eq!(all, vec!["acme", "zeta"]);
}

#[test]
fn update_meta_patches_only_given_fields_and_empty_clears() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let c = store
        .update_meta(
            "acme",
            &MetaPatch {
                name: Some("ACME".into()),
                contact: Some(String::new()),
                ..MetaPatch::default()
            },
        )
        .unwrap();
    assert_eq!(c.meta.name, "ACME");
    assert_eq!(c.meta.contact, None, "empty string clears an optional field");
    assert_eq!(c.meta.color.as_deref(), Some("#3366cc"), "untouched");
}

#[test]
fn assign_registers_an_unknown_path_without_bumping_last_run() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store.assign("acme", ProjectRef::Path(project.path())).unwrap();
    assert!(ws.id.starts_with("ws_"));
    assert_eq!(ws.last_run_at, None, "assigning is not a run");
    assert_eq!(store.customer_of(&ws.id).unwrap().as_deref(), Some("acme"));
    let projects = store.projects_of("acme").unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, ws.id);
}

#[test]
fn assign_refuses_unknown_and_archived_customers() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    assert!(matches!(
        store.assign("nope", ProjectRef::Path(project.path())),
        Err(CustomerError::NotFound(_))
    ));
    store.create("acme", &acme()).unwrap();
    store.set_archived("acme", true).unwrap();
    assert!(matches!(
        store.assign("acme", ProjectRef::Path(project.path())),
        Err(CustomerError::Archived(_))
    ));
}

#[test]
fn assign_by_unknown_ws_id_is_an_error() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    assert!(matches!(
        store.assign("acme", ProjectRef::Id("ws_missing")),
        Err(CustomerError::NoProject(_))
    ));
}

#[test]
fn assignment_survives_an_upsert_after_it() {
    // `upsert` rewrites the whole workspace record on every run; the
    // assignment lives in a sidecar it never touches.
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store.assign("acme", ProjectRef::Path(project.path())).unwrap();
    let again = upsert(&ws_store(h.path()), project.path()).unwrap();
    assert_eq!(again.id, ws.id);
    assert_eq!(store.customer_of(&ws.id).unwrap().as_deref(), Some("acme"));
}

#[test]
fn unassign_removes_the_sidecar_and_is_idempotent() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store.assign("acme", ProjectRef::Path(project.path())).unwrap();
    store.unassign(ProjectRef::Id(&ws.id)).unwrap();
    assert_eq!(store.customer_of(&ws.id).unwrap(), None);
    store.unassign(ProjectRef::Id(&ws.id)).unwrap();
}

#[test]
fn delete_refuses_while_projects_are_assigned() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store.assign("acme", ProjectRef::Path(project.path())).unwrap();
    match store.delete("acme") {
        Err(CustomerError::HasProjects { slug, projects }) => {
            assert_eq!(slug, "acme");
            assert_eq!(projects, vec![ws.path.clone()]);
        }
        other => panic!("expected HasProjects, got {other:?}"),
    }
    store.unassign(ProjectRef::Id(&ws.id)).unwrap();
    store.delete("acme").unwrap();
    assert!(matches!(store.get("acme"), Err(CustomerError::NotFound(_))));
    h.child("customers/acme").assert(predicates::path::missing());
}

#[test]
fn lookup_walks_up_to_the_nearest_assigned_ancestor() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let sub = project.child("src/deep");
    sub.create_dir_all().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    store.assign("acme", ProjectRef::Path(project.path())).unwrap();
    // A run in a subdirectory has its own (unassigned) workspace record.
    upsert(&ws_store(h.path()), sub.path()).unwrap();

    assert_eq!(
        store.customer_for_dir(sub.path()).unwrap().as_deref(),
        Some("acme")
    );
    assert_eq!(
        store.customer_config_for_dir(sub.path()).unwrap(),
        Some(store.config_path("acme"))
    );
}

#[test]
fn lookup_without_any_assignment_is_none() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    assert_eq!(store.customer_config_for_dir(project.path()).unwrap(), None);
    upsert(&ws_store(h.path()), project.path()).unwrap();
    assert_eq!(store.customer_config_for_dir(project.path()).unwrap(), None);
}

#[test]
fn a_dangling_assignment_is_an_error_not_a_fallback() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    store.assign("acme", ProjectRef::Path(project.path())).unwrap();
    // The customer directory disappears behind the store's back.
    std::fs::remove_dir_all(h.path().join("customers/acme")).unwrap();
    match store.customer_config_for_dir(project.path()) {
        Err(CustomerError::Dangling { slug, .. }) => assert_eq!(slug, "acme"),
        other => panic!("expected Dangling, got {other:?}"),
    }
}
```

`rupu-workspace`'s dev-dependencies already include `assert_fs`, `predicates` and `tempfile`; add `toml.workspace = true` to `[dev-dependencies]` only if the test fails to resolve `toml` (it is already a normal dependency, which integration tests can use, so it should not be needed).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-workspace --test it customers::`
Expected: compile error — `CustomerStore`, `NewCustomer`, … not found in `rupu_workspace`.

- [ ] **Step 3: Add `find_by_path` and `register` to `store.rs`**

In `crates/rupu-workspace/src/store.rs`, replace the body of `upsert` from `let now = ...` through `store.write(&ws)?;` and add the two helpers above it:

```rust
/// Canonicalize `path` and return it with its UTF-8 string form — the key a
/// workspace record is matched on. Shared by [`upsert`], [`find_by_path`]
/// and [`register`] so the three can never disagree on what "the same
/// workspace" means.
fn canonical_key(path: &Path) -> Result<(PathBuf, String), StoreError> {
    let canonical = path.canonicalize().map_err(|e| StoreError::Io {
        action: format!("canonicalize {}", path.display()),
        source: e,
    })?;
    // Use to_str() to avoid display()'s lossy replacement chars on
    // non-UTF-8 paths. The path is the lookup key for "same workspace
    // already recorded" — a mangled path here would create a duplicate
    // record on every run.
    let s = canonical
        .to_str()
        .ok_or_else(|| StoreError::NonUtf8Path {
            path: canonical.display().to_string(),
        })?
        .to_string();
    Ok((canonical, s))
}

fn find_canonical(store: &WorkspaceStore, canonical: &Path) -> Result<Option<Workspace>, StoreError> {
    Ok(store.list()?.into_iter().find(|w| {
        Path::new(&w.path)
            .canonicalize()
            .map(|p| p == canonical)
            .unwrap_or(false)
    }))
}

/// The workspace recorded for `path`, if any. Never creates one.
pub fn find_by_path(store: &WorkspaceStore, path: &Path) -> Result<Option<Workspace>, StoreError> {
    let (canonical, _) = canonical_key(path)?;
    find_canonical(store, &canonical)
}

/// The workspace for `path`, registering it if absent. Unlike [`upsert`]
/// this is not a run: it leaves `last_run_at` alone (unset on a new
/// record). Used by customer assignment, which may name a project rupu has
/// never run in.
pub fn register(store: &WorkspaceStore, path: &Path) -> Result<Workspace, StoreError> {
    let (canonical, canonical_str) = canonical_key(path)?;
    if let Some(w) = find_canonical(store, &canonical)? {
        return Ok(w);
    }
    let ws = Workspace {
        id: new_id(),
        path: canonical_str,
        repo_remote: detect_repo_remote(&canonical),
        initial_branch: detect_initial_branch(&canonical),
        created_at: Utc::now().to_rfc3339(),
        last_run_at: None,
    };
    store.write(&ws)?;
    Ok(ws)
}
```

and make `upsert` use them:

```rust
pub fn upsert(store: &WorkspaceStore, path: &Path) -> Result<Workspace, StoreError> {
    let (canonical, canonical_str) = canonical_key(path)?;
    let now = Utc::now().to_rfc3339();
    let ws = match find_canonical(store, &canonical)? {
        Some(mut w) => {
            w.last_run_at = Some(now);
            w
        }
        None => Workspace {
            id: new_id(),
            path: canonical_str,
            repo_remote: detect_repo_remote(&canonical),
            initial_branch: detect_initial_branch(&canonical),
            created_at: now.clone(),
            last_run_at: Some(now),
        },
    };
    store.write(&ws)?;
    Ok(ws)
}
```

Also give `WorkspaceStore` a sidecar-path helper next to `record_path`, so the customers module never hard-codes the layout:

```rust
    /// `<root>/<id>.customer` — the customer-assignment sidecar
    /// (`crate::customers`). Not a `.toml` file, so [`Self::list`] skips it.
    pub(crate) fn customer_sidecar_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.customer"))
    }
```

- [ ] **Step 4: Write `customers.rs`**

Create `crates/rupu-workspace/src/customers.rs`:

```rust
//! Customers: a named group of projects with its own config layer.
//!
//! Spec: `docs/superpowers/specs/2026-10-06-rupu-customers-design.md`.
//! Layout under `RUPU_HOME`:
//!
//! ```text
//! customers/<slug>/customer.toml   metadata (CustomerMeta)
//! customers/<slug>/config.toml     the customer's config layer
//! workspaces/<ws_id>.customer      assignment sidecar: the slug
//! ```
//!
//! The assignment is a sidecar, not a `Workspace` field, because
//! [`crate::upsert`] rewrites the whole workspace record, unlocked, on every
//! run: a field would let a run that started while you assigned write back
//! its stale copy. `upsert` never touches the sidecar.

use crate::record::Workspace;
use crate::store::{find_by_path, register, StoreError, WorkspaceStore};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum CustomerError {
    #[error(
        "invalid customer slug `{0}`: use 1-63 characters of a-z, 0-9 and `-`, \
         starting with a letter or digit"
    )]
    InvalidSlug(String),
    #[error("invalid color `{0}`: use #rrggbb")]
    InvalidColor(String),
    #[error("customer name must not be empty")]
    EmptyName,
    #[error("customer `{0}` already exists")]
    Exists(String),
    #[error("no customer `{0}`")]
    NotFound(String),
    #[error("customer `{0}` is archived; unarchive it before assigning projects")]
    Archived(String),
    #[error("customer `{slug}` still has assigned projects: {}", .projects.join(", "))]
    HasProjects { slug: String, projects: Vec<String> },
    #[error(
        "project {project} is assigned to customer `{slug}`, which does not exist \
         (recreate the customer, or remove {sidecar})"
    )]
    Dangling {
        project: String,
        slug: String,
        sidecar: String,
    },
    #[error("no rupu project is registered as {0}")]
    NoProject(String),
    #[error("io {action}: {source}")]
    Io {
        action: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("serialize: {0}")]
    Ser(#[from] toml::ser::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// `customers/<slug>/customer.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// `#rrggbb`. `None` ⇒ consumers derive a tint from the slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub archived: bool,
    /// RFC 3339.
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Customer {
    pub slug: String,
    pub meta: CustomerMeta,
}

/// Input to [`CustomerStore::create`].
#[derive(Debug, Clone, Default)]
pub struct NewCustomer {
    pub name: String,
    pub notes: Option<String>,
    pub contact: Option<String>,
    pub color: Option<String>,
}

/// Input to [`CustomerStore::update_meta`]. `None` leaves a field alone;
/// `Some("")` clears an optional field (`notes`, `contact`, `color`).
#[derive(Debug, Clone, Default)]
pub struct MetaPatch {
    pub name: Option<String>,
    pub notes: Option<String>,
    pub contact: Option<String>,
    pub color: Option<String>,
}

/// A project named by its directory or by its workspace id.
#[derive(Debug, Clone, Copy)]
pub enum ProjectRef<'a> {
    Path(&'a Path),
    Id(&'a str),
}

pub fn validate_slug(slug: &str) -> Result<(), CustomerError> {
    let mut chars = slug.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {
            slug.len() <= 63
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(CustomerError::InvalidSlug(slug.to_string()))
    }
}

fn validate_color(color: &str) -> Result<(), CustomerError> {
    let hex = color.strip_prefix('#').unwrap_or("");
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(CustomerError::InvalidColor(color.to_string()))
    }
}

fn validate_name(name: &str) -> Result<String, CustomerError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        Err(CustomerError::EmptyName)
    } else {
        Ok(trimmed.to_string())
    }
}

/// `Some("")` ⇒ cleared, `Some(v)` ⇒ set, `None` ⇒ unchanged.
fn patch_opt(slot: &mut Option<String>, value: &Option<String>) {
    if let Some(v) = value {
        *slot = if v.is_empty() { None } else { Some(v.clone()) };
    }
}

const LAYER_HEADER: &str = "# Customer config layer. Same schema as ~/.rupu/config.toml.\n\
# Precedence: global lock > customer lock ([policy].lock here) > project > customer > global.\n";

#[derive(Debug, Clone)]
pub struct CustomerStore {
    pub home: PathBuf,
}

impl CustomerStore {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    fn workspaces(&self) -> WorkspaceStore {
        WorkspaceStore {
            root: self.home.join("workspaces"),
        }
    }

    fn customers_dir(&self) -> PathBuf {
        self.home.join("customers")
    }

    fn dir(&self, slug: &str) -> PathBuf {
        self.customers_dir().join(slug)
    }

    fn meta_path(&self, slug: &str) -> PathBuf {
        self.dir(slug).join("customer.toml")
    }

    /// `customers/<slug>/config.toml` — the customer's config layer.
    pub fn config_path(&self, slug: &str) -> PathBuf {
        self.dir(slug).join("config.toml")
    }

    fn exists(&self, slug: &str) -> bool {
        self.meta_path(slug).is_file()
    }

    fn read_meta(&self, slug: &str) -> Result<CustomerMeta, CustomerError> {
        let path = self.meta_path(slug);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CustomerError::NotFound(slug.to_string()))
            }
            Err(e) => {
                return Err(CustomerError::Io {
                    action: format!("read {}", path.display()),
                    source: e,
                })
            }
        };
        toml::from_str(&text).map_err(|e| CustomerError::Parse {
            path: path.display().to_string(),
            source: e,
        })
    }

    fn write_meta(&self, slug: &str, meta: &CustomerMeta) -> Result<(), CustomerError> {
        write_atomic(&self.meta_path(slug), &toml::to_string(meta)?)
    }

    pub fn list(&self, include_archived: bool) -> Result<Vec<Customer>, CustomerError> {
        let dir = self.customers_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => {
                return Err(CustomerError::Io {
                    action: format!("read_dir {}", dir.display()),
                    source: e,
                })
            }
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| CustomerError::Io {
                action: "read_dir entry".into(),
                source: e,
            })?;
            let Some(slug) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if validate_slug(&slug).is_err() || !self.exists(&slug) {
                continue;
            }
            match self.read_meta(&slug) {
                Ok(meta) if include_archived || !meta.archived => {
                    out.push(Customer { slug, meta });
                }
                Ok(_) => {}
                Err(e) => warn!(slug = %slug, error = %e, "skipping unreadable customer"),
            }
        }
        out.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(out)
    }

    pub fn get(&self, slug: &str) -> Result<Customer, CustomerError> {
        validate_slug(slug)?;
        Ok(Customer {
            slug: slug.to_string(),
            meta: self.read_meta(slug)?,
        })
    }

    pub fn create(&self, slug: &str, new: &NewCustomer) -> Result<Customer, CustomerError> {
        validate_slug(slug)?;
        let name = validate_name(&new.name)?;
        if let Some(c) = &new.color {
            validate_color(c)?;
        }
        if self.exists(slug) {
            return Err(CustomerError::Exists(slug.to_string()));
        }
        let meta = CustomerMeta {
            name,
            notes: new.notes.clone().filter(|s| !s.is_empty()),
            contact: new.contact.clone().filter(|s| !s.is_empty()),
            color: new.color.clone(),
            archived: false,
            created_at: Utc::now().to_rfc3339(),
        };
        let dir = self.dir(slug);
        std::fs::create_dir_all(&dir).map_err(|e| CustomerError::Io {
            action: format!("create_dir_all {}", dir.display()),
            source: e,
        })?;
        if !self.config_path(slug).exists() {
            write_atomic(&self.config_path(slug), LAYER_HEADER)?;
        }
        self.write_meta(slug, &meta)?;
        Ok(Customer {
            slug: slug.to_string(),
            meta,
        })
    }

    pub fn update_meta(&self, slug: &str, patch: &MetaPatch) -> Result<Customer, CustomerError> {
        let mut c = self.get(slug)?;
        if let Some(n) = &patch.name {
            c.meta.name = validate_name(n)?;
        }
        if let Some(col) = patch.color.as_deref().filter(|s| !s.is_empty()) {
            validate_color(col)?;
        }
        patch_opt(&mut c.meta.notes, &patch.notes);
        patch_opt(&mut c.meta.contact, &patch.contact);
        patch_opt(&mut c.meta.color, &patch.color);
        self.write_meta(slug, &c.meta)?;
        Ok(c)
    }

    pub fn set_archived(&self, slug: &str, archived: bool) -> Result<Customer, CustomerError> {
        let mut c = self.get(slug)?;
        c.meta.archived = archived;
        self.write_meta(slug, &c.meta)?;
        Ok(c)
    }

    pub fn delete(&self, slug: &str) -> Result<(), CustomerError> {
        self.get(slug)?;
        let projects = self.projects_of(slug)?;
        if !projects.is_empty() {
            return Err(CustomerError::HasProjects {
                slug: slug.to_string(),
                projects: projects.into_iter().map(|w| w.path).collect(),
            });
        }
        let dir = self.dir(slug);
        std::fs::remove_dir_all(&dir).map_err(|e| CustomerError::Io {
            action: format!("remove_dir_all {}", dir.display()),
            source: e,
        })
    }

    fn resolve_project(
        &self,
        project: ProjectRef<'_>,
        register_missing: bool,
    ) -> Result<Workspace, CustomerError> {
        let store = self.workspaces();
        match project {
            ProjectRef::Id(id) => store
                .load(id)?
                .ok_or_else(|| CustomerError::NoProject(id.to_string())),
            ProjectRef::Path(p) if register_missing => Ok(register(&store, p)?),
            ProjectRef::Path(p) => find_by_path(&store, p)?
                .ok_or_else(|| CustomerError::NoProject(p.display().to_string())),
        }
    }

    pub fn assign(&self, slug: &str, project: ProjectRef<'_>) -> Result<Workspace, CustomerError> {
        let c = self.get(slug)?;
        if c.meta.archived {
            return Err(CustomerError::Archived(slug.to_string()));
        }
        let ws = self.resolve_project(project, true)?;
        write_atomic(
            &self.workspaces().customer_sidecar_path(&ws.id),
            &format!("{slug}\n"),
        )?;
        Ok(ws)
    }

    /// Idempotent: unassigning a project with no customer succeeds.
    pub fn unassign(&self, project: ProjectRef<'_>) -> Result<Workspace, CustomerError> {
        let ws = self.resolve_project(project, false)?;
        let sidecar = self.workspaces().customer_sidecar_path(&ws.id);
        match std::fs::remove_file(&sidecar) {
            Ok(()) => Ok(ws),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ws),
            Err(e) => Err(CustomerError::Io {
                action: format!("remove {}", sidecar.display()),
                source: e,
            }),
        }
    }

    /// The slug in `ws_id`'s sidecar, as written — not checked against the
    /// customer directory (see [`Self::customer_config_for_dir`] for that).
    pub fn customer_of(&self, ws_id: &str) -> Result<Option<String>, CustomerError> {
        let path = self.workspaces().customer_sidecar_path(ws_id);
        match std::fs::read_to_string(&path) {
            Ok(s) => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CustomerError::Io {
                action: format!("read {}", path.display()),
                source: e,
            }),
        }
    }

    pub fn projects_of(&self, slug: &str) -> Result<Vec<Workspace>, CustomerError> {
        let mut out = Vec::new();
        for ws in self.workspaces().list()? {
            if self.customer_of(&ws.id)?.as_deref() == Some(slug) {
                out.push(ws);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// The customer that owns runs in `dir`: the nearest ancestor of `dir`
    /// (inclusive) whose workspace record has an assignment. Workspace
    /// records are keyed by the directory rupu ran in, so a run from a
    /// subdirectory has its own record; walking up lets it inherit its
    /// project's customer. Errors with [`CustomerError::Dangling`] when the
    /// nearest assignment names a customer that no longer exists — callers
    /// on launch paths must fail the run, never fall back to global config.
    pub fn customer_for_dir(&self, dir: &Path) -> Result<Option<String>, CustomerError> {
        let canonical = match dir.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                return Err(CustomerError::Io {
                    action: format!("canonicalize {}", dir.display()),
                    source: e,
                })
            }
        };
        // One pass over the records, keyed by canonical path.
        let mut by_path: BTreeMap<PathBuf, String> = BTreeMap::new();
        for ws in self.workspaces().list()? {
            if let Ok(p) = Path::new(&ws.path).canonicalize() {
                by_path.insert(p, ws.id);
            }
        }
        for ancestor in canonical.ancestors() {
            let Some(id) = by_path.get(ancestor) else {
                continue;
            };
            let Some(slug) = self.customer_of(id)? else {
                continue;
            };
            if validate_slug(&slug).is_err() || !self.exists(&slug) {
                return Err(CustomerError::Dangling {
                    project: ancestor.display().to_string(),
                    slug,
                    sidecar: self
                        .workspaces()
                        .customer_sidecar_path(id)
                        .display()
                        .to_string(),
                });
            }
            return Ok(Some(slug));
        }
        Ok(None)
    }

    /// [`Self::customer_for_dir`], as the path of that customer's layer.
    pub fn customer_config_for_dir(&self, dir: &Path) -> Result<Option<PathBuf>, CustomerError> {
        Ok(self
            .customer_for_dir(dir)?
            .map(|slug| self.config_path(&slug)))
    }
}

/// Temp file + rename, so readers never see a partial file.
fn write_atomic(path: &Path, body: &str) -> Result<(), CustomerError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CustomerError::Io {
            action: format!("create_dir_all {}", parent.display()),
            source: e,
        })?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).map_err(|e| CustomerError::Io {
        action: format!("write {}", tmp.display()),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| CustomerError::Io {
        action: format!("rename {} -> {}", tmp.display(), path.display()),
        source: e,
    })
}
```

Note `path.with_extension("tmp")` on `ws_x.customer` gives `ws_x.tmp`, and on `config.toml` gives `config.tmp` — neither ends in `.toml`, so `WorkspaceStore::list` never picks up a half-written file.

- [ ] **Step 5: Export it**

In `crates/rupu-workspace/src/lib.rs`, add `pub mod customers;` (alphabetically after `autoflow_worktree`) and:

```rust
pub use customers::{
    validate_slug, Customer, CustomerError, CustomerMeta, CustomerStore, MetaPatch, NewCustomer,
    ProjectRef,
};
pub use store::{find_by_path, register, upsert, StoreError, WorkspaceStore};
```

(replacing the existing `pub use store::{upsert, StoreError, WorkspaceStore};`).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rupu-workspace --test it`
Expected: all pass, including the existing `upsert::` and `record::` modules (the `upsert` refactor must not change their behaviour).

- [ ] **Step 7: Lint and commit**

```bash
rustfmt --edition 2021 crates/rupu-workspace/src/customers.rs crates/rupu-workspace/src/store.rs crates/rupu-workspace/src/lib.rs crates/rupu-workspace/tests/it/customers.rs crates/rupu-workspace/tests/it/main.rs
cargo clippy -p rupu-workspace --all-targets -- -D warnings
git add crates/rupu-workspace
git commit -m "feat(workspace): customer store — metadata, assignment sidecars, run-dir lookup"
```

---

### Task 2: Three-layer config resolution in `rupu-config`

**Files:**
- Modify: `crates/rupu-config/src/layer.rs`
- Modify: `crates/rupu-config/src/resolve.rs`
- Modify: `crates/rupu-config/src/lib.rs`
- Test: `crates/rupu-config/tests/it/customer_layer.rs`, `crates/rupu-config/tests/it/main.rs`, existing in-file tests in `layer.rs` / `resolve.rs`, `crates/rupu-config/tests/it/layering.rs`

This task changes the signatures of `layer_files`, `layer_files_locked` and `resolve`. Callers outside `rupu-config` stop compiling until Task 3 converts them; that is intended (the compiler is the inventory). Within this task, build and test only `rupu-config`.

**Interfaces:**
- Produces:
  - `rupu_config::LayerPaths<'a> { pub global: Option<&'a Path>, pub customer: Option<&'a Path>, pub project: Option<&'a Path> }` (`Debug, Clone, Copy, Default`) with `fn new(global, customer, project) -> Self` and `fn global_only(global: &'a Path) -> Self`.
  - `pub fn layer_files(paths: LayerPaths<'_>) -> Result<Config, LayerError>`
  - `pub fn layer_files_locked(paths: LayerPaths<'_>) -> Result<Config, LayerError>`
  - `pub fn resolve(paths: LayerPaths<'_>) -> Result<Resolved, LayerError>`
  - `KeySource::{Global, Customer, Project, Default}` (serialized lowercase: `"customer"`).
  - `rupu_config::LockOwner::{Global, Customer}` (serialized lowercase).
  - `KeyProvenance { source, locked: bool, locked_by: Option<LockOwner> }` — `locked == locked_by.is_some()`; `locked_by` is skipped in JSON when `None`.
  - `Resolved { config, provenance, customer_lock: Vec<String>, warnings: Vec<String> }`.
  - `LayerError::Layered { global_path, customer_path, project_path, source }`.

- [ ] **Step 1: Write the failing tests**

Add `mod customer_layer;` to `crates/rupu-config/tests/it/main.rs`. Create `crates/rupu-config/tests/it/customer_layer.rs`:

```rust
use rupu_config::{
    layer_files, layer_files_locked, resolve, KeySource, LayerPaths, LockOwner,
};
use std::path::{Path, PathBuf};

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

struct Layers {
    _dir: tempfile::TempDir,
    g: PathBuf,
    c: PathBuf,
    p: PathBuf,
}

fn layers(g: &str, c: &str, p: &str) -> Layers {
    let dir = tempfile::tempdir().unwrap();
    Layers {
        g: write(dir.path(), "g.toml", g),
        c: write(dir.path(), "c.toml", c),
        p: write(dir.path(), "p.toml", p),
        _dir: dir,
    }
}

impl Layers {
    fn paths(&self) -> LayerPaths<'_> {
        LayerPaths::new(Some(&self.g), Some(&self.c), Some(&self.p))
    }
}

#[test]
fn customer_beats_global_when_project_is_silent() {
    let l = layers("default_provider = \"anthropic\"\n", "default_provider = \"anthropic-acme\"\n", "");
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.default_provider.as_deref(), Some("anthropic-acme"));
    let prov = &r.provenance["default_provider"];
    assert_eq!(prov.source, KeySource::Customer);
    assert!(!prov.locked);
    assert_eq!(prov.locked_by, None);
}

#[test]
fn project_beats_an_unlocked_customer_key() {
    let l = layers("", "default_model = \"c\"\n", "default_model = \"p\"\n");
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.default_model.as_deref(), Some("p"));
    assert_eq!(r.provenance["default_model"].source, KeySource::Project);
}

#[test]
fn a_customer_lock_beats_the_project() {
    let l = layers(
        "",
        "default_provider = \"anthropic-acme\"\n[policy]\nlock = [\"default_provider\"]\n",
        "default_provider = \"anthropic\"\n",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.default_provider.as_deref(), Some("anthropic-acme"));
    let prov = &r.provenance["default_provider"];
    assert_eq!(prov.source, KeySource::Customer);
    assert!(prov.locked);
    assert_eq!(prov.locked_by, Some(LockOwner::Customer));
    assert_eq!(r.customer_lock, vec!["default_provider".to_string()]);
    assert!(r.warnings.is_empty());
}

#[test]
fn a_global_lock_beats_a_customer_lock() {
    let l = layers(
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"bypass\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"ask\"\n",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.permission_mode.as_deref(), Some("readonly"));
    let prov = &r.provenance["permission_mode"];
    assert_eq!(prov.source, KeySource::Global);
    assert_eq!(prov.locked_by, Some(LockOwner::Global));
}

#[test]
fn the_resolved_lock_list_stays_global_only() {
    let l = layers(
        "[policy]\nlock = [\"log_level\"]\n",
        "default_model = \"c\"\n[policy]\nlock = [\"default_model\"]\n",
        "[policy]\nlock = []\n",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.policy.lock, vec!["log_level".to_string()]);
    assert_eq!(r.customer_lock, vec!["default_model".to_string()]);
}

#[test]
fn a_customer_lock_on_a_key_it_does_not_set_warns_and_locks_nothing() {
    let l = layers("", "[policy]\nlock = [\"default_model\"]\n", "default_model = \"p\"\n");
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.default_model.as_deref(), Some("p"));
    assert_eq!(r.provenance["default_model"].source, KeySource::Project);
    assert_eq!(r.provenance["default_model"].locked_by, None);
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert!(r.warnings[0].contains("default_model"));
}

#[test]
fn arrays_replace_across_three_layers() {
    let l = layers(
        "[[scm.rules]]\nowner = \"me\"\naccount = \"github\"\n",
        "[[scm.rules]]\nowner = \"acme-corp\"\naccount = \"github-acme\"\n",
        "",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.scm.rules.len(), 1);
    assert_eq!(r.config.scm.rules[0].account, "github-acme");
}

#[test]
fn layer_files_merges_three_layers_in_order_and_pins_the_global_lock() {
    let l = layers(
        "default_model = \"g\"\nlog_level = \"info\"\n[policy]\nlock = [\"log_level\"]\n",
        "default_model = \"c\"\ndefault_provider = \"anthropic-acme\"\n[policy]\nlock = [\"default_model\"]\n",
        "default_model = \"p\"\n",
    );
    let cfg = layer_files(l.paths()).unwrap();
    assert_eq!(cfg.default_model.as_deref(), Some("p"));
    assert_eq!(cfg.default_provider.as_deref(), Some("anthropic-acme"));
    assert_eq!(cfg.log_level.as_deref(), Some("info"));
    // Plain layering never trusts a lower layer's lock list.
    assert_eq!(cfg.policy.lock, vec!["log_level".to_string()]);
}

#[test]
fn layer_files_locked_honours_the_customer_lock() {
    let l = layers(
        "",
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"bypass\"\n",
    );
    let cfg = layer_files_locked(l.paths()).unwrap();
    assert_eq!(cfg.permission_mode.as_deref(), Some("readonly"));
}

#[test]
fn a_missing_customer_file_is_an_empty_layer() {
    let dir = tempfile::tempdir().unwrap();
    let g = write(dir.path(), "g.toml", "default_model = \"g\"\n");
    let missing = dir.path().join("nope.toml");
    let cfg = layer_files(LayerPaths::new(Some(&g), Some(&missing), None)).unwrap();
    assert_eq!(cfg.default_model.as_deref(), Some("g"));
}

#[test]
fn a_malformed_customer_layer_is_an_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let c = write(dir.path(), "c.toml", "default_model = \n");
    let err = layer_files_locked(LayerPaths::new(None, Some(&c), None)).unwrap_err();
    assert!(err.to_string().contains("c.toml"), "{err}");
}

#[test]
fn provenance_serializes_customer_source_and_lock_owner() {
    let l = layers(
        "",
        "default_model = \"c\"\n[policy]\nlock = [\"default_model\"]\n",
        "",
    );
    let r = resolve(l.paths()).unwrap();
    let json = serde_json::to_value(&r.provenance["default_model"]).unwrap();
    assert_eq!(json["source"], "customer");
    assert_eq!(json["locked"], true);
    assert_eq!(json["locked_by"], "customer");
}

/// The dotted-key contract: a customer lock on a key whose segment holds a
/// `.` matches the canonical quoted encoding `resolve` uses for provenance.
#[test]
fn a_customer_lock_on_a_dotted_pricing_key_uses_the_canonical_encoding() {
    let l = layers(
        "",
        "[pricing.oracle.\"GLM-5.2-FP8\"]\ninput_per_mtok = 1.0\noutput_per_mtok = 2.0\n\
         [policy]\nlock = ['pricing.oracle.\"GLM-5.2-FP8\".input_per_mtok']\n",
        "[pricing.oracle.\"GLM-5.2-FP8\"]\ninput_per_mtok = 9.0\noutput_per_mtok = 9.0\n",
    );
    let r = resolve(l.paths()).unwrap();
    let locked = "pricing.oracle.\"GLM-5.2-FP8\".input_per_mtok";
    let free = "pricing.oracle.\"GLM-5.2-FP8\".output_per_mtok";
    assert_eq!(r.provenance[locked].source, KeySource::Customer);
    assert_eq!(r.provenance[locked].locked_by, Some(LockOwner::Customer));
    assert_eq!(r.provenance[free].source, KeySource::Project);
    let mp = r.config.pricing.models["oracle"]["GLM-5.2-FP8"];
    assert_eq!(mp.input_per_mtok, 1.0);
    assert_eq!(mp.output_per_mtok, 9.0);
}

#[test]
fn global_only_matches_the_old_single_file_load() {
    let dir = tempfile::tempdir().unwrap();
    let g = write(dir.path(), "g.toml", "default_model = \"g\"\n");
    let cfg = layer_files(LayerPaths::global_only(&g)).unwrap();
    assert_eq!(cfg.default_model.as_deref(), Some("g"));
}
```

If `serde_json` is not already a dev-dependency of `rupu-config`, add `serde_json.workspace = true` under `[dev-dependencies]` in `crates/rupu-config/Cargo.toml` (check first: `grep -n serde_json crates/rupu-config/Cargo.toml`).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-config --test it customer_layer::`
Expected: compile error — `LayerPaths`, `LockOwner` not found.

- [ ] **Step 3: `LayerPaths` and three-layer `layer_files` in `layer.rs`**

In `crates/rupu-config/src/layer.rs`:

Update the module doc's first lines to say *Global + customer + project config layering* and add one rule line: `- Layer order (lowest first): global, customer, project. Only the global layer's [policy].lock survives into the merged config.`

Add `customer_path` to `LayerError::Layered`:

```rust
    #[error(
        "layered config invalid (merging {global_path:?} + {customer_path:?} + {project_path:?}): {source}"
    )]
    Layered {
        global_path: Option<String>,
        customer_path: Option<String>,
        project_path: Option<String>,
        #[source]
        source: Box<toml::de::Error>,
    },
```

Add, above `layer_files`:

```rust
/// The config files one load layers, lowest first: global, customer,
/// project. Any may be `None` (or name a missing file) — that layer is
/// empty. Spec: `docs/superpowers/specs/2026-10-06-rupu-customers-design.md`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LayerPaths<'a> {
    pub global: Option<&'a Path>,
    pub customer: Option<&'a Path>,
    pub project: Option<&'a Path>,
}

impl<'a> LayerPaths<'a> {
    pub fn new(
        global: Option<&'a Path>,
        customer: Option<&'a Path>,
        project: Option<&'a Path>,
    ) -> Self {
        Self {
            global,
            customer,
            project,
        }
    }

    /// Just the global file — for loads that serve no project by design.
    pub fn global_only(global: &'a Path) -> Self {
        Self {
            global: Some(global),
            ..Self::default()
        }
    }

    pub(crate) fn layered_error(&self, source: toml::de::Error) -> LayerError {
        let show = |p: Option<&Path>| p.map(|p| p.display().to_string());
        LayerError::Layered {
            global_path: show(self.global),
            customer_path: show(self.customer),
            project_path: show(self.project),
            source: Box::new(source),
        }
    }
}

/// `[policy].lock` of one raw layer, as written.
pub(crate) fn policy_lock_of(layer: Option<&Value>) -> Vec<String> {
    layer
        .and_then(|v| v.get("policy"))
        .and_then(|p| p.get("lock"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}
```

Replace `layer_files` and `layer_files_locked`:

```rust
/// Layer global, customer and project config files into a single
/// [`Config`]: plain layering, no lock enforcement — correct for non-policy
/// reads only (see [`layer_files_locked`]).
///
/// Merge semantics:
///
/// - **Tables** merge key-by-key recursively.
/// - **Arrays** in a higher layer REPLACE arrays below it — they never
///   concatenate, so a layer can subtract an entry from a lower allow-list.
/// - **Scalars** in a higher layer overwrite lower ones.
/// - `policy.lock` is pinned to the GLOBAL layer's list: a customer's or
///   project's lock list never appears in the merged config.
pub fn layer_files(paths: LayerPaths<'_>) -> Result<Config, LayerError> {
    let g = read_optional_toml(paths.global)?;
    let c = read_optional_toml(paths.customer)?;
    let p = read_optional_toml(paths.project)?;
    let global_lock = policy_lock_of(g.as_ref());

    let merged = [g, c, p]
        .into_iter()
        .flatten()
        .reduce(deep_merge)
        .unwrap_or_else(|| Value::Table(toml::value::Table::new()));

    let mut cfg: Config = merged
        .try_into()
        .map_err(|source| paths.layered_error(source))?;
    cfg.policy.lock = global_lock;
    cfg.attach_provider_kinds();
    cfg.validate()?;
    Ok(cfg)
}

/// Like [`layer_files`], but honouring `[policy].lock`: a key the GLOBAL
/// lock names keeps its global value, and a key the CUSTOMER lock names
/// keeps its customer value against the project.
///
/// Every path that honors operator policy must use this — see ISSUES.md
/// I-7. This delegates entirely to `resolve()` for lock precedence and
/// dotted-key handling rather than reimplementing them, and logs
/// `resolve`'s warnings (e.g. a customer lock naming a key the customer
/// does not set).
pub fn layer_files_locked(paths: LayerPaths<'_>) -> Result<Config, LayerError> {
    let resolved = crate::resolve::resolve(paths)?;
    for w in &resolved.warnings {
        tracing::warn!("{w}");
    }
    Ok(resolved.config)
}
```

Update the in-file test `layer_files_attaches_provider_account_kinds_to_pricing` to call `layer_files(LayerPaths::global_only(&g))`.

- [ ] **Step 4: Three-layer `resolve` in `resolve.rs`**

Update the module doc: precedence is now *global lock > customer lock > project > customer > global > default*, and locks come from the global and customer layers only.

Replace `KeySource`, `KeyProvenance`, `Resolved`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    Global,
    Customer,
    Project,
    Default,
}

/// Which layer's `[policy].lock` names a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockOwner {
    Global,
    Customer,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyProvenance {
    pub source: KeySource,
    /// `locked_by.is_some()` — kept so existing consumers (the CP) read it
    /// unchanged.
    pub locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_by: Option<LockOwner>,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub config: Config,
    pub provenance: BTreeMap<String, KeyProvenance>,
    /// The customer layer's `[policy].lock`, as written. Not merged into
    /// `config.policy.lock`, which stays the global list.
    pub customer_lock: Vec<String>,
    /// Non-fatal problems worth telling the operator about.
    pub warnings: Vec<String>,
}
```

Replace the `resolve` function (keep `flatten`, `unflatten`, `dotted` unchanged):

```rust
pub fn resolve(paths: LayerPaths<'_>) -> Result<Resolved, LayerError> {
    let g = read_optional_toml(paths.global)?;
    let c = read_optional_toml(paths.customer)?;
    let p = read_optional_toml(paths.project)?;

    let flat = |v: &Option<Value>| {
        let mut out: BTreeMap<Vec<String>, Value> = BTreeMap::new();
        if let Some(v) = v {
            flatten(&[], v, &mut out);
        }
        out
    };
    let (fg, fc, fp) = (flat(&g), flat(&c), flat(&p));

    // Locks come from the GLOBAL and CUSTOMER layers only; a project's
    // `[policy].lock` is ignored, as it always was.
    let global_lock = policy_lock_of(g.as_ref());
    let customer_lock = policy_lock_of(c.as_ref());
    let g_locked = |key: &str| global_lock.iter().any(|l| l == key);
    let c_locked = |key: &str| customer_lock.iter().any(|l| l == key);

    let mut warnings = Vec::new();
    for l in &customer_lock {
        let set_by_customer = fc.keys().any(|k| &dotted(k) == l);
        if !set_by_customer && !g_locked(l) {
            warnings.push(format!(
                "customer [policy].lock names `{l}`, which the customer layer does not set; \
                 the lock has no effect"
            ));
        }
    }

    let mut winners: BTreeMap<Vec<String>, Value> = BTreeMap::new();
    let mut provenance: BTreeMap<String, KeyProvenance> = BTreeMap::new();
    let all_keys: std::collections::BTreeSet<Vec<String>> = fg
        .keys()
        .chain(fc.keys())
        .chain(fp.keys())
        .cloned()
        .collect();

    for key in all_keys {
        let key_dotted = dotted(&key);
        // A customer lock only counts when the customer layer sets the key
        // (a lock on a key it does not set "locks nothing" and is reported
        // in `warnings`). A global lock is reported as before: listed ⇒ locked.
        let locked_by = if g_locked(&key_dotted) {
            Some(LockOwner::Global)
        } else if c_locked(&key_dotted) && fc.contains_key(&key) {
            Some(LockOwner::Customer)
        } else {
            None
        };
        // A lock pins its owner's value when that layer sets the key;
        // otherwise the key falls through to ordinary precedence
        // (project > customer > global), exactly as a global lock on a key
        // only the project set always did.
        let pick = if g_locked(&key_dotted) && fg.contains_key(&key) {
            Some((&fg, KeySource::Global))
        } else if c_locked(&key_dotted) && fc.contains_key(&key) {
            Some((&fc, KeySource::Customer))
        } else if fp.contains_key(&key) {
            Some((&fp, KeySource::Project))
        } else if fc.contains_key(&key) {
            Some((&fc, KeySource::Customer))
        } else if fg.contains_key(&key) {
            Some((&fg, KeySource::Global))
        } else {
            None
        };
        if let Some((layer, source)) = pick {
            winners.insert(key.clone(), layer[&key].clone());
            provenance.insert(
                key_dotted,
                KeyProvenance {
                    source,
                    locked: locked_by.is_some(),
                    locked_by,
                },
            );
        }
    }

    let merged = unflatten(&winners)?;
    let mut config: Config = merged
        .try_into()
        .map_err(|source| paths.layered_error(source))?;

    // The `policy.lock` list is itself an unlocked key, so a lower layer's
    // `[policy].lock` would otherwise land in the resolved config and
    // mislead consumers (the CP reading `config.policy.lock` for lock
    // badges, or a project appearing to clear locks). Pin it to the GLOBAL
    // list; the customer's list is reported separately in `customer_lock`.
    config.policy.lock = global_lock.clone();
    let policy_lock_key = vec!["policy".to_string(), "lock".to_string()];
    if winners.contains_key(&policy_lock_key) || !global_lock.is_empty() {
        let locked_by = g_locked("policy.lock").then_some(LockOwner::Global);
        provenance.insert(
            "policy.lock".to_string(),
            KeyProvenance {
                source: KeySource::Global,
                locked: locked_by.is_some(),
                locked_by,
            },
        );
    }

    config.attach_provider_kinds();
    config.validate()?;
    Ok(Resolved {
        config,
        provenance,
        customer_lock,
        warnings,
    })
}
```

Change the imports at the top of `resolve.rs` to `use crate::layer::{policy_lock_of, read_optional_toml, LayerError, LayerPaths};`.

Update every in-file test in `resolve.rs` from `resolve(Some(&g), Some(&p))` to `resolve(LayerPaths::new(Some(&g), None, Some(&p)))` and `resolve(Some(&g), None)` to `resolve(LayerPaths::global_only(&g))` (find them with `grep -n "resolve(Some\|resolve(None" crates/rupu-config/src/resolve.rs`). Do the same in `crates/rupu-config/tests/it/layering.rs` and any other `crates/rupu-config/tests/it/*.rs` that call the three loaders (`grep -rn "layer_files\|resolve(" crates/rupu-config/tests`).

- [ ] **Step 5: Export the new names**

In `crates/rupu-config/src/lib.rs`: update the crate doc's first paragraph to *Three-tier configuration: global, customer (`~/.rupu/customers/<slug>/config.toml`), project*, and change the re-exports to:

```rust
pub use layer::{layer_files, layer_files_locked, LayerError, LayerPaths};
pub use resolve::{resolve, KeyProvenance, KeySource, LockOwner, Resolved};
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rupu-config`
Expected: all pass — the new `customer_layer::` module, the updated `layering::` module, and the in-file `layer`/`resolve` tests (the existing two-layer expectations — e.g. `locked_key_global_overrides_project`, `unlocked_key_project_overrides_global` — must hold unchanged with `customer: None`).

- [ ] **Step 7: Lint and commit**

```bash
rustfmt --edition 2021 crates/rupu-config/src/layer.rs crates/rupu-config/src/resolve.rs crates/rupu-config/src/lib.rs crates/rupu-config/tests/it/customer_layer.rs crates/rupu-config/tests/it/main.rs crates/rupu-config/tests/it/layering.rs
cargo clippy -p rupu-config --all-targets -- -D warnings
git add crates/rupu-config
git commit -m "feat(config): customer layer — LayerPaths, customer locks, Customer provenance"
```

The workspace as a whole does not build at this commit (Task 3 converts the callers); do not push between Task 2 and Task 3.

---

### Task 3: Every config load goes through the customer layer

**Files:**
- Modify: `crates/rupu-cli/src/paths.rs` (add `ConfigPaths`, `config_paths`, `config_paths_for_display`)
- Modify: every call site in the table below (`crates/rupu-cli/src/**`, `crates/rupu-cp/src/**`)
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (`KeySource`, `locked_by`)
- Modify (tests): `crates/rupu-cli/tests/serial/policy_lock.rs`, `crates/rupu-cp/src/api/config.rs` test module
- Test: `crates/rupu-cli/src/paths.rs` (unit tests), `crates/rupu-cp/src/api/config.rs` (new test)

After Task 2 the workspace does not compile: every caller of `layer_files`, `layer_files_locked` and `resolve` outside `rupu-config` still passes two paths. This task converts all of them. The compiler is the checklist — `cargo check --workspace --all-targets` must end clean — and the table below says, per site, **which** customer applies and whether a lookup failure is fatal. Do not convert a project-serving site to `LayerPaths::global_only` or to `customer: None` to make it compile; that is exactly the silent skip this design exists to prevent.

**Interfaces:**
- Consumes: `rupu_config::LayerPaths` (Task 2); `rupu_workspace::CustomerStore::customer_config_for_dir` (Task 1).
- Produces (used by Tasks 4–5):
  - `crate::paths::ConfigPaths { pub global: PathBuf, pub customer: Option<PathBuf>, pub project: Option<PathBuf> }` with `fn layers(&self) -> rupu_config::LayerPaths<'_>`.
  - `crate::paths::config_paths(global: &Path, project_root: Option<&Path>, run_dir: &Path) -> anyhow::Result<ConfigPaths>` — **strict**.
  - `crate::paths::config_paths_for_display(global: &Path, project_root: Option<&Path>, run_dir: &Path) -> ConfigPaths` — **lenient**.

The customer is looked up from `project_root` when there is one, else from `run_dir` (the directory the run or command works in), walking up to the nearest assigned ancestor (`customer_config_for_dir`). One rule for every site keeps two loads of the same run from disagreeing — e.g. `workflow run`'s first load (#4) and its per-invocation load (#5), which both see the same `project_root`.

- [ ] **Step 1: Write the failing helper tests**

Append to `crates/rupu-cli/src/paths.rs` (inside the existing `#[cfg(test)] mod tests` if there is one — `grep -n "cfg(test)" crates/rupu-cli/src/paths.rs` — otherwise add one):

```rust
#[cfg(test)]
mod customer_layer_tests {
    use super::*;
    use rupu_workspace::{CustomerStore, NewCustomer, ProjectRef};

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(project.join(".rupu")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        (tmp, home, project)
    }

    fn assign(home: &Path, project: &Path) {
        let store = CustomerStore::new(home);
        store
            .create("acme", &NewCustomer { name: "Acme".into(), ..NewCustomer::default() })
            .unwrap();
        store.assign("acme", ProjectRef::Path(project)).unwrap();
    }

    #[test]
    fn config_paths_includes_the_customer_of_the_project_root() {
        let (_t, home, project) = setup();
        assign(&home, &project);
        let p = config_paths(&home, Some(&project), Path::new("/")).unwrap();
        assert_eq!(p.global, home.join("config.toml"));
        assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
        assert_eq!(p.project, Some(project.join(".rupu/config.toml")));
    }

    #[test]
    fn config_paths_falls_back_to_the_run_dir_without_a_project_root() {
        let (_t, home, project) = setup();
        assign(&home, &project);
        let p = config_paths(&home, None, &project).unwrap();
        assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
        assert_eq!(p.project, None);
    }

    #[test]
    fn strict_errors_and_display_degrades_on_a_dangling_assignment() {
        let (_t, home, project) = setup();
        assign(&home, &project);
        std::fs::remove_dir_all(home.join("customers/acme")).unwrap();
        assert!(config_paths(&home, Some(&project), &project).is_err());
        let p = config_paths_for_display(&home, Some(&project), &project);
        assert_eq!(p.customer, None);
        assert_eq!(p.project, Some(project.join(".rupu/config.toml")));
    }
}
```

Check `tempfile` is a dev-dependency of `rupu-cli` (`grep -n tempfile crates/rupu-cli/Cargo.toml`); it is used by existing unit tests, so it should be.

- [ ] **Step 2: Write the helpers**

Add to `crates/rupu-cli/src/paths.rs`:

```rust
/// The config files one load layers: global, the customer of the project
/// (if assigned), and the project's `.rupu/config.toml`. Spec:
/// `docs/superpowers/specs/2026-10-06-rupu-customers-design.md` §2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub global: PathBuf,
    pub customer: Option<PathBuf>,
    pub project: Option<PathBuf>,
}

impl ConfigPaths {
    pub fn layers(&self) -> rupu_config::LayerPaths<'_> {
        rupu_config::LayerPaths::new(
            Some(&self.global),
            self.customer.as_deref(),
            self.project.as_deref(),
        )
    }

    fn without_customer(global: &Path, project_root: Option<&Path>) -> Self {
        Self {
            global: global.join("config.toml"),
            customer: None,
            project: project_root.map(|p| p.join(".rupu/config.toml")),
        }
    }
}

/// Layer paths for a load that serves a project. The customer is the one
/// assigned to the nearest ancestor of `project_root` (else of `run_dir`,
/// the directory the command works in).
///
/// **Strict** — for launch paths and for anything whose result drives a
/// provider, SCM-account or permission decision: a project assigned to a
/// customer that no longer exists is an error, never a silent fall back to
/// the global config.
pub fn config_paths(
    global: &Path,
    project_root: Option<&Path>,
    run_dir: &Path,
) -> Result<ConfigPaths> {
    let dir = project_root.unwrap_or(run_dir);
    let customer = rupu_workspace::CustomerStore::new(global)
        .customer_config_for_dir(dir)
        .with_context(|| format!("resolve the customer of {}", dir.display()))?;
    Ok(ConfigPaths {
        customer,
        ..ConfigPaths::without_customer(global, project_root)
    })
}

/// [`config_paths`] for display-only reads (UI preferences, pricing tables,
/// listings): a failed customer lookup is logged and the customer layer
/// left out, so a broken assignment never stops `rupu transcript list`.
/// Never use this where the config picks a provider, account or permission.
pub fn config_paths_for_display(
    global: &Path,
    project_root: Option<&Path>,
    run_dir: &Path,
) -> ConfigPaths {
    config_paths(global, project_root, run_dir).unwrap_or_else(|e| {
        tracing::warn!(error = %format!("{e:#}"), "customer config layer skipped for this display");
        ConfigPaths::without_customer(global, project_root)
    })
}
```

- [ ] **Step 3: Convert the call sites**

Line numbers are as of `main` at `6253bda9`; locate each by its enclosing function. In the snippets, `global` is the `RUPU_HOME` dir (`paths::global_dir()?`), and `pwd` is `std::env::current_dir()?` (most sites already have one bound — reuse it). The general shapes:

```rust
// Strict (launch / decision):
let cfg_paths = paths::config_paths(&global, project_root.as_deref(), &pwd)?;
let cfg = rupu_config::layer_files_locked(cfg_paths.layers())?;

// Lenient (display):
let cfg_paths = paths::config_paths_for_display(&global, project_root.as_deref(), &pwd);
let cfg = rupu_config::layer_files(cfg_paths.layers()).unwrap_or_default();

// Global-only by design (keeps its existing error handling):
let cfg = rupu_config::layer_files_locked(rupu_config::LayerPaths::global_only(&global_cfg_path))…;
```

Keep each site's existing `layer_files` vs `layer_files_locked` choice and its existing error handling, except where the table says otherwise. Where a site already computed `project_cfg_path`/`global_cfg_path` locals only to pass them to the loader, delete those locals.

**Strict sites** (`config_paths`, error propagates):

| # | Site | `project_root` / `run_dir` |
|---|---|---|
| 1 | `cmd/run.rs` `run_inner` (~580) | `project_root` / `pwd` |
| 4 | `cmd/workflow.rs` `run_with_outcome` (~4643) | `project_root` / `pwd` |
| 5 | `cmd/workflow.rs` `execute_workflow_invocation` (~5354) | `ctx.project_root` / `ctx.workspace_path` |
| 6 | `cmd/workflow.rs` `resume_run` (~3664) | `project_root` (already `project_root_for(&workspace_path)`) / `workspace_path` |
| 7 | `resume.rs` `rebuild_opts_from_disk` (~314) | same as #6, from the loaded record |
| 10 | `cmd/session.rs` `start` (~1537) | `project_root` / `pwd` |
| 11 | `cmd/session.rs` `run_turn` (~7685) | `session.project_root` / `session.workspace_path` |
| 12 | `cmd/session.rs` `run_compact_request` (~7290) | same as #11; **change** its `.unwrap_or_default()` on the loader into `?` (it picks the compaction provider), and make the function return the error the way its caller expects — read the caller first; if the function cannot return an error, log `warn!` and return without compacting rather than compacting on the global config |
| 13 | `cmd/session.rs` `compact` (~6907) | same as #11 |
| 18 | `cmd/autoflow.rs` `resolve_config(global, project_root)` (~11977) | `project_root` / `project_root` — when `project_root` is `None` this wrapper serves no project: use `LayerPaths::global_only`. Keep the wrapper's signature (its tests call it) |
| 20 | `cmd/cron.rs` `tick_polled_events` (~485) | `project_root` / `pwd` |
| 30 | `cmd/issues.rs` `build_registry` (~504) | `project_root` / `pwd` |
| 31 | `cmd/mcp.rs` `serve_inner` (~49) | `project_root` / `pwd` |
| 32 | `cmd/repos.rs` `list_inner` (~182) | `project_root` / `pwd` |

**Wrapper sites with a decision-making caller** — change the wrapper to return `anyhow::Result<Config>` using the strict helper; the decision-making caller uses `?`, display callers do `.unwrap_or_else(|e| { tracing::warn!(error = %format!("{e:#}"), "config"); Config::default() })`:

| # | Wrapper | Decision caller (`?`) | Display callers |
|---|---|---|---|
| 8 | `cmd/workflow.rs` `layered_config_workflow` (~2700) — add a `run_dir: &Path` param (callers pass their `pwd`) | `create` (~2227, `--gen-provider`) | `list` (~1369), `runs` (~2612), `show_run` (~2834) |
| 36 | `cmd/agent.rs` `layered_config` (~579) — add `run_dir: &Path` | `create` (~445, `--gen-provider`) | `list` (~308), `show` (~367) |

**Lenient sites** (`config_paths_for_display`; keep existing error handling of the loader):

| # | Site | `project_root` / `run_dir` |
|---|---|---|
| 9 | `cmd/workflow.rs` `show` (~1467) | `project_root` / `pwd` |
| 14 | `cmd/session.rs` `attach_blocking` (~1988) | `project_root` / `pwd` |
| 15 | `cmd/session.rs` `list` (~1309) | `project_root` / `pwd` |
| 16 | `cmd/session.rs` `show` (~1346) | `project_root` / `pwd` |
| 19 | `cmd/autoflow.rs` `retained_serve_ui_prefs` (~4117) | `project_root` / `pwd` |
| 21 | `cmd/cron.rs` `events` (~711) | `project_root` / `pwd` |
| 22 | `cmd/cron.rs` `ui_prefs` (~371) | `project_root` / `pwd` |
| 28 | `cmd/update.rs` `load_cli_config` (~68) | `project_root` / `pwd` (callers read `log_level` / update prefs only) |
| 29 | `cmd/webhook.rs` `load_cli_config` (~161) | `project_root` / `pwd` (the daemon's own settings; each dispatched run loads strictly through #5) |
| 33 | `cmd/repos.rs` `tracked_inner` (~446) | `project_root` / `pwd` |
| 34 | `cmd/scm.rs` `warn_if_account_unknown` (~127) | `project_root` / `pwd` |
| 35 | `cmd/scm.rs` `accounts_inner` (~295) | `project_root` / `pwd` |
| 37 | `cmd/usage.rs` `layered_config` (~256) | add `run_dir: &Path`; `project_root` / `run_dir` |
| 38 | `cmd/editor.rs` `config_editor` (~88) | `project_root` / `pwd` |
| 39 | `cmd/watch.rs` `resolve_watch_pricing` (~332) | `project_root` / `pwd` |
| 40 | `cmd/watch.rs` `resolve_watch_prefs` (~346) | `project_root` / `pwd` |
| 41 | `cmd/coverage.rs` `ui_prefs(workspace)` (~87) | `project_root` / `workspace` — this site's global path is an `Option`; when it is `None` keep today's behaviour (build `LayerPaths::new(None, customer, project)` from the `ConfigPaths` fields by hand) |
| 42 | `cmd/netflow.rs` `prune_ui_prefs` (~374) | `project_root` / `pwd` |
| 43 | `cmd/transcript.rs` `list` (~1878) | `project_root` / `pwd` |
| 44 | `cmd/transcript.rs` `show` (~1961) | `project_root` / `pwd` |
| 45 | `cmd/transcript.rs` `prune_ui_prefs` (~2113) | `project_root` / `pwd` |
| 50 | `cmd/auth.rs` `auth_ui_prefs` (~836) | `project_root` / `pwd` |
| 54 | `cmd/ui.rs` `themes` (~678) | `project_root` / `pwd` |
| 55 | `output/workflow_printer.rs` `retained_workflow_ui_prefs` (~2330) | `project_root` / `pwd` |
| 56 | `output/diag.rs` `prefs_for_diag` (~191) | `project_root` / `pwd` — if `current_dir()` fails here, use `LayerPaths::global_only`; diag must never fail |

**Global-only by design** (`LayerPaths::global_only(&path)`; no customer — these read settings that are global by contract, or serve no project):

`cmd/run.rs` `list` (~416) and `show` (~501) · `cmd/session.rs` `session_prune_cutoff` (~8666) · `cmd/cp.rs` `handle` Serve arm (~103, ~385) · `cp_model_catalog.rs` (~12) · `cp_definition_generator.rs` (~24, ~102) · `cmd/transcript.rs` `prune_cutoff` (~2556) · `cmd/auth.rs` (~161, ~180, ~371, ~1165) · `cmd/findings.rs` `export_prefix` (~216) and `import_cmd` (~421) (the export prefix is global-only by contract) · `rupu-cp/src/state.rs` `resolve_global_config` (~274) · `rupu-cp/src/embed.rs` `resolve_shell` (~22) · `rupu-cp/src/lib.rs` `load_pricing` (~125).

**The CP Settings read** — `crates/rupu-cp/src/api/config.rs` `get_config` (~79): with `?project=<ws_id>`, include that project's customer. Add next to `project_config_path`:

```rust
/// The customer layer of the project `id`, if it is assigned — the same
/// lookup a run in that project makes. A dangling assignment is an error
/// (500), matching the run, which would fail too.
fn project_customer_config_path(s: &AppState, id: &str) -> ApiResult<Option<PathBuf>> {
    validate_ws_id(id)?;
    let store = rupu_workspace::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    };
    let ws = match store.load(id) {
        Ok(Some(w)) => w,
        Ok(None) => return Err(ApiError::not_found(format!("project {id} not found"))),
        Err(e) => return Err(ApiError::internal(e.to_string())),
    };
    rupu_workspace::CustomerStore::new(&s.global_dir)
        .customer_config_for_dir(Path::new(&ws.path))
        .map_err(|e| ApiError::internal(e.to_string()))
}
```

and in `get_config`:

```rust
    let (project_path, customer_path) = match &q.project {
        Some(id) => (
            Some(project_config_path(&s, id)?),
            project_customer_config_path(&s, id)?,
        ),
        None => (None, None),
    };
    let resolved = rupu_config::resolve(rupu_config::LayerPaths::new(
        Some(&global),
        customer_path.as_deref(),
        project_path.as_deref(),
    ))
    .map_err(|e| ApiError::internal(e.to_string()))?;
```

Add a test in that file's test module, next to the existing ones at ~768/~830 (copy their `AppState` construction):

```rust
#[tokio::test]
async fn get_config_with_project_reports_customer_provenance() {
    // Build the AppState the neighbouring tests build; register a project
    // with `rupu_workspace::CustomerStore::assign` after creating customer
    // `acme` whose config.toml sets `default_model = "acme-model"`.
    // GET /api/config?project=<ws_id> → effective.default_model ==
    // "acme-model" and provenance["default_model"].source == "customer".
}
```

Write the body for real by following the two existing `get_config` tests in that module (same router/oneshot helpers); the comment above is the assertion list, not a stub to leave in place.

Update the two existing test calls in that module (`rupu_config::resolve(...)` at ~768/~830) to `LayerPaths`.

- [ ] **Step 4: The web type**

In `crates/rupu-cp/web/src/lib/api.ts` (~1721):

```ts
/** Provenance source for one resolved config key — mirrors `rupu_config::KeySource`. */
export type KeySource = 'global' | 'customer' | 'project' | 'default';
```

and add to the provenance entry type right below (the one with `source: KeySource;`):

```ts
  /** Which layer's `[policy].lock` names this key (absent when unlocked). */
  locked_by?: 'global' | 'customer';
```

Run `cd crates/rupu-cp/web && npx tsc --noEmit` — expected clean. (Plan 2 renders the customer badge; here the type only has to admit the value.)

- [ ] **Step 5: Tests that called the old signature**

`crates/rupu-cli/tests/serial/policy_lock.rs`: `layer_files_locked(Some(&global), Some(&project))` → `layer_files_locked(rupu_config::LayerPaths::new(Some(&global), None, Some(&project)))` (both occurrences). These deliberately have no customer.

- [ ] **Step 6: Build everything**

Run: `cargo check --workspace --all-targets`
Expected: clean. Then confirm nothing is left on the old shape:

```bash
grep -rn "layer_files(Some\|layer_files_locked(Some\|resolve(Some\|resolve(None" crates --include=*.rs
```

Expected: no output.

- [ ] **Step 7: Run the affected tests**

```bash
cargo test -p rupu-cli --lib paths::
cargo test -p rupu-cp --lib api::config::
cargo test -p rupu-cli --test serial policy_lock:: < /dev/null
cargo test -p rupu-cli --lib cmd::autoflow:: resolve_config
```

Expected: PASS (the autoflow `resolve_config` tests keep their signature and behaviour; a `None` project is still global-only).

- [ ] **Step 8: Lint and commit**

```bash
cargo clippy -p rupu-cli -p rupu-cp --all-targets -- -D warnings
git add crates/rupu-cli crates/rupu-cp
git commit -m "feat(cli,cp): every config load layers the project's customer"
```

Format only the files you edited, one by one, and only if `rustfmt --check --edition 2021 <file>` passes on `main`'s version of that file (several of these files are fmt-dirty on `main`; formatting them wholesale buries the diff).

---

### Task 4: `rupu customer` subcommand

**Files:**
- Create: `crates/rupu-cli/src/cmd/customer.rs`
- Modify: `crates/rupu-cli/src/cmd/mod.rs` (add `pub mod customer;`)
- Modify: `crates/rupu-cp/src/config_write.rs` (`split_dotted_key` → `pub`)
- Modify: `crates/rupu-cli/src/lib.rs` (`Cmd::Customer`, dispatch)
- Test: `crates/rupu-cli/tests/serial/customer_cli.rs`, `crates/rupu-cli/tests/serial/main.rs`

**Interfaces:**
- Consumes: `rupu_workspace::{CustomerStore, NewCustomer, MetaPatch, ProjectRef, Customer}` (Task 1); `crate::paths::{global_dir, project_root_for}`; `rupu_config::resolve`, `KeySource`, `LockOwner` (Task 2); `crate::cmd::editor::open_for_edit(explicit: Option<&str>, path: &Path) -> anyhow::Result<()>`; `crate::output::report::{emit_collection, CollectionOutput}`; `crate::output::tables::new_table`; `crate::output::diag::fail`.
- Produces: `pub enum Action` (clap) and `pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode`.

- [ ] **Step 1: Write the failing tests**

Add `mod customer_cli;` to `crates/rupu-cli/tests/serial/main.rs` (alphabetical among the existing `mod` lines). Create `crates/rupu-cli/tests/serial/customer_cli.rs`:

```rust
//! `rupu customer` end to end: the store it writes is the one a run reads.

use crate::ENV_LOCK;
use assert_fs::prelude::*;

async fn rupu(args: &[&str]) -> std::process::ExitCode {
    let mut argv = vec!["rupu".to_string()];
    argv.extend(args.iter().map(|s| s.to_string()));
    rupu_cli::run(argv).await
}

/// `ExitCode` has no `PartialEq`; compare the way the other serial tests do.
fn ok(code: std::process::ExitCode) -> bool {
    format!("{code:?}") == format!("{:?}", std::process::ExitCode::from(0))
}

#[tokio::test(flavor = "multi_thread")]
async fn create_assign_show_and_delete_round_trip() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let project = tmp.child("proj");
    project.child(".rupu").create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();

    assert!(ok(rupu(&["customer", "create", "acme", "--name", "Acme Corp"]).await));
    assert!(!ok(rupu(&["customer", "create", "acme", "--name", "Again"]).await), "duplicate refused");
    assert!(ok(rupu(&["customer", "assign", "acme"]).await), "defaults to the cwd project");

    let store = rupu_workspace::CustomerStore::new(home.path());
    assert_eq!(
        store.customer_for_dir(project.path()).unwrap().as_deref(),
        Some("acme")
    );
    assert!(ok(rupu(&["customer", "show", "acme"]).await));
    assert!(ok(rupu(&["customer", "list"]).await));

    assert!(!ok(rupu(&["customer", "delete", "acme"]).await), "refused while assigned");
    assert!(ok(rupu(&["customer", "unassign"]).await));
    assert!(ok(rupu(&["customer", "delete", "acme"]).await));
    assert!(store.list(true).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn set_archive_and_unarchive() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    assert!(ok(rupu(&["customer", "create", "acme", "--name", "Acme"]).await));
    assert!(ok(rupu(&["customer", "set", "acme", "--color", "#112233", "--notes", "retainer"]).await));
    assert!(!ok(rupu(&["customer", "set", "acme", "--color", "red"]).await), "bad color refused");
    assert!(ok(rupu(&["customer", "archive", "acme"]).await));
    let store = rupu_workspace::CustomerStore::new(home.path());
    let c = store.get("acme").unwrap();
    assert!(c.meta.archived);
    assert_eq!(c.meta.color.as_deref(), Some("#112233"));
    assert_eq!(c.meta.notes.as_deref(), Some("retainer"));
    assert!(ok(rupu(&["customer", "unarchive", "acme"]).await));
    assert!(!store.get("acme").unwrap().meta.archived);
}

#[tokio::test(flavor = "multi_thread")]
async fn assign_by_explicit_project_path() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let project = tmp.child("elsewhere");
    project.create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    assert!(ok(rupu(&["customer", "create", "acme", "--name", "Acme"]).await));
    let p = project.path().to_str().unwrap();
    assert!(ok(rupu(&["customer", "assign", "acme", "--project", p]).await));
    let store = rupu_workspace::CustomerStore::new(home.path());
    assert_eq!(store.projects_of("acme").unwrap().len(), 1);
}
```

(The `ProcessStateGuard` restores env and cwd on drop, so the tests need no manual cleanup.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cli --test serial customer_cli:: < /dev/null`
Expected: FAIL — the `customer` subcommand does not exist (clap usage error, non-success exit).

- [ ] **Step 3: Write `cmd/customer.rs`**

```rust
//! `rupu customer` — group projects under a customer whose config layer sits
//! between global and project. Thin: argument parsing, then
//! `rupu_workspace::CustomerStore`. Spec:
//! `docs/superpowers/specs/2026-10-06-rupu-customers-design.md`.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use crate::paths;
use clap::Subcommand;
use comfy_table::Cell;
use rupu_workspace::{CustomerStore, MetaPatch, NewCustomer, ProjectRef};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List customers.
    List {
        /// Include archived customers.
        #[arg(long)]
        archived: bool,
    },
    /// Metadata, assigned projects, and the customer's effective config.
    Show { slug: String },
    /// Create a customer.
    Create {
        slug: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        contact: Option<String>,
        /// `#rrggbb`.
        #[arg(long)]
        color: Option<String>,
    },
    /// Change metadata. An empty value clears `--notes`, `--contact`, `--color`.
    Set {
        slug: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        contact: Option<String>,
        #[arg(long)]
        color: Option<String>,
    },
    /// Edit the customer's config layer in $EDITOR (validated on save).
    Edit {
        slug: String,
        /// Editor command; defaults to `[ui].editor`, then $VISUAL / $EDITOR.
        #[arg(long)]
        editor: Option<String>,
    },
    /// Hide a customer from pickers; its projects keep running.
    Archive { slug: String },
    Unarchive { slug: String },
    /// Delete a customer. Refused while any project is assigned.
    Delete { slug: String },
    /// Assign a project (default: the project in the current directory).
    Assign {
        slug: String,
        /// A project directory or workspace id (`ws_…`).
        #[arg(long)]
        project: Option<String>,
    },
    /// Remove a project's customer (default: the current directory's project).
    Unassign {
        #[arg(long)]
        project: Option<String>,
    },
}

pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode {
    match handle_inner(action, format) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => crate::output::diag::fail(e),
    }
}

fn store() -> anyhow::Result<CustomerStore> {
    Ok(CustomerStore::new(paths::global_dir()?))
}

/// `--project` as given, else the current directory's project root (the
/// directory holding `.rupu/`), else the current directory.
fn project_dir(project: Option<&str>) -> anyhow::Result<ProjectArg> {
    match project {
        Some(p) if p.starts_with("ws_") => Ok(ProjectArg::Id(p.to_string())),
        Some(p) => Ok(ProjectArg::Path(PathBuf::from(p))),
        None => {
            let pwd = std::env::current_dir()?;
            Ok(ProjectArg::Path(paths::project_root_for(&pwd)?.unwrap_or(pwd)))
        }
    }
}

enum ProjectArg {
    Path(PathBuf),
    Id(String),
}

impl ProjectArg {
    fn as_ref(&self) -> ProjectRef<'_> {
        match self {
            ProjectArg::Path(p) => ProjectRef::Path(p),
            ProjectArg::Id(id) => ProjectRef::Id(id),
        }
    }
}

fn handle_inner(action: Action, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let store = store()?;
    match action {
        Action::List { archived } => list(&store, archived, format),
        Action::Show { slug } => show(&store, &slug),
        Action::Create {
            slug,
            name,
            notes,
            contact,
            color,
        } => {
            let c = store.create(
                &slug,
                &NewCustomer {
                    name,
                    notes,
                    contact,
                    color,
                },
            )?;
            println!("created customer {} ({})", c.slug, c.meta.name);
            println!("config layer: {}", store.config_path(&c.slug).display());
            Ok(())
        }
        Action::Set {
            slug,
            name,
            notes,
            contact,
            color,
        } => {
            store.update_meta(
                &slug,
                &MetaPatch {
                    name,
                    notes,
                    contact,
                    color,
                },
            )?;
            Ok(())
        }
        Action::Edit { slug, editor } => edit(&store, &slug, editor.as_deref()),
        Action::Archive { slug } => store.set_archived(&slug, true).map(|_| ()).map_err(Into::into),
        Action::Unarchive { slug } => store.set_archived(&slug, false).map(|_| ()).map_err(Into::into),
        Action::Delete { slug } => {
            store.delete(&slug)?;
            println!("deleted customer {slug}");
            Ok(())
        }
        Action::Assign { slug, project } => {
            let target = project_dir(project.as_deref())?;
            let ws = store.assign(&slug, target.as_ref())?;
            println!("assigned {} ({}) to {slug}", ws.path, ws.id);
            Ok(())
        }
        Action::Unassign { project } => {
            let target = project_dir(project.as_deref())?;
            let ws = store.unassign(target.as_ref())?;
            println!("unassigned {} ({})", ws.path, ws.id);
            Ok(())
        }
    }
}

/// Open the layer in the editor; on a parse/validation error, say why and
/// offer to re-open, so a typo never leaves a layer that fails every run.
fn edit(store: &CustomerStore, slug: &str, editor: Option<&str>) -> anyhow::Result<()> {
    store.get(slug)?;
    let path = store.config_path(slug);
    loop {
        crate::cmd::editor::open_for_edit(editor, &path)?;
        match rupu_config::layer_files(rupu_config::LayerPaths::new(None, Some(&path), None)) {
            Ok(_) => return Ok(()),
            Err(e) => {
                eprintln!("{} is invalid: {e}", path.display());
                if !confirm_reopen()? {
                    anyhow::bail!(
                        "left {} invalid — every run of {slug}'s projects will fail until it is fixed",
                        path.display()
                    );
                }
            }
        }
    }
}

fn confirm_reopen() -> anyhow::Result<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("re-open the editor? [Y/n] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(!line.trim().eq_ignore_ascii_case("n"))
}

fn show(store: &CustomerStore, slug: &str) -> anyhow::Result<()> {
    let c = store.get(slug)?;
    println!("{} — {}", c.slug, c.meta.name);
    if c.meta.archived {
        println!("archived");
    }
    for (label, v) in [
        ("contact", &c.meta.contact),
        ("color", &c.meta.color),
        ("notes", &c.meta.notes),
    ] {
        if let Some(v) = v {
            println!("{label}: {v}");
        }
    }
    println!("created: {}", c.meta.created_at);

    let projects = store.projects_of(slug)?;
    println!("\nprojects ({}):", projects.len());
    for ws in &projects {
        println!("  {}  {}", ws.id, ws.path);
    }

    let global = paths::global_dir()?.join("config.toml");
    let layer = store.config_path(slug);
    println!("\nconfig layer: {}", layer.display());
    print_effective(&global, &layer)
}

/// Effective config for a project of this customer that sets nothing
/// itself: every key the global or customer layer sets, its value, where it
/// came from, and who locks it.
fn print_effective(global: &Path, layer: &Path) -> anyhow::Result<()> {
    let r = rupu_config::resolve(rupu_config::LayerPaths::new(Some(global), Some(layer), None))?;
    let flat = toml::Value::try_from(&r.config)?;
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["KEY", "VALUE", "SOURCE", "LOCKED BY"]);
    for (key, prov) in &r.provenance {
        let value = lookup_dotted(&flat, key).map(|v| v.to_string()).unwrap_or_default();
        table.add_row(vec![
            Cell::new(key),
            Cell::new(value),
            Cell::new(format!("{:?}", prov.source).to_lowercase()),
            Cell::new(
                prov.locked_by
                    .map(|o| format!("{o:?}").to_lowercase())
                    .unwrap_or_else(|| "-".into()),
            ),
        ]);
    }
    println!("{table}");
    for w in &r.warnings {
        println!("warning: {w}");
    }
    Ok(())
}

/// Walk a provenance key. Decoded with the CP write path's
/// `split_dotted_key` — the canonical decoder of the dotted-key contract —
/// never a naive `split('.')` (a model id like `GLM-5.2-FP8` is one segment).
fn lookup_dotted<'a>(v: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let segs = rupu_cp::config_write::split_dotted_key(key).ok()?;
    segs.iter().try_fold(v, |cur, seg| cur.get(seg.as_str()))
}

#[derive(Debug, Clone, Serialize)]
struct CustomerRow {
    slug: String,
    name: String,
    projects: usize,
    archived: bool,
    color: String,
}

#[derive(Debug, Clone, Serialize)]
struct CustomersReport {
    kind: &'static str,
    version: u8,
    rows: Vec<CustomerRow>,
}

struct CustomersOutput {
    report: CustomersReport,
}

impl CollectionOutput for CustomersOutput {
    type JsonReport = CustomersReport;
    type CsvRow = CustomerRow;

    fn command_name(&self) -> &'static str {
        "customer list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["slug", "name", "projects", "archived", "color"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["SLUG", "NAME", "PROJECTS", "ARCHIVED", "COLOR"]);
        for r in &self.report.rows {
            table.add_row(vec![
                Cell::new(&r.slug),
                Cell::new(&r.name),
                Cell::new(r.projects),
                Cell::new(if r.archived { "yes" } else { "" }),
                Cell::new(&r.color),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

fn list(store: &CustomerStore, archived: bool, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let mut rows = Vec::new();
    for c in store.list(archived)? {
        rows.push(CustomerRow {
            projects: store.projects_of(&c.slug)?.len(),
            name: c.meta.name,
            archived: c.meta.archived,
            color: c.meta.color.unwrap_or_default(),
            slug: c.slug,
        });
    }
    report::emit_collection(
        format,
        &CustomersOutput {
            report: CustomersReport {
                kind: "customers",
                version: 1,
                rows,
            },
        },
    )
}
```

Make the CP's decoder reachable: in `crates/rupu-cp/src/config_write.rs` (~157) change `fn split_dotted_key` to `pub fn split_dotted_key` and add to its doc comment: "Also used by `rupu customer show` to walk provenance keys — one decoder, not a fifth lockstep site." (`rupu-cli` already depends on `rupu-cp`; `cmd/scm.rs` uses `rupu_cp::config_write::write_atomic`.)

Before relying on `toml::Value::try_from(&r.config)`, confirm `Config: Serialize` (`grep -n "derive(.*Serialize" crates/rupu-config/src/config.rs | head -2`); it is (the CP serializes it for `/api/config`). Confirm `open_for_edit`'s first parameter is the explicit editor override (`crates/rupu-cli/src/cmd/editor.rs:25`).

- [ ] **Step 4: Register the subcommand**

In `crates/rupu-cli/src/cmd/mod.rs` add `pub mod customer;` (alphabetical). In `crates/rupu-cli/src/lib.rs`, add to `enum Cmd` right after `Config { … }`:

```rust
    /// Customers: group projects under a customer with its own config layer.
    Customer {
        #[command(subcommand)]
        action: cmd::customer::Action,
    },
```

and in the dispatch `match`, after the `Cmd::Config` arm:

```rust
        Cmd::Customer { action } => cmd::customer::handle(action, cli.format).await,
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rupu-cli --test serial customer_cli:: < /dev/null`
Expected: PASS. Also run `cargo test -p rupu-cli --test it test_layout::` — it checks every serial test takes `ENV_LOCK`.

- [ ] **Step 6: Lint and commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/customer.rs crates/rupu-cli/tests/serial/customer_cli.rs
cargo clippy -p rupu-cli --all-targets -- -D warnings
git add crates/rupu-cli crates/rupu-cp/src/config_write.rs
git commit -m "feat(cli): rupu customer — create/set/edit/archive/delete/assign/show/list"
```

(`lib.rs`, `cmd/mod.rs` and `tests/serial/main.rs` were edited by hand; do not run `rustfmt` on them unless they were already rustfmt-clean — check with `rustfmt --check --edition 2021 <file>` first and only format if the check passes on `main`'s version.)

---

### Task 5: A real run honours the customer layer (end to end)

**Files:**
- Test: `crates/rupu-cli/tests/serial/customer_layer.rs`, `crates/rupu-cli/tests/serial/main.rs`

This task adds no production code; it proves Tasks 1–4 compose on the actual `rupu run` path, the same way `tests/serial/policy_lock.rs` proves `[policy].lock` — by observing a permission decision, not a config value.

**Interfaces:**
- Consumes: `rupu_cli::run`, `RUPU_HOME`, `RUPU_MOCK_PROVIDER_SCRIPT`, `rupu_workspace::{CustomerStore, NewCustomer, ProjectRef}`.

- [ ] **Step 1: Write the tests**

Add `mod customer_layer;` to `crates/rupu-cli/tests/serial/main.rs`. Create `crates/rupu-cli/tests/serial/customer_layer.rs`:

```rust
//! The customer layer on the real `rupu run` path. Like `policy_lock.rs`,
//! these observe the decision the layer drives — whether a `write_file`
//! call is permitted — rather than a config value, so they fail if any
//! launch-path loader skipped the customer layer.

use crate::ENV_LOCK;
use assert_fs::prelude::*;
use rupu_workspace::{CustomerStore, NewCustomer, ProjectRef};

const WRITE_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "write_file", "tool_input": {"path": "out.txt", "content": "written"}, "stop": "tool_use" } },
  { "AssistantText": { "text": "done", "stop": "end_turn" } }
]
"#;

const WRITER_AGENT: &str =
    "---\nname: writer\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 2\ntools: [write_file]\n---\nyou write files.";

/// `<tmp>/.rupu` (home, with the writer agent and `global_cfg`) and
/// `<tmp>/proj` (with `.rupu/config.toml` = `project_cfg`), the project
/// assigned to customer `acme` whose layer is `customer_cfg`.
fn fixture(
    global_cfg: &str,
    customer_cfg: &str,
    project_cfg: &str,
) -> (assert_fs::TempDir, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.child("agents").create_dir_all().unwrap();
    home.child("agents/writer.md").write_str(WRITER_AGENT).unwrap();
    home.child("config.toml").write_str(global_cfg).unwrap();

    let project = tmp.child("proj");
    project.child(".rupu").create_dir_all().unwrap();
    project.child(".rupu/config.toml").write_str(project_cfg).unwrap();

    let store = CustomerStore::new(home.path());
    store
        .create(
            "acme",
            &NewCustomer {
                name: "Acme".into(),
                ..NewCustomer::default()
            },
        )
        .unwrap();
    std::fs::write(store.config_path("acme"), customer_cfg).unwrap();
    store.assign("acme", ProjectRef::Path(project.path())).unwrap();
    (tmp, project.path().to_path_buf())
}

/// `ExitCode` has no `PartialEq`; compare the way the other serial tests do.
fn ok(code: std::process::ExitCode) -> bool {
    format!("{code:?}") == format!("{:?}", std::process::ExitCode::from(0))
}

async fn run_writer(home: &std::path::Path, cwd: &std::path::Path) -> std::process::ExitCode {
    std::env::set_var("RUPU_HOME", home);
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", WRITE_SCRIPT);
    std::env::set_current_dir(cwd).unwrap();
    rupu_cli::run(vec!["rupu".into(), "run".into(), "writer".into(), "go".into()]).await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_customer_layer_applies_when_the_project_is_silent() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture("permission_mode = \"bypass\"\n", "permission_mode = \"readonly\"\n", "");
    run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(
        !project.join("out.txt").exists(),
        "the customer's readonly mode should have denied write_file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unlocked_project_key_beats_the_customer() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture(
        "",
        "permission_mode = \"readonly\"\n",
        "permission_mode = \"bypass\"\n",
    );
    run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(project.join("out.txt").exists(), "project bypass should win");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_customer_lock_beats_the_project() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture(
        "",
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"bypass\"\n",
    );
    run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(
        !project.join("out.txt").exists(),
        "the repo config overrode a customer-LOCKED permission_mode"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subdirectory_run_inherits_the_customer() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture("permission_mode = \"bypass\"\n", "permission_mode = \"readonly\"\n", "");
    let sub = project.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    run_writer(tmp.child(".rupu").path(), &sub).await;
    assert!(!sub.join("out.txt").exists() && !project.join("out.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dangling_assignment_fails_the_run() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture("permission_mode = \"bypass\"\n", "", "");
    std::fs::remove_dir_all(tmp.child(".rupu/customers/acme").path()).unwrap();
    let code = run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(!ok(code), "must not fall back to global config");
    assert!(!project.join("out.txt").exists(), "no model call may happen");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_customer_layer_fails_the_run() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture("permission_mode = \"bypass\"\n", "permission_mode = \n", "");
    let code = run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(!ok(code));
    assert!(!project.join("out.txt").exists());
}
```

`rupu workflow run` takes its permission mode from the CLI only (`mode.unwrap_or("ask")` in `cmd/workflow.rs`), so for workflows the observable is the **provider**: the mock provider accepts any name (`build_for_provider_with_config` returns the mock before looking at it), and the step transcript's `RunStart` records the resolved provider. Append to the same file:

```rust
const ECHO_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "step output", "stop": "end_turn" } }
]
"#;

const HELLO_WF: &str = "name: hello-wf\nsteps:\n  - id: a\n    agent: echo\n    actions: []\n    prompt: hi\n";

/// `echo` names no provider, so the run takes `default_provider` from config.
const ECHO_AGENT: &str = "---\nname: echo\nmodel: claude-sonnet-4-6\n---\nyou echo.";

async fn run_hello_wf(home: &std::path::Path, cwd: &std::path::Path) -> std::process::ExitCode {
    std::env::set_var("RUPU_HOME", home);
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", ECHO_SCRIPT);
    std::env::set_current_dir(cwd).unwrap();
    rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "hello-wf".into(),
        "--mode".into(),
        "bypass".into(),
    ])
    .await
}

fn workflow_fixture(customer_cfg: &str) -> (assert_fs::TempDir, std::path::PathBuf) {
    let (tmp, project) = fixture("default_provider = \"anthropic\"\n", customer_cfg, "");
    let home = tmp.child(".rupu");
    home.child("agents/echo.md").write_str(ECHO_AGENT).unwrap();
    home.child("workflows").create_dir_all().unwrap();
    home.child("workflows/hello-wf.yaml").write_str(HELLO_WF).unwrap();
    (tmp, project)
}

/// The first line of the only run's first step transcript.
fn first_step_run_start(home: &std::path::Path) -> String {
    let store = rupu_orchestrator::RunStore::new(home.join("runs"));
    let runs = store.list().unwrap();
    assert_eq!(runs.len(), 1, "expected exactly one workflow run");
    let steps = store.read_step_results(&runs[0].id).unwrap();
    let transcript = std::fs::read_to_string(&steps[0].transcript_path).unwrap();
    transcript.lines().next().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_run_takes_the_customer_default_provider() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = workflow_fixture(
        "default_provider = \"anthropic-acme\"\n[providers.anthropic-acme]\nkind = \"anthropic\"\n",
    );
    let code = run_hello_wf(tmp.child(".rupu").path(), &project).await;
    assert!(ok(code), "workflow run failed");
    let run_start = first_step_run_start(tmp.child(".rupu").path());
    assert!(
        run_start.contains("\"provider\":\"anthropic-acme\""),
        "the step should run on the customer's account: {run_start}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_run_fails_on_a_dangling_assignment() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = workflow_fixture("");
    std::fs::remove_dir_all(tmp.child(".rupu/customers/acme").path()).unwrap();
    let code = run_hello_wf(tmp.child(".rupu").path(), &project).await;
    assert!(!ok(code), "must not fall back to global config");
}
```

`rupu-orchestrator` is already a dev-dependency of `rupu-cli` (`tests/serial/cli_workflow.rs` uses `rupu_orchestrator::RunStore`). If `RunStart`'s serialized key is not `"provider"` (check `crates/rupu-transcript/src/event.rs`, `RunStart { … provider: String, … }`, and the enum's `serde` tag attributes), adjust the substring to the real key — do not loosen the assertion to "contains anthropic".

- [ ] **Step 2: Run them**

Run: `cargo test -p rupu-cli --test serial customer_layer:: < /dev/null`
Expected: PASS. If any fails, the failing launch path missed the customer layer in Task 3 — fix the call site there, not the test.

- [ ] **Step 3: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/tests/serial/customer_layer.rs
git add crates/rupu-cli/tests/serial
git commit -m "test(cli): rupu run / workflow run honour the customer layer and its locks"
```

---

### Task 6: Docs

**Files:**
- Modify: `docs/configuration.md` (the `## Locations and layering` section, and `## [policy]`)
- Modify: `CLAUDE.md` (Read-first list; `rupu-cli` subcommand list; `rupu-workspace` is not described there, so add the customers note to the `rupu-cli` bullet)

- [ ] **Step 1: `docs/configuration.md`**

In `## Locations and layering`, add the customer file as the middle layer, and this paragraph:

```markdown
### Customer layer

A project can be assigned to a **customer** (`rupu customer assign <slug>`). The
customer's layer, `~/.rupu/customers/<slug>/config.toml`, sits between the global
file and the project's `.rupu/config.toml`:

global lock › customer lock › project › customer › global › default

- The project still wins on any key the customer has not locked.
- A customer locks keys with its own `[policy].lock` — e.g. `["default_provider"]`
  so a repo's committed config cannot move the customer's runs onto another account.
  It cannot unlock a key the global `[policy].lock` names.
- Arrays replace, as between global and project: a customer that declares
  `[[scm.rules]]` replaces the global rules for its projects.
- A run in a subdirectory uses the customer of the nearest assigned ancestor.
- A project assigned to a customer that no longer exists, or a customer layer that
  does not parse, fails the run — it never falls back to the global config.

Typical customer layer:

    default_provider = "anthropic-acme"

    [providers.anthropic-acme]
    kind = "anthropic"

    [[scm.rules]]
    owner = "acme-corp"
    account = "github-acme"

    [policy]
    lock = ["default_provider"]

Log in each named account once (`rupu auth login --provider anthropic-acme …`).
```

In `## [policy]`, add one sentence: a customer layer's `[policy].lock` locks keys against the project layer only (see *Customer layer*).

Before writing the `rupu auth login` line, check its real flag spelling: `rupu auth login --help` (or `grep -n "struct LoginArgs\|Login {" -A15 crates/rupu-cli/src/cmd/auth.rs`) and use that exact form.

- [ ] **Step 2: `CLAUDE.md`**

- Read-first list: add `- Customers spec + Plan 1 (model, config layer, CLI): \`docs/superpowers/specs/2026-10-06-rupu-customers-design.md\`, \`docs/superpowers/plans/2026-10-06-rupu-customers-plan-1-model-config-cli.md\``.
- `rupu-cli` bullet: "Fourteen subcommands" lists only some; add `customer` to the list and append: "`rupu customer` manages customers (`rupu_workspace::CustomerStore`: `customers/<slug>/{customer,config}.toml`, assignment sidecars `workspaces/<ws_id>.customer`); every config load goes through `paths::config_paths`, which layers global → customer → project (`rupu_config::LayerPaths`)."

- [ ] **Step 3: Commit**

```bash
git add docs/configuration.md CLAUDE.md
git commit -m "docs: customer config layer"
```

---

### Task 7: Verify and review

- [ ] **Step 1: Targeted tests**

```bash
cargo test -p rupu-config
cargo test -p rupu-workspace --test it
cargo test -p rupu-cli --test serial customer_ < /dev/null
cargo test -p rupu-cli --test serial policy_lock:: < /dev/null
cargo test -p rupu-cli --test it test_layout::
cargo test -p rupu-cp config
```

Expected: all pass. `policy_lock::` must still pass unchanged — it is the regression guard that global locks still win.

- [ ] **Step 2: Build and lint the touched crates**

```bash
cargo clippy -p rupu-config -p rupu-workspace -p rupu-cli -p rupu-cp --all-targets -- -D warnings
```

- [ ] **Step 3: No loader call left on the two-path shape**

```bash
grep -rn "layer_files(Some\|layer_files_locked(Some\|resolve(Some\|resolve(None" crates --include=*.rs
```

Expected: no output (every call now passes a `LayerPaths`).

- [ ] **Step 4: Code review**

Invoke `superpowers:requesting-code-review` on the branch diff. Fix every behaviour bug it finds in-branch before opening the PR.
