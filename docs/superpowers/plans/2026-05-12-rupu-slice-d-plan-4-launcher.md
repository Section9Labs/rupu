# Slice D Plan 4 — Launcher

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the launcher sheet — workflow inputs form + mode picker + target picker (workspace dir / browse / clone RepoRef) + Run button. Three triggers (Graph toolbar button / ⌘R / right-click). Operator-complete: open workspace → run workflow → watch + approve all happens inside `rupu.app`.

**Architecture:** New `crates/rupu-app/src/launcher/` module owns `LauncherState` (pure data) + `clone_repo_ref` async helper. New `crates/rupu-app/src/view/launcher.rs` is the GPUI render function. `WorkspaceWindow` gains `launcher: Option<LauncherState>` and `focused_workflow: Option<PathBuf>` fields; existing `handle_run_clicked` is repurposed to open the launcher instead of dispatching directly. `AppExecutor::start_workflow_with_opts(path, inputs, mode, target_dir)` extends the existing `start_workflow(path)`. The clone helper is lifted from `rupu-cli`'s `--tmp` flow into `rupu-scm` so both CLI and app share it. Single-tab (real tab strip lands in D-5).

**Tech Stack:** Rust 2021, GPUI (already pinned), `tokio`, `tempfile` (already a workspace dep), `directories` (already a rupu-app dep), `rupu-scm::RepoConnector::clone_to` for cloning. No new external dependencies.

**Spec:** `docs/superpowers/specs/2026-05-12-rupu-slice-d-plan-4-launcher-design.md`

---

## File structure

### New files

```
crates/rupu-app/src/launcher/
  mod.rs                              # Module root: pub mod state; pub mod clone;
  state.rs                            # LauncherState + enums + validation
  clone.rs                            # Async wrapper around rupu-scm clone helper

crates/rupu-app/src/view/launcher.rs  # GPUI render function for the sheet

crates/rupu-app/tests/launcher_state.rs  # Validation + state-machine tests

crates/rupu-scm/src/clone.rs          # NEW — lifted clone-to-tempdir helper
```

### Modified files

```
crates/rupu-scm/src/lib.rs            # pub mod clone; pub use clone::clone_repo_ref;
crates/rupu-cli/src/cmd/workflow.rs   # call rupu_scm::clone_repo_ref instead of inline code (~lines 1199-1216)

crates/rupu-app/src/lib.rs            # pub mod launcher;
crates/rupu-app/src/view/mod.rs       # pub mod launcher;
crates/rupu-app/src/executor/mod.rs   # add start_workflow_with_opts; keep start_workflow as wrapper
crates/rupu-app/src/window/mod.rs     # add launcher + focused_workflow fields; open/close handlers; replace handle_run_clicked body
crates/rupu-app/src/window/sidebar.rs # click handler updates focused_workflow; right-click opens launcher
crates/rupu-app/src/menu/app_menu.rs  # add LaunchSelected action + Cmd-R binding
crates/rupu-app/src/workspace/storage.rs # add clones_dir() helper
crates/rupu-app/src/main.rs           # call clones_dir GC on startup

CLAUDE.md                             # Mark Plan 4 complete + crate description updates
```

---

## Implementation tasks

### Task 1: `LauncherMode` + `LauncherTarget` + `CloneStatus` + `LauncherState`

**Files:**
- Create: `crates/rupu-app/src/launcher/mod.rs`
- Create: `crates/rupu-app/src/launcher/state.rs`
- Modify: `crates/rupu-app/src/lib.rs` (add `pub mod launcher;`)

- [ ] **Step 1: Scaffold the module**

`crates/rupu-app/src/launcher/mod.rs`:

```rust
//! Launcher — sheet UI for starting a workflow run from inside the
//! app. Owns `LauncherState` (pure data) + the async clone helper.

pub mod state;
pub mod clone;

pub use state::{
    CloneStatus, LauncherMode, LauncherState, LauncherTarget, ValidationError,
};
```

`crates/rupu-app/src/launcher/clone.rs` (empty stub for now):

```rust
//! Async wrapper around `rupu-scm`'s clone helper. Populated in Task 5.
```

Add `pub mod launcher;` to `crates/rupu-app/src/lib.rs` in alphabetical order.

- [ ] **Step 2: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean build (the empty modules compile).

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/launcher/ crates/rupu-app/src/lib.rs
git commit -m "feat(rupu-app): scaffold launcher module"
```

---

### Task 2: `LauncherState` + validation tests

**Files:**
- Modify: `crates/rupu-app/src/launcher/state.rs`
- Create: `crates/rupu-app/tests/launcher_state.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-app/tests/launcher_state.rs`:

```rust
//! LauncherState validation transitions. Each test exercises one
//! InputDef shape against `LauncherState::new` + per-keystroke
//! validation via `validate`.

use std::path::PathBuf;

use rupu_app::launcher::{LauncherMode, LauncherState, LauncherTarget};
use rupu_orchestrator::Workflow;

fn parse_wf(yaml: &str) -> Workflow {
    Workflow::parse(yaml).expect("parse")
}

#[test]
fn new_prefills_string_default() {
    let yaml = r#"
name: t
inputs:
  topic:
    type: string
    required: true
    default: "hello"
steps:
  - id: a
    agent: x
    prompt: "{{ input.topic }}"
"#;
    let wf = parse_wf(yaml);
    let state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    assert_eq!(state.inputs.get("topic").map(String::as_str), Some("hello"));
    assert!(matches!(state.mode, LauncherMode::Ask));
    assert!(matches!(state.target, LauncherTarget::ThisWorkspace));
    assert!(state.validation.is_none(), "default pre-fill should validate");
}

#[test]
fn missing_required_input_surfaces_validation_error() {
    let yaml = r#"
name: t
inputs:
  topic:
    type: string
    required: true
steps:
  - id: a
    agent: x
    prompt: "{{ input.topic }}"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.set_input("topic", "");
    state.revalidate();
    assert!(
        state.validation.is_some(),
        "empty required input must produce a validation error"
    );
}

#[test]
fn int_type_mismatch_surfaces_validation_error() {
    let yaml = r#"
name: t
inputs:
  count:
    type: int
    required: false
    default: 1
steps:
  - id: a
    agent: x
    prompt: "{{ input.count }}"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.set_input("count", "not-an-int");
    state.revalidate();
    assert!(state.validation.is_some());
}

