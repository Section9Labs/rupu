# Slice D Plan 4 — Launcher (design)

> **Status:** design ready · brainstorm complete · awaiting plan write-up

## Goal

Make `rupu.app` operator-complete. The user opens a workspace, triggers a launcher sheet for a workflow (toolbar Run button / ⌘R / right-click → Run), fills inputs, picks mode + target (workspace dir / browse directory / clone a `RepoRef` to tempdir), clicks Run, and the main area switches to the new run's live Graph view with the D-3 drill-down + approval UI taking over. Headline outcome from the parent spec §10: "rupu.app is now self-sufficient for open workspace → run workflow → watch + approve."

## Scope

**In scope:**

- Floating launcher sheet (modal-style, anchored center of window, dims rest of UI).
- Inputs form with type-appropriate widgets per `InputDef`: text, select (when `allowed:` is set), number (Int), checkbox (Bool). Required fields highlighted, defaults pre-filled.
- Mode picker: Ask / Bypass / Read-only. Default: Ask.
- Target picker with three options: This workspace (default), Pick directory (`NSOpenPanel`), Clone repo (`RepoRef` text input → async clone to tempdir).
- Three triggers: Graph toolbar Run button, ⌘R keyboard shortcut, right-click → Run on sidebar workflow rows.
- `AppExecutor::start_workflow_with_opts(workflow_path, inputs, mode, target_dir)` extended entry point; existing `start_workflow(path)` becomes a thin wrapper that calls it with sensible defaults.
- `RepoRef` clone helper extracted from `rupu-cli`'s `--tmp` flow into a shared location so both CLI and app share it.
- Best-effort cleanup of cached clones (older than 7 days) on app start.

**Out of scope (deliberate, deferred):**

- **Tab strip** — D-4 stays single-tab. A new run replaces the current main-area Graph view. Real tabs (Workflow / Run / File / Issue / Agent / Repo / Autoflow per spec §6.3) land in D-5/D-6 alongside YAML and Canvas views.
- **Recent runs list / sidebar history** — out of scope; lands with Transcript view (D-8).
- **Cancel button** — `WorkflowExecutor::cancel` exists on the trait; no UI surface in D-4.
- **Per-field inline validation indicators** (red borders, real-time field hints) — single error message at the bottom of the form on submit; field-level polish deferred.
- **Persistent input recall** — last-used input values per workflow are not remembered between sessions.
- **Mode persistence** — launcher always opens with mode=Ask; user picks per-run.
- **Persistent clone cache + dedup** — every Clone selection produces a fresh tempdir.
- **Cross-workspace launcher** — launcher only operates against the currently-open workspace.

## Background