#[test]
fn enum_not_allowed_surfaces_validation_error() {
    let yaml = r#"
name: t
inputs:
  mode:
    type: string
    required: true
    default: "fast"
    enum: [fast, slow]
steps:
  - id: a
    agent: x
    prompt: "{{ input.mode }}"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.set_input("mode", "wibble");
    state.revalidate();
    assert!(state.validation.is_some());
}

#[test]
fn valid_inputs_clear_validation() {
    let yaml = r#"
name: t
inputs:
  topic:
    type: string
    required: true
steps:
  - id: a
    agent: x
    prompt: "{{ input.topic }}"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.set_input("topic", "hello");
    state.revalidate();
    assert!(state.validation.is_none());
}

#[test]
fn mode_changes_persist() {
    let yaml = r#"
name: t
steps:
  - id: a
    agent: x
    prompt: "go"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.mode = LauncherMode::Bypass;
    assert!(matches!(state.mode, LauncherMode::Bypass));
}

#[test]
fn target_clone_status_transitions() {
    let yaml = r#"
name: t
steps:
  - id: a
    agent: x
    prompt: "go"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.target = LauncherTarget::Clone {
        repo_ref: "github:foo/bar".into(),
        status: rupu_app::launcher::CloneStatus::NotStarted,
    };
    if let LauncherTarget::Clone { ref mut status, .. } = state.target {
        *status = rupu_app::launcher::CloneStatus::InProgress;
    }
    assert!(matches!(
        state.target,
        LauncherTarget::Clone {
            status: rupu_app::launcher::CloneStatus::InProgress,
            ..
        }
    ));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-app --test launcher_state`
Expected: compile error (`LauncherState` not yet implemented).

- [ ] **Step 3: Implement `LauncherState`**

Replace `crates/rupu-app/src/launcher/state.rs` with:

```rust
//! LauncherState — pure data driving the launcher sheet. Mutated by
//! user input + the clone task. Validation re-runs on every keystroke.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rupu_orchestrator::Workflow;

#[derive(Debug, Clone)]
pub struct LauncherState {
    pub workflow_path: PathBuf,
    pub workflow: Workflow,
    pub inputs: BTreeMap<String, String>,
    pub mode: LauncherMode,
    pub target: LauncherTarget,
    pub validation: Option<ValidationError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherMode {
    Ask,
    Bypass,
    ReadOnly,
}

impl LauncherMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            LauncherMode::Ask => "ask",
            LauncherMode::Bypass => "bypass",
            LauncherMode::ReadOnly => "readonly",
        }
    }
}

#[derive(Debug, Clone)]
pub enum LauncherTarget {
    ThisWorkspace,
    Directory(PathBuf),
    Clone { repo_ref: String, status: CloneStatus },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneStatus {
    NotStarted,
    InProgress,
    Done(PathBuf),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub message: String,
}

impl LauncherState {
    pub fn new(workflow_path: PathBuf, workflow: Workflow) -> Self {
        let mut inputs: BTreeMap<String, String> = BTreeMap::new();
        for (name, def) in &workflow.inputs {
            if let Some(default) = &def.default {
                if let Some(s) = yaml_value_to_string(default) {
                    inputs.insert(name.clone(), s);
                }
            }
        }
        let mut state = Self {
            workflow_path,
            workflow,
            inputs,
            mode: LauncherMode::Ask,
            target: LauncherTarget::ThisWorkspace,
            validation: None,
        };
        state.revalidate();
        state
    }

    pub fn set_input(&mut self, name: &str, value: impl Into<String>) {
        let v = value.into();
        if v.is_empty() {
            self.inputs.remove(name);
        } else {
            self.inputs.insert(name.into(), v);
        }
    }

    pub fn revalidate(&mut self) {
        // Reuse the orchestrator's resolver. `RunWorkflowError` only
        // implements Display, so format the error message and store it.
        match rupu_orchestrator::resolve_inputs(&self.workflow, &self.inputs) {
            Ok(_) => self.validation = None,
            Err(e) => self.validation = Some(ValidationError { message: e.to_string() }),
        }
    }

    /// True when the Run button should be enabled.
    pub fn can_run(&self) -> bool {
        if self.validation.is_some() {
            return false;
        }
        matches!(
            &self.target,
            LauncherTarget::ThisWorkspace
                | LauncherTarget::Directory(_)
                | LauncherTarget::Clone {
                    status: CloneStatus::Done(_) | CloneStatus::NotStarted | CloneStatus::Failed(_),
                    ..
                }
        )
    }
}

fn yaml_value_to_string(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
```

- [ ] **Step 4: Make `resolve_inputs` reachable**

The function `resolve_inputs` lives in `crates/rupu-orchestrator/src/runner.rs` and is likely private. Add a re-export from the crate root.

In `crates/rupu-orchestrator/src/lib.rs`, add:

```rust
pub use runner::resolve_inputs;
```

(or, if `resolve_inputs` is `fn` not `pub fn`, change it to `pub fn` in `runner.rs` first.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-app --test launcher_state`
Expected: 7 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/launcher/state.rs crates/rupu-app/tests/launcher_state.rs crates/rupu-orchestrator/src/lib.rs crates/rupu-orchestrator/src/runner.rs
git commit -m "feat(rupu-app): LauncherState + pure validation tests"
```

---

### Task 3: Lift the `--tmp` clone helper into `rupu-scm`

**Files:**
- Create: `crates/rupu-scm/src/clone.rs`
- Modify: `crates/rupu-scm/src/lib.rs`
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (the inline tempdir block around lines 1199-1216)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-scm/tests/clone.rs`:

```rust
//! clone_repo_ref happy path against a fixture connector.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use rupu_scm::{clone_repo_ref, Platform, RepoConnector, RepoRef, Registry, ScmError};

struct FakeConnector;

#[async_trait]
impl RepoConnector for FakeConnector {
    async fn clone_to(&self, r: &RepoRef, dir: &Path) -> Result<(), ScmError> {
        std::fs::create_dir_all(dir).map_err(|e| ScmError::Other(e.to_string()))?;
        std::fs::write(dir.join("README.md"), format!("{}/{}\n", r.owner, r.repo))
            .map_err(|e| ScmError::Other(e.to_string()))?;
        Ok(())
    }
}

#[tokio::test]
async fn clone_repo_ref_creates_target_dir_with_content() {
    let mut registry = Registry::default();
    registry.set_repo(Platform::Github, Arc::new(FakeConnector));
    let r = RepoRef {
        platform: Platform::Github,
        owner: "foo".into(),
        repo: "bar".into(),
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("clone");
    clone_repo_ref(&registry, &r, &target).await.expect("clone");
    assert!(target.join("README.md").exists());
}
```

(The exact `Registry::default` / `set_repo` API may differ. Check `rupu-scm/src/connectors/mod.rs` for the actual constructor pattern and adapt.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-scm --test clone`
Expected: compile error (`clone_repo_ref` not exported).

- [ ] **Step 3: Implement `clone_repo_ref`**

Create `crates/rupu-scm/src/clone.rs`:

```rust
//! Clone-to-tempdir helper. Shared between rupu-cli's `--tmp` flag
//! and rupu-app's Launcher target=Clone path.

use std::path::Path;

use crate::{Platform, RepoRef, Registry, ScmError};

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("no {0} credential — run `rupu auth login --provider {0}`")]
    MissingConnector(Platform),
    #[error("clone failed: {0}")]
    Scm(#[from] ScmError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Clone the given RepoRef into `target_dir`. Caller owns the
/// destination (may be a tempfile::TempDir or any other path). Does
/// NOT create the parent directory automatically — callers should
/// pre-create as needed.
pub async fn clone_repo_ref(
    registry: &Registry,
    r: &RepoRef,
    target_dir: &Path,
) -> Result<(), CloneError> {
    let conn = registry
        .repo(r.platform)
        .ok_or(CloneError::MissingConnector(r.platform))?;
    conn.clone_to(r, target_dir).await?;
    Ok(())
}
```

(Replace `Platform` / `RepoRef` / `Registry` import paths with the actual public surface of `rupu-scm`. If `Registry` doesn't have a public `repo()` accessor, check the existing CLI call site at `workflow.rs:1205` for the actual API.)

- [ ] **Step 4: Wire `pub mod clone;` + re-exports in `crates/rupu-scm/src/lib.rs`**

```rust
pub mod clone;
pub use clone::{clone_repo_ref, CloneError};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rupu-scm --test clone`
Expected: 1 test passes.

- [ ] **Step 6: Refactor the CLI's `--tmp` flow to call the new helper**

In `crates/rupu-cli/src/cmd/workflow.rs` around lines 1199-1216, replace the inline tempdir + clone block with a call to `rupu_scm::clone_repo_ref`. The tempdir lifecycle stays in the CLI (the `(Some(tmp), p)` tuple is what's returned). The change is mechanical: keep the `tempfile::tempdir()` call, replace `conn.clone_to(&r, tmp.path()).await?;` with `rupu_scm::clone_repo_ref(&mcp_registry, &r, tmp.path()).await?;`, and remove the manual `registry.repo(*platform).ok_or_else(...)` line since `clone_repo_ref` does that lookup itself.

- [ ] **Step 7: Run the full test suite to verify no CLI regression**

Run: `cargo test -p rupu-cli`
Expected: same pass/fail set as before — only the pre-existing `samples_byte_match_dogfooded_files` failure.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-scm/src/clone.rs crates/rupu-scm/src/lib.rs crates/rupu-scm/tests/clone.rs crates/rupu-cli/src/cmd/workflow.rs
git commit -m "refactor(rupu-scm): extract clone_repo_ref helper; rupu-cli --tmp uses it"
```

---

### Task 4: `AppExecutor::start_workflow_with_opts`

**Files:**
- Modify: `crates/rupu-app/src/executor/mod.rs`

- [ ] **Step 1: Read current state**

Read `crates/rupu-app/src/executor/mod.rs` lines 60-100 to see the existing `start_workflow` body.

- [ ] **Step 2: Extend the executor's surface**

Add a new method on `AppExecutor` after the existing `start_workflow`:

```rust
pub async fn start_workflow_with_opts(
    &self,
    workflow_path: PathBuf,
    inputs: std::collections::BTreeMap<String, String>,
    mode: crate::launcher::LauncherMode,
    target_dir: PathBuf,
) -> Result<String, rupu_orchestrator::executor::ExecutorError> {
    let yaml = std::fs::read_to_string(&workflow_path)?;
    let workflow = rupu_orchestrator::Workflow::parse(&yaml)?;

    let factory: Arc<dyn StepFactory> = Arc::new(DefaultStepFactory {
        workflow,
        global: self.config.global.clone(),
        project_root: Some(target_dir.clone()),
        resolver: Arc::clone(&self.config.resolver),
        mode_str: mode.as_str().into(),
        mcp_registry: Arc::clone(&self.config.mcp_registry),
        system_prompt_suffix: None,
        dispatcher: None,
    });

    let handle = self
        .inner
        .start(
            WorkflowRunOpts {
                workflow_path,
                vars: inputs,
            },
            factory,
        )
        .await?;
    Ok(handle.run_id)
}
```

Then replace the body of the existing `start_workflow` with a delegating call:

```rust
pub async fn start_workflow(
    &self,
    workflow_path: PathBuf,
) -> Result<String, rupu_orchestrator::executor::ExecutorError> {
    let workspace_path = self.config.project_root.clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    self.start_workflow_with_opts(
        workflow_path,
        Default::default(),
        crate::launcher::LauncherMode::Ask,
        workspace_path,
    ).await
}
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 4: Verify existing tests pass**

Run: `cargo test -p rupu-app`
Expected: existing tests still pass (the wrapper preserves behavior).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/executor/mod.rs
git commit -m "feat(rupu-app): AppExecutor::start_workflow_with_opts (inputs + mode + target)"
```

---

### Task 5: `clone_repo_ref` async wrapper + cache dir

**Files:**
- Modify: `crates/rupu-app/src/launcher/clone.rs`
- Modify: `crates/rupu-app/src/workspace/storage.rs` (add `clones_dir()`)

- [ ] **Step 1: Add `clones_dir()` helper**

In `crates/rupu-app/src/workspace/storage.rs`, after the existing `workspaces_dir()` helper, add:

```rust
/// Returns `<cache>/rupu.app/clones/`. Created on first use.
pub fn clones_dir() -> Result<PathBuf> {
    let proj = directories::ProjectDirs::from("dev", "rupu", "rupu.app")
        .ok_or_else(|| anyhow::anyhow!("no platform cache dir"))?;
    let dir = proj.cache_dir().join("clones");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
```

- [ ] **Step 2: Implement the launcher's clone wrapper**

Replace `crates/rupu-app/src/launcher/clone.rs` with:

```rust
//! Async wrapper around `rupu_scm::clone_repo_ref`. Constructs a
//! ULID-suffixed tempdir under `~/Library/Caches/rupu.app/clones/`,
//! parses the RepoRef from a user-typed string, calls the connector,
//! and returns the populated path.

use std::path::PathBuf;
use std::sync::Arc;

use rupu_scm::{Platform, Registry, RepoRef};

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("clone failed: {0}")]
    Scm(#[from] rupu_scm::CloneError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn clone_repo_ref(
    registry: Arc<Registry>,
    user_input: &str,
) -> Result<PathBuf, CloneError> {
    let r = parse_repo_ref(user_input)?;
    let root = crate::workspace::storage::clones_dir()
        .map_err(|e| CloneError::Io(std::io::Error::other(e.to_string())))?;
    let id = ulid::Ulid::new().to_string();
    let dir = root.join(id);
    std::fs::create_dir_all(&dir)?;
    rupu_scm::clone_repo_ref(&registry, &r, &dir).await?;
    Ok(dir)
}

/// Parse `<platform>:<owner>/<repo>[@<ref>]` into a `RepoRef`. The
/// `@<ref>` segment is currently silently dropped — D-4 always clones
/// HEAD. Adding ref-aware cloning is a future polish task.
pub fn parse_repo_ref(s: &str) -> Result<RepoRef, CloneError> {
    let (platform_str, rest) = s
        .split_once(':')
        .ok_or_else(|| CloneError::Parse(format!("missing ':' separator in '{s}'")))?;
    let platform = match platform_str.to_lowercase().as_str() {
        "github" | "gh" => Platform::Github,
        "gitlab" | "gl" => Platform::Gitlab,
        other => return Err(CloneError::Parse(format!("unknown platform '{other}'"))),
    };
    let rest = rest.split_once('@').map(|(a, _b)| a).unwrap_or(rest);
    let (owner, repo) = rest
        .split_once('/')
        .ok_or_else(|| CloneError::Parse(format!("missing '/' in '{rest}'")))?;
    Ok(RepoRef {
        platform,
        owner: owner.into(),
        repo: repo.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_owner_repo() {
        let r = parse_repo_ref("github:foo/bar").expect("parse");
        assert!(matches!(r.platform, Platform::Github));
        assert_eq!(r.owner, "foo");
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn parse_drops_ref_suffix() {
        let r = parse_repo_ref("github:foo/bar@main").expect("parse");
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn parse_rejects_unknown_platform() {
        assert!(parse_repo_ref("bitbucket:foo/bar").is_err());
    }

    #[test]
    fn parse_rejects_missing_colon() {
        assert!(parse_repo_ref("foo/bar").is_err());
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rupu-app --lib launcher::clone`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/launcher/clone.rs crates/rupu-app/src/workspace/storage.rs
git commit -m "feat(rupu-app): clone_repo_ref launcher wrapper + clones_dir helper"
```

---

### Task 6: 7-day clone-cache GC

**Files:**
- Modify: `crates/rupu-app/src/workspace/storage.rs`
- Modify: `crates/rupu-app/src/main.rs`

- [ ] **Step 1: Add the GC helper**

In `crates/rupu-app/src/workspace/storage.rs`:

```rust
/// Best-effort sweep: delete `clones_dir()` entries older than 7 days.
/// Logs failures via tracing but never propagates.
pub fn gc_clones_dir() {
    let Ok(dir) = clones_dir() else { return; };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 24 * 3600);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "gc_clones_dir: read_dir failed");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if mtime < cutoff {
            if let Err(e) = std::fs::remove_dir_all(&path) {
                tracing::warn!(error = %e, path = %path.display(), "gc_clones_dir: remove failed");
            }
        }
    }
}
```

- [ ] **Step 2: Call from `main.rs`**

In `crates/rupu-app/src/main.rs`, after the workspace is opened (before the GPUI window opens), add:

```rust
// Best-effort cleanup of stale launcher-clone tempdirs.
std::thread::spawn(|| rupu_app::workspace::storage::gc_clones_dir());
```

Spawning a regular thread keeps GC off the GPUI main thread and out of the tokio runtime startup path.

- [ ] **Step 3: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/workspace/storage.rs crates/rupu-app/src/main.rs
git commit -m "feat(rupu-app): best-effort 7-day GC of launcher-clone tempdirs"
```