- Slice D spec: `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md` §10 lists D-4 as the sub-slice that ships "Workflow inputs form, run button, new tab opens streaming. **Operator-complete: rupu.app is now self-sufficient for open workspace → run workflow → watch + approve.**" Per matt's scope decisions during brainstorming, D-4 ships the launcher + dispatch but stays single-tab; the spec's tab-strip work moves to D-5.
- D-1 (workspace shell, PR #192), D-2 (Graph view, PR #196), and D-3 (live executor + Graph pulse, PR #200) are all merged on `main`. The post-D-3 refactor PRs #201 and #202 lifted `DefaultStepFactory` out of `rupu-cli` and wired it into `rupu-app`, so the app already starts production workflows when the existing `Run` button is clicked.
- D-3 left a known limitation: the Run button calls `AppExecutor::start_workflow(path)` directly with no inputs, no mode picker, no target picker. D-4 replaces that direct path with the launcher sheet.

## Architecture

The launcher sits between the user's click and `AppExecutor::start_workflow_with_opts`. Three layers:

1. **State** — `LauncherState` on `WorkspaceWindow` (`Option<LauncherState>`). Pure data. Mutated by user input + clone task. `None` means no sheet open.
2. **Render** — `view::launcher::render(&LauncherState)` produces the GPUI sheet overlay. Drives all interactions through `WorkspaceWindow` callbacks (same callback-injection pattern D-3 used for approval buttons).
3. **Dispatch** — when the user clicks Run with a valid form, `WorkspaceWindow::handle_launcher_run()` spawns a tokio task that either (a) runs a clone first (Clone target) or (b) goes straight to `app_executor.start_workflow_with_opts(...)`, then closes the sheet and updates `run_model`.

The cloned-repo flow:

1. User picks target=Clone, types a `RepoRef`, clicks Run.
2. State flips to `CloneStatus::InProgress`. Sheet re-renders showing a spinner.
3. Tokio task calls `clone_repo_ref(repo_ref, resolver, mcp_registry)`.
4. On success → `CloneStatus::Done(path)`; the same tokio task continues to `start_workflow_with_opts(path, inputs, mode, cloned_path)`. Sheet closes once the run starts.
5. On clone failure → `CloneStatus::Failed(msg)`; sheet stays open, shows error, Run button re-enabled.

The existing D-3 event subscription on `WorkspaceWindow` does the rest — the new run's events stream into `run_model`, the Graph view paints, the drill-down + approval UI react.

## Launcher state machine

```rust
// crates/rupu-app/src/launcher/state.rs

use std::collections::BTreeMap;
use std::path::PathBuf;

use rupu_orchestrator::Workflow;

pub struct LauncherState {
    pub workflow_path: PathBuf,
    pub workflow: Workflow,
    pub inputs: BTreeMap<String, String>,
    pub mode: LauncherMode,
    pub target: LauncherTarget,
    pub validation: Option<ValidationError>,
}

#[derive(Debug, Clone, Copy)]
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
    Clone {
        repo_ref: String,
        status: CloneStatus,
    },
}

#[derive(Debug, Clone)]
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
```

- `LauncherState::new(path, workflow)` pre-fills `inputs` from each input's `default` (when present), leaves `mode = Ask`, `target = ThisWorkspace`, `validation = None`.
- On every keystroke the launcher re-runs `Workflow::resolve_inputs(&self.workflow, &self.inputs)` (existing orchestrator function) and updates `validation`. Cheap operation — type-coercion + enum check + required-check.
- The Run button is disabled when `validation.is_some()` OR `target` is `Clone { status: InProgress, .. }`.

## Components

### `crates/rupu-app/src/launcher/`

- `mod.rs` — module root; `pub mod state; pub mod clone;`
- `state.rs` — `LauncherState` + enums (above).
- `clone.rs` — `clone_repo_ref` helper; bridges to `rupu-scm`.

### `crates/rupu-app/src/view/launcher.rs`

GPUI render function:

```rust
pub fn render(
    state: &LauncherState,
    on_input_change: Arc<dyn Fn(String /* input_name */, String /* new_value */)>,
    on_mode_change: Arc<dyn Fn(LauncherMode)>,
    on_target_change: Arc<dyn Fn(LauncherTarget)>,
    on_run: Arc<dyn Fn()>,
    on_close: Arc<dyn Fn()>,
) -> AnyElement
```

Layout (top to bottom inside the centered sheet, dimmed backdrop):

1. **Header row** — workflow name (large), workflow description (subtitle from YAML), close button (`✕`).
2. **Inputs form** — one row per declared `InputDef`. Widget by type:
   - `InputType::String` with empty `allowed` → text input.
   - `InputType::String` with non-empty `allowed` → select dropdown.
   - `InputType::Int` → text input restricted to digits + optional leading `-`.
   - `InputType::Bool` → checkbox.
   - Required fields show a small red dot next to the label.
   - `description` (when present) renders as muted text under the widget.
3. **Footer row** — three controls in a flex row:
   - Mode dropdown: Ask / Bypass / Read-only.
   - Target dropdown: This workspace / Pick directory… / Clone repo….
     - Pick directory → triggers `NSOpenPanel` via existing `objc2-app-kit` wiring (D-1 already uses NSOpenPanel for workspace open).
     - Clone repo → expands an inline `RepoRef` text field next to the dropdown.
   - Run button (right-aligned, primary color). Disabled while validation fails or clone is `InProgress`.
4. **Error band** (only when `validation.is_some()` or clone failed) — single-line red text at the bottom of the form.

The sheet itself is positioned absolute over the window's main area; the rest of the window dims behind it but stays interactive only for the Escape key (which closes the sheet).

### `crates/rupu-app/src/window/mod.rs` additions

- New field `launcher: Option<LauncherState>` on `WorkspaceWindow`. None → no sheet.
- New field `focused_workflow: Option<PathBuf>` for ⌘R support.
- Helper `WorkspaceWindow::open_launcher(workflow_path, cx)` — parses the YAML, builds `LauncherState`, sets `self.launcher = Some(...)`, calls `cx.notify()`.
- Helper `WorkspaceWindow::close_launcher(cx)` — sets `self.launcher = None`.
- Helper `WorkspaceWindow::handle_launcher_run(cx)` — described in the Dispatch flow below.
- Helper `WorkspaceWindow::handle_launcher_input_change(name, value, cx)` — mutates state, re-runs validation.
- Helper `WorkspaceWindow::handle_launcher_mode_change`, `WorkspaceWindow::handle_launcher_target_change` — mutate state.
- `Render::render` checks `self.launcher.is_some()` and stacks the launcher overlay above the existing main area.

### `crates/rupu-app/src/launcher/clone.rs`

```rust
pub async fn clone_repo_ref(
    repo_ref: &str,
    resolver: Arc<rupu_auth::KeychainResolver>,
    mcp_registry: Arc<rupu_scm::Registry>,
) -> Result<PathBuf, CloneError>;

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("clone failed: {0}")]
    Failed(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}
```

Internally:
1. Parse the `repo_ref` via `rupu_scm::RepoRef::parse`.
2. Look up the appropriate `RepoConnector` via the `Registry`.
3. Create a tempdir at `<cache_dir>/rupu.app/clones/<ULID>/`.
4. Call `connector.clone_to(...)` (matching `rupu-cli`'s `--tmp` clone helper — see "Refactor obligation" below).
5. Return the tempdir path.

`<cache_dir>` resolves to `~/Library/Caches` on macOS via the `directories` crate (already a dep).

### Refactor obligation: lift `rupu run --tmp` clone helper

The `rupu-cli` `--tmp` flow has clone logic today (search `crates/rupu-cli/src/cmd/workflow.rs` for `tmp` / `clone` references). If the helper is a `non-pub fn` in `workflow.rs`, this plan moves it into a shared crate so both CLI and app use it. Candidate homes:

1. `rupu-scm` — fits semantically since it owns `RepoConnector` and `RepoRef`. **Preferred.**
2. `rupu-orchestrator` — also a reasonable host since `DefaultStepFactory` already lives there.

The lift is in scope for D-4. After this work, both `rupu run --tmp <repo-ref>` and the app's launcher Clone target call the same helper.

## Dispatch flow

`WorkspaceWindow::handle_launcher_run` (called when the user clicks Run with `validation = None`):

```text
match state.target:
  ThisWorkspace ->
    final_target = self.workspace.path.clone()
    spawn_run(workflow_path, inputs, mode, final_target)
  Directory(path) ->
    final_target = path.clone()
    spawn_run(workflow_path, inputs, mode, final_target)
  Clone { repo_ref, status: NotStarted } ->
    flip status to InProgress
    spawn clone task:
      Ok(path) -> flip status to Done(path); spawn_run(workflow_path, inputs, mode, path)
      Err(e)   -> flip status to Failed(e.to_string()); leave sheet open
  Clone { status: InProgress } ->
    no-op (button is disabled anyway; defensive guard)
  Clone { status: Done(path) } ->
    spawn_run(workflow_path, inputs, mode, path)
  Clone { status: Failed(_) } ->
    flip back to NotStarted; user clicks Run again to retry
```

`spawn_run(path, inputs, mode, target)`:

1. Call `app_executor.start_workflow_with_opts(path, inputs, mode, target).await`.
2. On `Ok(run_id)` → `self.launcher = None`; `self.run_model = Some(RunModel::new(run_id, path))`; the existing D-3 event-stream subscription drains events into `run_model`.
3. On `Err(e)` → keep sheet open, show error in the validation band.

The clone task and the run-start task can be the same outer task; the launcher state mutations happen via `WeakEntity::update` (same pattern D-3 used).

## `AppExecutor::start_workflow_with_opts` signature

```rust
pub async fn start_workflow_with_opts(
    &self,
    workflow_path: PathBuf,
    inputs: BTreeMap<String, String>,
    mode: LauncherMode,
    target_dir: PathBuf,
) -> Result<String, rupu_orchestrator::executor::ExecutorError>;
```

Internally:
1. Read + parse the workflow YAML.
2. Build `DefaultStepFactory` with `mode_str: mode.as_str().into()`, `project_root: Some(target_dir.clone())`, `workflow: parsed`, rest pinned to the AppExecutor's existing config (resolver, mcp_registry, global, system_prompt_suffix, dispatcher).
3. Build `WorkflowRunOpts { workflow_path, vars: inputs }`.
4. Call `self.inner.start(run_opts, factory).await`.

The existing `start_workflow(path)` becomes:

```rust
pub async fn start_workflow(&self, workflow_path: PathBuf) -> Result<String, ExecutorError> {
    self.start_workflow_with_opts(workflow_path, Default::default(), LauncherMode::Ask, self.workspace_path.clone()).await
}
```

Backward-compatible — callers that haven't migrated still work.

## Triggers + keyboard

### Graph toolbar Run button

D-3 added a Run button on the Graph view toolbar (`crates/rupu-app/src/window/mod.rs::handle_run_clicked`). Today it calls `start_workflow` directly. D-4 changes it to open the launcher:

```rust
fn handle_run_clicked(&mut self, cx: &mut Context<Self>) {
    let Some(path) = self.current_workflow_path() else { return; };
    self.open_launcher(path, cx);
}
```

### ⌘R keyboard shortcut

New GPUI action `LaunchSelected` bound to `Cmd-R`. Registered in `crates/rupu-app/src/menu/app_menu.rs` (D-3 already declares `ApproveFocused` + `RejectFocused` here, same pattern).

```rust
use gpui::actions;
actions!(rupu_app, [..., LaunchSelected]);
```

`WorkspaceWindow::render` attaches:

```rust
.on_action(cx.listener(|this, _: &LaunchSelected, _, cx| {
    if let Some(path) = this.focused_workflow.clone() {
        this.open_launcher(path, cx);
    }
}))
```

### Right-click → Run on sidebar rows

Each sidebar workflow row gains a context-menu handler. GPUI's context menu primitives in the pinned version may need verification — if `ContextMenu::new()` (or equivalent) is unavailable, fall back to: right-click directly opens the launcher (no menu, immediate action).

If the menu is available, it shows one item:

```
Run                                ⌘R
```

Selecting Run calls `self.open_launcher(row.path, cx)`.

### Focus tracking on sidebar rows

`WorkspaceWindow.focused_workflow: Option<PathBuf>` — set when the user clicks a workflow row (or arrow-keys to it, if keyboard nav exists). The sidebar uses GPUI's focus mechanism if available; otherwise a manual click handler updates the field.

## Error handling

- **Workflow YAML parse error on launcher open** — fail-soft. Show a toast at the bottom of the window with the parse error; don't open the launcher.
- **Validation errors during input editing** — render in the error band at the bottom of the form. Specific message per failure type (missing required / not in enum / type mismatch).
- **`AppExecutor::start_workflow_with_opts` error** — keep launcher sheet open, show error in the error band, re-enable Run button.
- **`NSOpenPanel` cancelled by user** — silent; revert target picker to its prior selection.
- **Clone parse error (`RepoRef` malformed)** — render under the RepoRef field, don't start the clone.
- **Clone network error** — flip to `CloneStatus::Failed`; show error band with message; user can retry or change target.
- **Clone succeeds, run start fails** — keep the clone (don't delete the tempdir); show error band with run-start failure.

## Cache hygiene

Cached clones live at `~/Library/Caches/rupu.app/clones/<ULID>/`. On app start, a background task walks this directory and deletes entries with `mtime` older than 7 days. Best-effort; logs failures but never panics. Out of scope: per-`repo_ref` dedup, persistent cache for re-runs, eviction policies based on size.

## Testing strategy

| Layer | What we test | How |
|---|---|---|
| `LauncherState::new` | Defaults pre-fill correctly per `InputDef` | Unit tests |
| `LauncherState` validation | Each `InputType` variant + enum + required + mismatch | Unit tests + insta snapshots |
| `clone_repo_ref` | Happy path + parse error + connector failure | Integration test against a fixture connector (or reuse `rupu-scm`'s mock infra) |
| Dispatch state machine | Each `LauncherTarget` arm flows correctly | Unit test on a `WorkspaceWindow` test harness or pure-function helper |
| GPUI render | Launcher renders for each `InputType` + each `LauncherTarget` variant | Snapshot test with insta or visual smoke via `make app-smoke` |
| End-to-end | Open app, fixture workspace, programmatically open launcher, fill inputs, run → assert `RunModel` populated | `make app-smoke` extension |
| Existing tests | All D-3 tests still pass | `cargo test --workspace` |

## Acceptance criteria

1. Open a workspace with a workflow declaring inputs. Click the Run button in the Graph toolbar → launcher opens with inputs form pre-filled from `default` values.
2. Fill required inputs, leave mode=Ask and target=This workspace. Click Run → sheet closes; main area shows live Graph nodes transitioning through statuses.
3. With a workflow row focused in the sidebar, press ⌘R → launcher opens.
4. Right-click a workflow row → context menu (or direct launcher, depending on GPUI capability) → launcher opens.
5. Select target=Clone repo, enter `github:rupu-test/sample@main` (or any valid `RepoRef`), click Run → progress indicator shown → clone completes → run starts against the cloned tempdir.
6. CLI `rupu run` and `rupu run --tmp <repo-ref>` still work; both use the same lifted clone helper.
7. `cargo test --workspace`, `cargo fmt --check`, `make app-smoke` all pass.

## Implementation phases

The plan author should decompose into roughly these task clusters, in order:

1. `LauncherState` + `LauncherMode` + `LauncherTarget` + `CloneStatus` + unit tests for validation.
2. `AppExecutor::start_workflow_with_opts` signature extension; existing `start_workflow` becomes a thin wrapper.
3. Lift the `rupu run --tmp` clone helper into a shared crate (likely `rupu-scm`); update CLI to call the new helper; verify CLI tests still pass.
4. `launcher::clone::clone_repo_ref` async helper + tests.
5. `view::launcher::render` GPUI element + per-type widgets.
6. `WorkspaceWindow.launcher: Option<LauncherState>` field + open/close/render-overlay wiring.
7. Launcher state mutation handlers (input change, mode change, target change).
8. Dispatch flow (`handle_launcher_run` with the state-machine arms).
9. Replace D-3's `handle_run_clicked` direct-start with `open_launcher`.
10. `WorkspaceWindow.focused_workflow` field + sidebar click-to-focus.
11. `LaunchSelected` GPUI action + ⌘R binding.
12. Right-click context menu (or fallback to direct-action) on sidebar workflow rows.
13. Cache hygiene: 7-day GC sweep on app start.
14. `make app-smoke` extension + workspace gates.
15. `CLAUDE.md` update + Slice D progress note in the slice spec.