---

### Task 7: `view::launcher::render` skeleton

**Files:**
- Create: `crates/rupu-app/src/view/launcher.rs`
- Modify: `crates/rupu-app/src/view/mod.rs`

- [ ] **Step 1: Add `pub mod launcher;` to `view/mod.rs`**

In `crates/rupu-app/src/view/mod.rs`, alphabetical position.

- [ ] **Step 2: Create the render skeleton**

`crates/rupu-app/src/view/launcher.rs`:

```rust
//! Launcher sheet — GPUI render for `LauncherState`. Pure function;
//! all interactions dispatch through callbacks injected by the window.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{div, prelude::*, px, AnyElement, IntoElement, Rgba, SharedString};

use crate::launcher::{CloneStatus, LauncherMode, LauncherState, LauncherTarget};
use crate::palette;

pub type InputChangeCb = Arc<dyn Fn(String, String, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type ModeChangeCb = Arc<dyn Fn(LauncherMode, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type TargetChangeCb = Arc<dyn Fn(LauncherTarget, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type RunCb = Arc<dyn Fn(&mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type CloseCb = Arc<dyn Fn(&mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;

pub fn render(
    state: &LauncherState,
    on_input_change: InputChangeCb,
    on_mode_change: ModeChangeCb,
    on_target_change: TargetChangeCb,
    on_run: RunCb,
    on_close: CloseCb,
) -> AnyElement {
    // Backdrop: full-window overlay with the sheet centered.
    let _ = (on_input_change, on_mode_change, on_target_change, on_run, on_close);
    let sheet = render_sheet(state);

    div()
        .absolute()
        .inset_0()
        .bg(palette::BG_OVERLAY)
        .flex()
        .items_center()
        .justify_center()
        .child(sheet)
        .into_any_element()
}

fn render_sheet(state: &LauncherState) -> AnyElement {
    let mut sheet = div()
        .w(px(560.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .bg(palette::BG_PRIMARY)
        .border_1()
        .border_color(palette::BORDER)
        .px(px(24.0))
        .py(px(20.0));

    // Header
    sheet = sheet.child(render_header(state));

    // Inputs form
    sheet = sheet.child(render_inputs_form(state));

    // Footer (mode + target + Run button)
    sheet = sheet.child(render_footer(state));

    // Error band (validation or clone failure)
    if let Some(err) = &state.validation {
        sheet = sheet.child(
            div()
                .text_color(palette::FAILED)
                .text_sm()
                .child(err.message.clone()),
        );
    } else if let LauncherTarget::Clone { status: CloneStatus::Failed(msg), .. } = &state.target {
        sheet = sheet.child(
            div()
                .text_color(palette::FAILED)
                .text_sm()
                .child(format!("clone failed: {msg}")),
        );
    }

    sheet.into_any_element()
}

fn render_header(state: &LauncherState) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .child(
            div()
                .flex_grow()
                .text_color(palette::TEXT_PRIMARY)
                .text_lg()
                .child(state.workflow.name.clone()),
        )
        .into_any_element()
}

fn render_inputs_form(state: &LauncherState) -> AnyElement {
    let mut form = div().flex().flex_col().gap(px(8.0));
    for (name, _def) in &state.workflow.inputs {
        // Task 8 fleshes this out with per-widget rendering.
        // For now, render a label so the sheet structure is testable.
        form = form.child(
            div()
                .text_color(palette::TEXT_PRIMARY)
                .child(SharedString::from(name.clone())),
        );
    }
    form.into_any_element()
}

fn render_footer(state: &LauncherState) -> AnyElement {
    let _ = state;
    // Task 8 fleshes this out.
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .child(div().flex_grow())
        .child(div().px(px(12.0)).py(px(6.0)).child("Run"))
        .into_any_element()
}
```

This is a placeholder skeleton — Task 8 fills in the per-widget rendering. `palette::BG_OVERLAY` may need to be added to `crates/rupu-app/src/palette.rs` (semi-transparent dark RGBA).

- [ ] **Step 3: Add `BG_OVERLAY` to the palette if missing**

In `crates/rupu-app/src/palette.rs`, alongside existing color constants:

```rust
pub const BG_OVERLAY: Rgba = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.6 };
```

(Match the existing `Rgba` import + style; if the palette uses a different color type, adapt.)

- [ ] **Step 4: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/view/launcher.rs crates/rupu-app/src/view/mod.rs crates/rupu-app/src/palette.rs
git commit -m "feat(rupu-app): launcher render skeleton (header + form stub + footer stub)"
```

---

### Task 8: Per-widget inputs form + mode/target controls

**Files:**
- Modify: `crates/rupu-app/src/view/launcher.rs`

- [ ] **Step 1: Render text widget for `InputType::String`**

In `render_inputs_form`, replace the placeholder per-row code with per-widget rendering. Use the existing GPUI text-input primitive — search the codebase for how D-1/D-3 handle text inputs. If GPUI doesn't have a native text input in this pinned version, use a click-to-edit pattern with a buffered value; minimal version: show the current value in a styled `div` with a hover state.

The simplest functional pattern for D-4 (matching what GPUI commonly provides):

```rust
for (name, def) in &state.workflow.inputs {
    let current = state.inputs.get(name).cloned().unwrap_or_default();
    let row = match def.ty {
        rupu_orchestrator::InputType::String if !def.allowed.is_empty() => {
            render_select_row(name, &def.allowed, &current, def.required, on_input_change.clone())
        }
        rupu_orchestrator::InputType::String => {
            render_text_row(name, &current, def.required, on_input_change.clone())
        }
        rupu_orchestrator::InputType::Int => {
            render_number_row(name, &current, def.required, on_input_change.clone())
        }
        rupu_orchestrator::InputType::Bool => {
            render_checkbox_row(name, &current, def.required, on_input_change.clone())
        }
    };
    form = form.child(row);
}
```

Implement each `render_*_row` helper as a function that returns `AnyElement`. Each helper accepts the input name, current value, required flag, and the `InputChangeCb` callback. On change, the callback fires with `(name, new_value)`.

Use whatever input primitive GPUI exposes. If text input is not directly available, a workable substitute for D-4 is `div().on_click(...)` that opens a separate input-capture overlay; defer to a polish pass.

- [ ] **Step 2: Render mode dropdown + target dropdown + Run button in `render_footer`**

```rust
fn render_footer(
    state: &LauncherState,
    on_mode_change: ModeChangeCb,
    on_target_change: TargetChangeCb,
    on_run: RunCb,
) -> AnyElement {
    let can_run = state.can_run();
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .items_center()
        .child(render_mode_picker(state.mode, on_mode_change))
        .child(render_target_picker(&state.target, on_target_change))
        .child(div().flex_grow())
        .child(render_run_button(can_run, on_run))
        .into_any_element()
}
```

Implement `render_mode_picker` as a 3-option dropdown (Ask / Bypass / Read-only). `render_target_picker` as a 3-option dropdown (This workspace / Pick directory… / Clone repo…); when target is `Clone`, show an inline text field for the RepoRef next to the dropdown. `render_run_button` greys out when `!can_run`.

- [ ] **Step 3: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 4: Visual sanity check**

Optional: run `cargo run --bin rupu-app` and trigger the launcher (Task 9 wires it). For Task 8 alone, just verify it compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/view/launcher.rs
git commit -m "feat(rupu-app): launcher per-widget rendering + mode/target/run controls"
```

---

### Task 9: `WorkspaceWindow.launcher` field + render overlay

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs`

- [ ] **Step 1: Add the field**

In `crates/rupu-app/src/window/mod.rs`, in the `WorkspaceWindow` struct (~line 20), add:

```rust
pub struct WorkspaceWindow {
    pub workspace: Workspace,
    pub app_executor: Arc<AppExecutor>,
    pub run_model: Option<crate::run_model::RunModel>,
    pub transcript_lines: Vec<TranscriptLine>,
    /// `Some` when the launcher sheet is open. None otherwise.
    pub launcher: Option<crate::launcher::LauncherState>,
    /// The workflow row most recently focused in the sidebar. ⌘R uses
    /// this. `None` means no row has focus.
    pub focused_workflow: Option<PathBuf>,
}
```

Update the constructor (the `cx.new(|_cx| WorkspaceWindow { ... })` block) to initialize the new fields as `None`.

- [ ] **Step 2: Add open/close helpers**

After the existing `handle_run_clicked` method, add:

```rust
pub fn open_launcher(&mut self, workflow_path: PathBuf, cx: &mut Context<Self>) {
    let yaml = match std::fs::read_to_string(&workflow_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, path = %workflow_path.display(), "open_launcher: read_to_string failed");
            return;
        }
    };
    let workflow = match rupu_orchestrator::Workflow::parse(&yaml) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, path = %workflow_path.display(), "open_launcher: parse failed");
            return;
        }
    };
    self.launcher = Some(crate::launcher::LauncherState::new(workflow_path, workflow));
    cx.notify();
}

pub fn close_launcher(&mut self, cx: &mut Context<Self>) {
    self.launcher = None;
    cx.notify();
}

pub fn handle_launcher_input_change(&mut self, name: String, value: String, cx: &mut Context<Self>) {
    if let Some(state) = self.launcher.as_mut() {
        state.set_input(&name, value);
        state.revalidate();
        cx.notify();
    }
}

pub fn handle_launcher_mode_change(&mut self, mode: crate::launcher::LauncherMode, cx: &mut Context<Self>) {
    if let Some(state) = self.launcher.as_mut() {
        state.mode = mode;
        cx.notify();
    }
}

pub fn handle_launcher_target_change(&mut self, target: crate::launcher::LauncherTarget, cx: &mut Context<Self>) {
    if let Some(state) = self.launcher.as_mut() {
        state.target = target;
        cx.notify();
    }
}
```

- [ ] **Step 3: Render the overlay**

In `Render::render` (the `impl Render for WorkspaceWindow` block, around line 275), after building the main layout, stack the launcher overlay above it conditionally:

```rust
fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let main_layout = /* existing render body */;

    if let Some(state) = self.launcher.clone() {
        let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let w1 = weak.clone();
        let on_input_change: crate::view::launcher::InputChangeCb = Arc::new(
            move |name, value, _w, cx| {
                let _ = w1.update(cx, |this, cx| this.handle_launcher_input_change(name, value, cx));
            },
        );
        let w2 = weak.clone();
        let on_mode_change: crate::view::launcher::ModeChangeCb = Arc::new(
            move |mode, _w, cx| {
                let _ = w2.update(cx, |this, cx| this.handle_launcher_mode_change(mode, cx));
            },
        );
        let w3 = weak.clone();
        let on_target_change: crate::view::launcher::TargetChangeCb = Arc::new(
            move |target, _w, cx| {
                let _ = w3.update(cx, |this, cx| this.handle_launcher_target_change(target, cx));
            },
        );
        let w4 = weak.clone();
        let on_run: crate::view::launcher::RunCb = Arc::new(move |_w, cx| {
            let _ = w4.update(cx, |this, cx| this.handle_launcher_run(cx));
        });
        let w5 = weak.clone();
        let on_close: crate::view::launcher::CloseCb = Arc::new(move |_w, cx| {
            let _ = w5.update(cx, |this, cx| this.close_launcher(cx));
        });

        div()
            .relative()
            .size_full()
            .child(main_layout)
            .child(crate::view::launcher::render(
                &state,
                on_input_change,
                on_mode_change,
                on_target_change,
                on_run,
                on_close,
            ))
    } else {
        main_layout.into_any_element()
    }
}
```

Match the existing `impl Render` shape — the actual return type may be `impl IntoElement` or `AnyElement`; pick what compiles.

- [ ] **Step 4: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs
git commit -m "feat(rupu-app): WorkspaceWindow.launcher field + open/close + render overlay"
```

---

### Task 10: Dispatch flow (`handle_launcher_run`)

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs`

- [ ] **Step 1: Implement the dispatch handler**

Add to the `impl WorkspaceWindow` block:

```rust
pub fn handle_launcher_run(&mut self, cx: &mut Context<Self>) {
    let Some(state) = self.launcher.clone() else {
        return;
    };
    if !state.can_run() {
        return;
    }
    match &state.target {
        crate::launcher::LauncherTarget::ThisWorkspace => {
            let target = self.workspace.path.clone();
            self.spawn_run(state, target, cx);
        }
        crate::launcher::LauncherTarget::Directory(path) => {
            let target = path.clone();
            self.spawn_run(state, target, cx);
        }
        crate::launcher::LauncherTarget::Clone { repo_ref, status } => match status {
            crate::launcher::CloneStatus::Done(path) => {
                let target = path.clone();
                self.spawn_run(state, target, cx);
            }
            crate::launcher::CloneStatus::NotStarted | crate::launcher::CloneStatus::Failed(_) => {
                self.spawn_clone_then_run(state, repo_ref.clone(), cx);
            }
            crate::launcher::CloneStatus::InProgress => {
                // No-op; defensive — Run button is disabled in this state.
            }
        },
    }
}

fn spawn_clone_then_run(
    &mut self,
    state: crate::launcher::LauncherState,
    repo_ref: String,
    cx: &mut Context<Self>,
) {
    // Flip status to InProgress immediately so the UI reflects the
    // change before the spawn fires.
    if let Some(s) = self.launcher.as_mut() {
        if let crate::launcher::LauncherTarget::Clone { status, .. } = &mut s.target {
            *status = crate::launcher::CloneStatus::InProgress;
        }
    }
    cx.notify();

    let registry = self.app_executor.config_mcp_registry();
    let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
    cx.spawn(async move |_, cx| {
        let clone_result = crate::launcher::clone::clone_repo_ref(registry, &repo_ref).await;
        match clone_result {
            Ok(path) => {
                let _ = weak.update(cx, |this, cx| {
                    if let Some(s) = this.launcher.as_mut() {
                        if let crate::launcher::LauncherTarget::Clone { status, .. } = &mut s.target {
                            *status = crate::launcher::CloneStatus::Done(path.clone());
                        }
                    }
                    cx.notify();
                    this.spawn_run(state.clone(), path, cx);
                });
            }
            Err(e) => {
                let _ = weak.update(cx, |this, cx| {
                    if let Some(s) = this.launcher.as_mut() {
                        if let crate::launcher::LauncherTarget::Clone { status, .. } = &mut s.target {
                            *status = crate::launcher::CloneStatus::Failed(e.to_string());
                        }
                    }
                    cx.notify();
                });
            }
        }
    })
    .detach();
}

fn spawn_run(
    &mut self,
    state: crate::launcher::LauncherState,
    target_dir: PathBuf,
    cx: &mut Context<Self>,
) {
    let app_exec = self.app_executor.clone();
    let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
    let workflow_path = state.workflow_path.clone();
    let inputs = state.inputs.clone();
    let mode = state.mode;
    cx.spawn(async move |_, cx| {
        match app_exec
            .start_workflow_with_opts(workflow_path.clone(), inputs, mode, target_dir)
            .await
        {
            Ok(run_id) => {
                let _ = weak.update(cx, |this, cx| {
                    this.launcher = None;
                    this.run_model = Some(crate::run_model::RunModel::new(
                        run_id.clone(),
                        workflow_path.clone(),
                    ));
                    cx.notify();
                });
                // Subscribe to the new run's event stream (mirrors
                // handle_run_clicked's existing pattern).
                if let Ok(mut stream) = app_exec.attach(&run_id).await {
                    use futures_util::StreamExt;
                    while let Some(ev) = stream.next().await {
                        let res = weak.update(cx, |this, cx| {
                            if let Some(m) = this.run_model.take() {
                                this.run_model = Some(m.apply(&ev));
                            }
                            cx.notify();
                        });
                        if res.is_err() {
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                let _ = weak.update(cx, |this, cx| {
                    if let Some(s) = this.launcher.as_mut() {
                        s.validation = Some(crate::launcher::ValidationError {
                            message: format!("start failed: {e}"),
                        });
                    }
                    cx.notify();
                });
            }
        }
    })
    .detach();
}
```

- [ ] **Step 2: Expose `mcp_registry` from AppExecutor**

`spawn_clone_then_run` calls `self.app_executor.config_mcp_registry()`. Add the accessor on `AppExecutor` in `crates/rupu-app/src/executor/mod.rs`:

```rust
pub fn config_mcp_registry(&self) -> Arc<rupu_scm::Registry> {
    Arc::clone(&self.config.mcp_registry)
}
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs crates/rupu-app/src/executor/mod.rs
git commit -m "feat(rupu-app): launcher dispatch — direct run, clone-then-run, error feedback"
```

---

### Task 11: Replace `handle_run_clicked` direct-start with `open_launcher`

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs` (around line 234)

- [ ] **Step 1: Rewrite the body**

Replace the existing `handle_run_clicked` body with:

```rust
pub fn handle_run_clicked(&mut self, cx: &mut Context<Self>) {
    let Some(path) = self.current_workflow_path() else {
        return;
    };
    self.open_launcher(path, cx);
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean. No further wiring needed — the toolbar Run button continues to call `handle_run_clicked` which now opens the launcher.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs
git commit -m "feat(rupu-app): toolbar Run button opens launcher instead of dispatching directly"
```

---

### Task 12: `focused_workflow` tracking + sidebar click handlers

**Files:**
- Modify: `crates/rupu-app/src/window/sidebar.rs`
- Modify: `crates/rupu-app/src/window/mod.rs` (focused_workflow field + handler)

- [ ] **Step 1: Add focus-tracking handler to WorkspaceWindow**

```rust
pub fn handle_workflow_clicked(&mut self, path: PathBuf, cx: &mut Context<Self>) {
    self.focused_workflow = Some(path);
    cx.notify();
}

pub fn handle_workflow_right_clicked(&mut self, path: PathBuf, cx: &mut Context<Self>) {
    self.focused_workflow = Some(path.clone());
    self.open_launcher(path, cx);
}
```

- [ ] **Step 2: Wire click + right-click handlers in sidebar.rs**

In `crates/rupu-app/src/window/sidebar.rs` (around line 98 where workflow rows are rendered), add click + right-click handlers. Pattern (sidebar signature may need a `WeakEntity<WorkspaceWindow>` parameter to dispatch back to the window):

```rust
// Pass a weak entity to the sidebar render so each row can dispatch
// back into the window's click handler.
pub fn render(
    workspace: &Workspace,
    active_run_map: &ActiveRunMap,
    on_workflow_click: Arc<dyn Fn(PathBuf, &mut Window, &mut App) + Send + Sync + 'static>,
    on_workflow_right_click: Arc<dyn Fn(PathBuf, &mut Window, &mut App) + Send + Sync + 'static>,
) -> impl IntoElement {
    // ...existing render with each workflow row:
    let path = asset.path.clone();
    let cb_click = on_workflow_click.clone();
    let cb_right = on_workflow_right_click.clone();
    div()
        .id(SharedString::from(format!("wf-{}", asset.path.display())))
        .text_xs()
        .text_color(palette::TEXT_MUTED)
        .child(asset.name.clone())
        .cursor_pointer()
        .on_click(move |_, w, cx| cb_click(path.clone(), w, cx))
        // GPUI right-click — use whatever the pinned version exposes.
        // If `on_mouse_down(Button::Right, ...)` is available, use that.
        // Otherwise fall back to a long-press / Ctrl+click compromise.
}
```

Adapt the GPUI right-click API to what the pinned version supports. If no clean right-click API exists, document the fallback (e.g. Ctrl+click) and use `on_click` with a modifier check.

- [ ] **Step 3: Pass callbacks from WorkspaceWindow into sidebar::render**

In `WorkspaceWindow::render`, construct the callbacks via `weak_entity` (same pattern as approval callbacks), then pass them into `sidebar::render(...)`.

- [ ] **Step 4: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/window/
git commit -m "feat(rupu-app): sidebar click sets focused_workflow; right-click opens launcher"
```

---

### Task 13: `LaunchSelected` action + ⌘R binding

**Files:**
- Modify: `crates/rupu-app/src/menu/app_menu.rs`
- Modify: `crates/rupu-app/src/window/mod.rs`

- [ ] **Step 1: Declare the action**

In `crates/rupu-app/src/menu/app_menu.rs`, find the existing `actions!` invocation and append `LaunchSelected`:

```rust
actions!(rupu_app, [..., LaunchSelected]);
```

Add a key binding alongside other bindings:

```rust
cx.bind_keys(vec![
    KeyBinding::new("cmd-r", LaunchSelected, None),
    // ...existing bindings...
]);
```

- [ ] **Step 2: Wire the handler in WorkspaceWindow::render**

In the `Render::render` body, attach an `on_action` for `LaunchSelected`:

```rust
.on_action(cx.listener(|this, _: &LaunchSelected, _, cx| {
    if let Some(path) = this.focused_workflow.clone() {
        this.open_launcher(path, cx);
    }
}))
```

If `cx.listener`'s 4-arg signature differs in this GPUI version, match the pattern from D-3's `ApproveFocused`/`RejectFocused` handlers in the same file.

- [ ] **Step 3: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/menu/app_menu.rs crates/rupu-app/src/window/mod.rs
git commit -m "feat(rupu-app): ⌘R LaunchSelected action opens launcher for focused workflow"
```

---

### Task 14: NSOpenPanel "Pick directory" wiring

**Files:**
- Modify: `crates/rupu-app/src/view/launcher.rs` (target picker → triggers picker)
- Modify: `crates/rupu-app/src/window/mod.rs` (`handle_launcher_pick_directory`)

- [ ] **Step 1: Add `handle_launcher_pick_directory` to WorkspaceWindow**

```rust
pub fn handle_launcher_pick_directory(&mut self, cx: &mut Context<Self>) {
    let Some(path) = crate::menu::app_menu::pick_directory_modal("Pick target directory") else {
        return; // User cancelled
    };
    if let Some(state) = self.launcher.as_mut() {
        state.target = crate::launcher::LauncherTarget::Directory(path);
        cx.notify();
    }
}
```

`pick_directory_modal` is the existing helper in `crates/rupu-app/src/menu/app_menu.rs` (NSOpenPanel wrapper used for workspace-open). Make it `pub` if not already.

- [ ] **Step 2: Wire the picker click from `view::launcher::render`**

In the target dropdown, when the user clicks "Pick directory…", dispatch a callback that calls `handle_launcher_pick_directory`. Add a `PickDirCb` callback type to `view/launcher.rs`:

```rust
pub type PickDirCb = Arc<dyn Fn(&mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
```

Thread it through `render` as an additional parameter. In `WorkspaceWindow::render`, construct it as:

```rust
let w6 = weak.clone();
let on_pick_dir: crate::view::launcher::PickDirCb = Arc::new(move |_w, cx| {
    let _ = w6.update(cx, |this, cx| this.handle_launcher_pick_directory(cx));
});
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p rupu-app`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/view/launcher.rs crates/rupu-app/src/window/mod.rs crates/rupu-app/src/menu/app_menu.rs
git commit -m "feat(rupu-app): launcher Pick directory uses existing NSOpenPanel"
```

---

### Task 15: `app-smoke` extension + workspace gates

**Files:**
- Modify: `Makefile`

- [ ] **Step 1: Extend the make target**

Append to the existing `app-smoke` target:

```make
	@echo "  · launcher_state test"
	@cargo test -p rupu-app --test launcher_state
	@echo "  · clone helper test"
	@cargo test -p rupu-scm --test clone
```

- [ ] **Step 2: Run app-smoke**

Run: `make app-smoke`
Expected: passes; the new tests are part of the smoke suite.

- [ ] **Step 3: Run workspace gates**

```bash
cargo fmt --check
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30
```

Expected: same pass/fail baseline as `origin/main`. Pre-existing `samples_byte_match_dogfooded_files` + `autoflow.rs`/`autoflow_runtime.rs` clippy issues are not yours to fix.

- [ ] **Step 4: Commit**

```bash
git add Makefile
git commit -m "test: app-smoke includes launcher_state + clone helper tests"
```

---

### Task 16: CLAUDE.md update

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add Plan 4 to the Read first section**

```markdown
- Slice D Plan 4 (launcher, operator-complete): `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-4-launcher.md`
- Slice D Plan 4 spec: `docs/superpowers/specs/2026-05-12-rupu-slice-d-plan-4-launcher-design.md`
```

- [ ] **Step 2: Update the `rupu-app` crate description**

Add to the existing `rupu-app` paragraph:

```markdown
The launcher sheet (D-4) is the canonical entry to dispatch a workflow from the app — toolbar Run button, ⌘R on a focused sidebar row, or right-click → Run all open the same floating sheet (inputs form, mode picker Ask/Bypass/Read-only, target picker workspace/directory/RepoRef-clone). Clones land in `~/Library/Caches/rupu.app/clones/<ULID>/`; a best-effort 7-day sweep runs on startup.
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md — Slice D Plan 4 pointer + launcher behavior"
```

---

## Self-review

| Spec section | Plan task(s) |
|---|---|
| `LauncherState` + enums | 1, 2 |
| Validation on keystroke | 2 |
| `clone_repo_ref` lift from rupu-cli | 3 |
| `clone_repo_ref` async wrapper + cache dir | 5 |
| 7-day cache GC | 6 |
| `AppExecutor::start_workflow_with_opts` | 4 |
| Launcher render skeleton | 7 |
| Per-widget inputs form + mode/target controls | 8 |
| `WorkspaceWindow.launcher` field + overlay render | 9 |
| Dispatch flow (direct + clone-then-run) | 10 |
| Toolbar Run → open launcher | 11 |
| Sidebar focus + right-click → launcher | 12 |
| ⌘R `LaunchSelected` | 13 |
| NSOpenPanel "Pick directory" | 14 |
| app-smoke + gates | 15 |
| CLAUDE.md | 16 |

Acceptance criteria from the spec are all covered:
- (1) launcher opens with prefilled defaults → Task 2.
- (2) fill + Run → live Graph → Tasks 9–11.
- (3) ⌘R from focused sidebar row → Task 13.
- (4) right-click → context-menu-or-direct → Task 12.
- (5) Clone target end-to-end → Tasks 3, 5, 10.
- (6) CLI `rupu run --tmp` still works → Task 3.
- (7) workspace gates pass → Task 15.
