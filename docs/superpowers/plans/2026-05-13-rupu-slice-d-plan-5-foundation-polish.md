# rupu Slice D — Plan 5: Foundation Polish

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the interactivity / chrome gaps in rupu-app that fell through the cracks during D-1 → D-4. Make the sidebar a real accordion, give every row hover + selection + click feedback, restore native macOS window controls, wire the in-flight count badge, surface empty-state affordances, and expand the menubar to a credible v1 shape.

**Architecture:** Eight independent commits, each shippable on its own. Most touch one file in `crates/rupu-app/src/window/` or `crates/rupu-app/src/menu/`. State that needs to persist piggybacks on the existing `Workspace.manifest.ui` struct (already serialized to `~/Library/Application Support/rupu.app/workspaces/<id>.toml` via `workspace::storage::save`). No new crates, no new dependencies beyond what `gpui` already exposes.

**Tech Stack:** GPUI (Zed's framework, git-pinned), `gpui::TitlebarOptions` for window chrome, `gpui::actions!` + `cx.bind_keys` for menubar shortcuts, `std::process::Command` for OS open / reveal-in-Finder.

**Companion docs:** [Slice D design](../specs/2026-05-11-rupu-slice-d-app-design.md), [Plan 4 launcher](2026-05-12-rupu-slice-d-plan-4-launcher.md). This plan is **operator polish**, not a new sub-slice — the sub-slice order D-5..D-10 picks up after this lands.

**Companion audit:** Findings captured 2026-05-13 by visually probing every sidebar / titlebar / menubar surface and cross-referencing against the source. The comment `// For D-1, item clicks are no-ops (tabs land in D-2).` at `crates/rupu-app/src/window/sidebar.rs:5` is the smoking gun — D-2/D-3/D-4 added features but never circled back to wire sidebar interaction.

---

## File structure

### Modified files

- `crates/rupu-app/src/workspace/manifest.rs` — add `UiState::toggle_section_collapsed` method + test.
- `crates/rupu-app/src/window/sidebar.rs` — wire section-header click, add hover + selection styling on rows, add agent-row click + right-click, render empty-state affordances. Add new callback types (`SectionToggleCb`, `AgentClickCb`).
- `crates/rupu-app/src/window/titlebar.rs` — accept an `in_flight: u32` argument; signature changes from `render(&Workspace)` to `render(&Workspace, u32)`. Add unit test for the badge.
- `crates/rupu-app/src/window/mod.rs` — restore native traffic-light controls via `TitlebarOptions`, plumb new sidebar callbacks, plumb in-flight count from `active_run_map`.
- `crates/rupu-app/src/menu/app_menu.rs` — add Edit / View / Window / Help menus and the corresponding action handlers.
- `crates/rupu-app/src/workspace/mod.rs` — re-export `storage::save` for the sidebar handler convenience (optional; saves an import line).

### New files

None. Everything fits inside existing modules.

---

## Task 1: `UiState::toggle_section_collapsed`

**Files:**
- Modify: `crates/rupu-app/src/workspace/manifest.rs:86-99`
- Test: `crates/rupu-app/src/workspace/manifest.rs` (existing `#[cfg(test)] mod tests` at line 101)

- [ ] **Step 1: Write the failing test**

Append to the existing `mod tests` block in `crates/rupu-app/src/workspace/manifest.rs`:

```rust
#[test]
fn toggle_section_collapsed_adds_and_removes() {
    let mut ui = UiState::default();
    assert!(ui.sidebar_collapsed_sections.is_empty());

    ui.toggle_section_collapsed("agents");
    assert_eq!(ui.sidebar_collapsed_sections, vec!["agents".to_string()]);

    ui.toggle_section_collapsed("workflows");
    assert_eq!(
        ui.sidebar_collapsed_sections,
        vec!["agents".to_string(), "workflows".to_string()]
    );

    ui.toggle_section_collapsed("agents");
    assert_eq!(ui.sidebar_collapsed_sections, vec!["workflows".to_string()]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app --lib workspace::manifest::tests::toggle_section_collapsed_adds_and_removes`
Expected: FAIL with "no method named `toggle_section_collapsed` found for struct `UiState`"

- [ ] **Step 3: Implement the method**

Add to `impl UiState` in `crates/rupu-app/src/workspace/manifest.rs`. The struct is currently bare (no `impl` block), so add one right after the struct definition (after line 99):

```rust
impl UiState {
    /// Toggle whether `section` is in `sidebar_collapsed_sections`.
    /// Adds the section if absent, removes it if present.
    pub fn toggle_section_collapsed(&mut self, section: &str) {
        if let Some(idx) = self
            .sidebar_collapsed_sections
            .iter()
            .position(|s| s == section)
        {
            self.sidebar_collapsed_sections.remove(idx);
        } else {
            self.sidebar_collapsed_sections.push(section.to_string());
        }
    }
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p rupu-app --lib workspace::manifest`
Expected: PASS (all three tests including the new one).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/workspace/manifest.rs
git commit -m "feat(app): UiState::toggle_section_collapsed"
```

---

## Task 2: Sidebar section headers toggle collapse + persist

**Files:**
- Modify: `crates/rupu-app/src/window/sidebar.rs` (signature + header rendering)
- Modify: `crates/rupu-app/src/window/mod.rs:602-625` (sidebar callsite)

- [ ] **Step 1: Add the new callback type and re-render headers**

Edit `crates/rupu-app/src/window/sidebar.rs`. After the existing `WorkflowClickCb` type alias (line 26) add:

```rust
/// Callback type for sidebar section-header clicks. Receives the section
/// name (`"workflows"` / `"runs"` / `"repos"` / `"agents"` / `"issues"`),
/// the current GPUI window handle, and the app context.
pub type SectionToggleCb = Arc<dyn Fn(&'static str, &mut Window, &mut App) + Send + Sync + 'static>;
```

Update the `render` signature (line 28-33) to accept it:

```rust
pub fn render(
    workspace: &Workspace,
    active_runs: &ActiveRunMap,
    on_workflow_click: WorkflowClickCb,
    on_workflow_right_click: WorkflowClickCb,
    on_section_toggle: SectionToggleCb,
) -> impl IntoElement {
```

Update the loop body (line 60) to pass the callback into each section:

```rust
container = container.child(render_section(
    section,
    &items,
    is_collapsed,
    i == 0,
    active_runs,
    on_workflow_click.clone(),
    on_workflow_right_click.clone(),
    on_section_toggle.clone(),
));
```

Update `render_section`'s signature (line 74-82) similarly:

```rust
fn render_section(
    name: &'static str,
    items: &[&Asset],
    is_collapsed: bool,
    is_first: bool,
    active_runs: &ActiveRunMap,
    on_workflow_click: WorkflowClickCb,
    on_workflow_right_click: WorkflowClickCb,
    on_section_toggle: SectionToggleCb,
) -> impl IntoElement {
```

(Note: also change the `SECTION_ORDER` declaration at line 17 to use `&'static [&'static str]` — it already does.)

Replace the header construction (lines 103-113) with a clickable + always-visible-caret version:

```rust
let caret = if is_collapsed { "▸" } else { "▾" };
let header = div()
    .id(gpui::SharedString::from(format!("sec-{name}")))
    .text_xs()
    .text_color(palette::TEXT_DIMMEST)
    .mb(px(4.0))
    .when(!is_first, |d| d.mt(px(18.0)))
    .flex()
    .items_center()
    .gap(px(6.0))
    .cursor_pointer()
    .child(div().child(caret))
    .child(div().child(name.to_uppercase()))
    .child(
        // Count badge always shown, dimmer when not collapsed
        div()
            .ml_auto()
            .text_color(palette::TEXT_DIMMEST)
            .child(format!("{}", items.len())),
    )
    .on_click({
        let cb = on_section_toggle.clone();
        move |_, w, cx| cb(name, w, cx)
    });
```

Delete the old `caret_child` / `count_child` blocks at lines 84-101 — the new header above replaces them.

- [ ] **Step 2: Add `handle_section_toggle` on `WorkspaceWindow`**

In `crates/rupu-app/src/window/mod.rs`, add this method to the `impl WorkspaceWindow { ... }` block (next to `handle_workflow_clicked` at line 277):

```rust
/// Toggle a sidebar section's collapsed state and persist.
pub fn handle_section_toggle(&mut self, section: &'static str, cx: &mut Context<Self>) {
    self.workspace
        .manifest
        .ui
        .toggle_section_collapsed(section);
    if let Err(e) = crate::workspace::storage::save(&self.workspace.manifest) {
        tracing::warn!(%e, "persist sidebar collapse state");
    }
    cx.notify();
}
```

- [ ] **Step 3: Wire the callback at the sidebar callsite**

In `crates/rupu-app/src/window/mod.rs`, inside the `Render::render` body, just after `let weak_sidebar_right = weak.clone();` (around line 523) add:

```rust
let weak_section_toggle = weak.clone();
```

Then inside the sidebar callsite (replace lines 602-625, the `.child({ ... sidebar::render(...) })` block), add a new closure and pass it as the fifth arg:

```rust
.child({
    let on_workflow_click: WorkflowClickCb = Arc::new(move |path, _w, cx| {
        let weak_sidebar_click = weak_sidebar_click.clone();
        cx.defer(move |cx| {
            let _ = weak_sidebar_click
                .update(cx, |this, cx| this.handle_workflow_clicked(path, cx));
        });
    });
    let on_workflow_right_click: WorkflowClickCb =
        Arc::new(move |path, _w, cx| {
            let weak_sidebar_right = weak_sidebar_right.clone();
            cx.defer(move |cx| {
                let _ = weak_sidebar_right.update(cx, |this, cx| {
                    this.handle_workflow_right_clicked(path, cx)
                });
            });
        });
    let on_section_toggle: sidebar::SectionToggleCb =
        Arc::new(move |section, _w, cx| {
            let weak = weak_section_toggle.clone();
            cx.defer(move |cx| {
                let _ = weak
                    .update(cx, |this, cx| this.handle_section_toggle(section, cx));
            });
        });
    sidebar::render(
        &self.workspace,
        &active_run_map,
        on_workflow_click,
        on_workflow_right_click,
        on_section_toggle,
    )
})
```

- [ ] **Step 4: Build and verify**

Run: `cargo build -p rupu-app`
Expected: clean build, no warnings.

- [ ] **Step 5: Smoke test manually**

Run: `cargo run -p rupu-app`
Expected: clicking any section header (e.g., AGENTS) toggles the caret (▾ ↔ ▸) and hides/shows the items under it. The count badge stays visible in both states. After clicking, close the app and reopen — the collapsed state should persist (read from manifest TOML).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/window/sidebar.rs crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): sidebar section headers toggle collapse + persist"
```

---

## Task 3: Sidebar row hover + selection styling

**Files:**
- Modify: `crates/rupu-app/src/window/sidebar.rs:138-174` (the per-asset row rendering)
- Modify: `crates/rupu-app/src/window/sidebar.rs:28-33` (render signature: add `focused_workflow`)
- Modify: `crates/rupu-app/src/window/mod.rs:619-624` (sidebar callsite: pass `focused_workflow`)
- Modify: `crates/rupu-app/src/palette.rs` (new color tokens)

- [ ] **Step 1: Add palette tokens for hover and selection**

Append to `crates/rupu-app/src/palette.rs` after `pub const BORDER: ...` (line 41):

```rust
pub const BG_ROW_HOVER: Rgba = rgb(36, 36, 41); // sidebar row hover (#242429)
pub const BG_ROW_SELECTED: Rgba = rgb(50, 50, 56); // sidebar row selected (#323238)
```

- [ ] **Step 2: Extend `sidebar::render` to take the focused workflow path**

Update `crates/rupu-app/src/window/sidebar.rs` `render` signature (after Task 2 it has five args; add a sixth before the callbacks for readability):

```rust
pub fn render(
    workspace: &Workspace,
    active_runs: &ActiveRunMap,
    focused_workflow: Option<&PathBuf>,
    on_workflow_click: WorkflowClickCb,
    on_workflow_right_click: WorkflowClickCb,
    on_section_toggle: SectionToggleCb,
) -> impl IntoElement {
```

Plumb it through `render_section` (add `focused_workflow: Option<&PathBuf>` arg) and into the asset loop.

- [ ] **Step 3: Apply hover + selection styling to rows**

Replace the row construction in `render_section` (lines 139-150 in the post-Task-2 file). Find the `let mut row = div()…` block and replace with:

```rust
let is_selected = is_workflows
    && focused_workflow.map(|p| p == &asset.path).unwrap_or(false);
let row_bg = if is_selected {
    palette::BG_ROW_SELECTED
} else {
    palette::BG_SIDEBAR
};
let mut row = div()
    .id(gpui::SharedString::from(format!(
        "wf-{}",
        asset.path.display()
    )))
    .flex()
    .flex_row()
    .items_center()
    .gap(px(4.0))
    .px(px(4.0))
    .py(px(2.0))
    .rounded(px(3.0))
    .bg(row_bg)
    .hover(|s| s.bg(palette::BG_ROW_HOVER))
    .text_xs()
    .text_color(palette::TEXT_MUTED)
    .child(div().flex_1().child(asset.name.clone()));
```

- [ ] **Step 4: Update the sidebar callsite**

In `crates/rupu-app/src/window/mod.rs`'s `sidebar::render(...)` invocation, pass `focused_workflow`:

```rust
sidebar::render(
    &self.workspace,
    &active_run_map,
    self.focused_workflow.as_ref(),
    on_workflow_click,
    on_workflow_right_click,
    on_section_toggle,
)
```

- [ ] **Step 5: Build and smoke test**

Run: `cargo build -p rupu-app && cargo run -p rupu-app`
Expected: hovering any sidebar row shows a darker background. Clicking a workflow row highlights it with `BG_ROW_SELECTED`; clicking another workflow row moves the highlight.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/palette.rs crates/rupu-app/src/window/sidebar.rs crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): sidebar row hover + selection styling"
```

---

## Task 4: Agent rows — click opens file, right-click reveals in Finder

**Files:**
- Modify: `crates/rupu-app/src/window/sidebar.rs` (add `AgentClickCb`, attach handlers to agent rows)
- Modify: `crates/rupu-app/src/window/mod.rs` (add `handle_agent_clicked` / `handle_agent_reveal`, wire callbacks)

- [ ] **Step 1: Add the agent callback type**

In `crates/rupu-app/src/window/sidebar.rs`, after `pub type SectionToggleCb = …`:

```rust
/// Callback type for sidebar agent-row clicks. Receives the agent file
/// path, the current GPUI window handle, and the app context.
pub type AgentClickCb = Arc<dyn Fn(PathBuf, &mut Window, &mut App) + Send + Sync + 'static>;
```

- [ ] **Step 2: Extend `render` and `render_section` signatures**

Add two new params (alongside the workflow callbacks):

```rust
on_agent_click: AgentClickCb,
on_agent_right_click: AgentClickCb,
```

Forward into `render_section`.

- [ ] **Step 3: Attach handlers to agent rows**

Replace the existing `if is_workflows { … }` block (lines 156-169 in the post-Task-3 file) with a kind-aware version. The agent branch mirrors the workflow branch:

```rust
match name {
    "workflows" => {
        let cb_click = on_workflow_click.clone();
        let cb_right = on_workflow_right_click.clone();
        let path_right = path.clone();
        row = row
            .cursor_pointer()
            .on_click({
                let path = path.clone();
                move |_, w, cx| cb_click(path.clone(), w, cx)
            })
            .on_mouse_down(MouseButton::Right, {
                move |_, w, cx| cb_right(path_right.clone(), w, cx)
            });
    }
    "agents" => {
        let cb_click = on_agent_click.clone();
        let cb_right = on_agent_right_click.clone();
        let path_right = path.clone();
        row = row
            .cursor_pointer()
            .on_click({
                let path = path.clone();
                move |_, w, cx| cb_click(path.clone(), w, cx)
            })
            .on_mouse_down(MouseButton::Right, {
                move |_, w, cx| cb_right(path_right.clone(), w, cx)
            });
    }
    _ => {} // runs / repos / issues — wired in D-3 / D-9
}
```

- [ ] **Step 4: Add the open + reveal handlers on `WorkspaceWindow`**

In `crates/rupu-app/src/window/mod.rs`, next to `handle_section_toggle` add:

```rust
/// Open an agent's `.md` source file in the user's default app.
/// macOS: `open <path>`. On other platforms: log + return (no-op).
pub fn handle_agent_clicked(&mut self, path: PathBuf, _cx: &mut Context<Self>) {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = std::process::Command::new("open").arg(&path).spawn() {
            tracing::warn!(?path, %e, "open agent file");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        tracing::info!(?path, "agent click: no-op on non-macOS");
    }
}

/// Reveal the agent file in Finder. macOS: `open -R <path>`.
pub fn handle_agent_reveal(&mut self, path: PathBuf, _cx: &mut Context<Self>) {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = std::process::Command::new("open")
            .args(["-R"])
            .arg(&path)
            .spawn()
        {
            tracing::warn!(?path, %e, "reveal agent in Finder");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        tracing::info!(?path, "reveal: no-op on non-macOS");
    }
}
```

- [ ] **Step 5: Wire the callbacks at the sidebar callsite**

In `crates/rupu-app/src/window/mod.rs`'s `Render::render`, add two more weak clones (alongside `weak_section_toggle`):

```rust
let weak_agent_click = weak.clone();
let weak_agent_reveal = weak.clone();
```

And inside the sidebar `.child({…})` block, build the two callbacks and pass them:

```rust
let on_agent_click: sidebar::AgentClickCb = Arc::new(move |path, _w, cx| {
    let weak = weak_agent_click.clone();
    cx.defer(move |cx| {
        let _ = weak.update(cx, |this, cx| this.handle_agent_clicked(path, cx));
    });
});
let on_agent_right_click: sidebar::AgentClickCb = Arc::new(move |path, _w, cx| {
    let weak = weak_agent_reveal.clone();
    cx.defer(move |cx| {
        let _ = weak.update(cx, |this, cx| this.handle_agent_reveal(path, cx));
    });
});
sidebar::render(
    &self.workspace,
    &active_run_map,
    self.focused_workflow.as_ref(),
    on_workflow_click,
    on_workflow_right_click,
    on_agent_click,
    on_agent_right_click,
    on_section_toggle,
)
```

- [ ] **Step 6: Build and smoke test**

Run: `cargo build -p rupu-app && cargo run -p rupu-app`
Expected: clicking an agent row opens its `.md` source in the OS default app (TextEdit / Cursor / VS Code / whatever the user has set). Right-clicking reveals it in Finder.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-app/src/window/sidebar.rs crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): agent rows open file on click, reveal in Finder on right-click"
```

---

## Task 5: Empty-state affordances on WORKFLOWS and AGENTS

**Files:**
- Modify: `crates/rupu-app/src/window/sidebar.rs:118-124` (the empty-body branch)

- [ ] **Step 1: Replace the static em-dash with a per-section hint**

In `crates/rupu-app/src/window/sidebar.rs`'s `render_section`, replace the empty-state branch (lines 118-124, the `else if items.is_empty()` block):

```rust
} else if items.is_empty() {
    let hint = match name {
        "workflows" => Some("File → New Workspace to add"),
        "agents" => Some("Drop a `.md` agent into `~/.rupu/agents/`"),
        // Runs / repos / issues come alive in D-3 / D-9; leave a dash
        // until then.
        _ => None,
    };
    match hint {
        Some(h) => div()
            .mt(px(2.0))
            .text_xs()
            .text_color(palette::TEXT_DIMMEST)
            .italic()
            .child(h)
            .into_any_element(),
        None => div()
            .mt(px(2.0))
            .text_xs()
            .text_color(palette::TEXT_DIMMEST)
            .child("—")
            .into_any_element(),
    }
}
```

- [ ] **Step 2: Build and smoke test**

Open a workspace with no agents (or empty global `~/.rupu/agents/`):

Run: `cargo build -p rupu-app && cargo run -p rupu-app`
Expected: AGENTS section shows the italic hint instead of `—` when there are no agents. Same for WORKFLOWS.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/window/sidebar.rs
git commit -m "feat(app): empty-state hints on WORKFLOWS / AGENTS sidebar sections"
```

---

## Task 6: In-flight count badge in the titlebar

**Files:**
- Modify: `crates/rupu-app/src/window/titlebar.rs:12-53` (signature + body + tests)
- Modify: `crates/rupu-app/src/window/mod.rs:596` (titlebar callsite)

- [ ] **Step 1: Write the failing test**

Add a `#[cfg(test)] mod tests` at the bottom of `crates/rupu-app/src/window/titlebar.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::in_flight_count;
    use rupu_orchestrator::runs::RunStatus;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn in_flight_count_counts_running_and_awaiting() {
        let mut m: HashMap<PathBuf, RunStatus> = HashMap::new();
        m.insert("a".into(), RunStatus::Running);
        m.insert("b".into(), RunStatus::AwaitingApproval);
        m.insert("c".into(), RunStatus::Succeeded);
        m.insert("d".into(), RunStatus::Failed);
        assert_eq!(in_flight_count(&m), 2);
    }

    #[test]
    fn in_flight_count_zero_when_empty() {
        let m: HashMap<PathBuf, RunStatus> = HashMap::new();
        assert_eq!(in_flight_count(&m), 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app --lib window::titlebar::tests`
Expected: FAIL with "cannot find function `in_flight_count` in this scope".

- [ ] **Step 3: Implement `in_flight_count` and re-shape `render`**

Replace the whole body of `crates/rupu-app/src/window/titlebar.rs` with:

```rust
//! Titlebar: color chip · workspace name · in-flight count badge.
//!
//! Per spec §6.1, the count is this-workspace only (the system
//! menubar in `menu/menubar.rs` carries the cross-workspace
//! count).

use crate::palette;
use crate::window::sidebar::ActiveRunMap;
use crate::workspace::Workspace;
use gpui::{div, prelude::*, px, FontWeight, IntoElement};
use rupu_orchestrator::runs::RunStatus;

/// Count workflows whose active run is Running or AwaitingApproval.
pub fn in_flight_count(active_runs: &ActiveRunMap) -> u32 {
    active_runs
        .values()
        .filter(|s| matches!(s, RunStatus::Running | RunStatus::AwaitingApproval))
        .count() as u32
}

pub fn render(workspace: &Workspace, in_flight: u32) -> impl IntoElement {
    let chip_color = workspace.manifest.color.to_rgba();

    div()
        .h(px(36.0))
        .bg(palette::BG_TITLEBAR)
        .border_b_1()
        .border_color(palette::BORDER)
        // Leave room for native traffic lights on the left edge — Task 7
        // re-enables them at (9, 9).
        .pl(px(80.0))
        .pr(px(14.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(chip_color))
        .child(
            div()
                .text_color(palette::TEXT_PRIMARY)
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(workspace.manifest.name.clone()),
        )
        .child(if in_flight > 0 {
            div()
                .ml(px(8.0))
                .px(px(6.0))
                .py(px(1.0))
                .rounded(px(4.0))
                .bg(palette::RUNNING)
                .text_color(palette::TEXT_PRIMARY)
                .text_xs()
                .child(format!("{in_flight} running"))
                .into_any_element()
        } else {
            div().into_any_element()
        })
}
```

- [ ] **Step 4: Update the callsite**

In `crates/rupu-app/src/window/mod.rs:596`, change:

```rust
.child(titlebar::render(&self.workspace))
```

to:

```rust
.child(titlebar::render(
    &self.workspace,
    titlebar::in_flight_count(&active_run_map),
))
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-app --lib window::titlebar::tests`
Expected: PASS (both tests).

- [ ] **Step 6: Smoke test**

Run: `cargo run -p rupu-app` and launch a workflow. While the run is in `Running` state the titlebar should show `1 running` badge next to the workspace name; the badge clears once the run completes.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-app/src/window/titlebar.rs crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): in-flight count badge in workspace titlebar"
```

---

## Task 7: Restore native macOS traffic-light controls

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs:51-55` (WindowOptions)

- [ ] **Step 1: Add the `TitlebarOptions` import**

In `crates/rupu-app/src/window/mod.rs`, extend the gpui import (line 13-16) to include `TitlebarOptions` and `point`:

```rust
use gpui::{
    div, point, prelude::*, px, size, AnyElement, App, Bounds, Context, IntoElement, Render,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions,
};
```

- [ ] **Step 2: Replace `titlebar: None` with `Some(TitlebarOptions { ... })`**

In `WorkspaceWindow::open` (line 51-55), change:

```rust
let opts = WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(bounds)),
    titlebar: None, // we draw our own titlebar inside the view
    ..Default::default()
};
```

to:

```rust
let opts = WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(bounds)),
    // appears_transparent = true keeps the custom titlebar we draw in
    // `titlebar::render` (color chip + name + in-flight badge) while
    // restoring the native macOS traffic-light controls. The 80px
    // left-padding on the custom titlebar leaves room for the lights.
    // Position mirrors Zed (crates/zed/src/zed.rs:352).
    titlebar: Some(TitlebarOptions {
        title: None,
        appears_transparent: true,
        traffic_light_position: Some(point(px(9.0), px(9.0))),
    }),
    ..Default::default()
};
```

- [ ] **Step 3: Build and smoke test**

Run: `cargo build -p rupu-app && cargo run -p rupu-app`
Expected: the window now has the standard macOS red / yellow / green traffic-light controls in the top-left corner. The custom titlebar content (chip + name + badge) still renders, offset right by 80px so the lights don't overlap.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): restore native macOS traffic-light controls"
```

---

## Task 8: Expand the macOS menubar (Edit / View / Window / Help)

**Files:**
- Modify: `crates/rupu-app/src/menu/app_menu.rs:8-55` (actions, menu structure, handlers)
- Modify: `crates/rupu-app/src/window/mod.rs` (add `handle_toggle_sidebar` if View > Toggle Sidebar lands; see Step 4)

- [ ] **Step 1: Declare the new actions**

In `crates/rupu-app/src/menu/app_menu.rs:8-18`, extend the `actions!` macro:

```rust
actions!(
    rupu_app,
    [
        NewWorkspace,
        OpenWorkspace,
        Quit,
        ApproveFocused,
        RejectFocused,
        LaunchSelected,
        ToggleSidebar,
        AboutRupu,
    ]
);
```

- [ ] **Step 2: Rebuild the menu structure**

Replace the `cx.set_menus(...)` block (lines 22-29) with:

```rust
cx.set_menus(vec![
    Menu::new("rupu").items(vec![
        MenuItem::action("About rupu", AboutRupu),
        MenuItem::separator(),
        MenuItem::action("Quit rupu", Quit),
    ]),
    Menu::new("File").items(vec![
        MenuItem::action("New Workspace\u{2026}", NewWorkspace),
        MenuItem::action("Open Workspace\u{2026}", OpenWorkspace),
    ]),
    Menu::new("Edit").items(vec![
        // Standard macOS clipboard items dispatch through GPUI's built-in
        // text-editing actions on focused text fields. Listing them in the
        // menu surfaces the ⌘C / ⌘V / ⌘X shortcuts so users know they
        // work in the launcher's inputs.
        MenuItem::os_action(
            "Cut",
            gpui::ClipboardItemKind::Cut,
            gpui::OsAction::Cut,
        ),
        MenuItem::os_action(
            "Copy",
            gpui::ClipboardItemKind::Copy,
            gpui::OsAction::Copy,
        ),
        MenuItem::os_action(
            "Paste",
            gpui::ClipboardItemKind::Paste,
            gpui::OsAction::Paste,
        ),
    ]),
    Menu::new("View").items(vec![
        MenuItem::action("Toggle Sidebar", ToggleSidebar),
    ]),
    Menu::new("Window").items(vec![
        // macOS standard "Minimize / Zoom / Bring All to Front" are
        // synthesized by AppKit when the window has the standard
        // titlebar (restored in Task 7).
    ]),
    Menu::new("Help").items(vec![
        // Help → About is the macOS convention for app info; we delegate
        // to the app-menu's About item above.
    ]),
]);
```

> If `gpui::ClipboardItemKind` / `OsAction` / `MenuItem::os_action` don't exist in the pinned rev, fall back to plain `MenuItem::action("Cut", Cut)` etc. with corresponding `actions!` entries — but at the same rev as Zed master they're typically wired. Inspect with `grep -n "os_action\|OsAction" ~/.cargo/git/checkouts/zed-*/1a2e50e/crates/gpui/src/platform.rs` first; remove the Edit menu entirely if the API isn't present at this rev.

- [ ] **Step 3: Bind a keyboard shortcut for `ToggleSidebar`**

Extend the existing `cx.bind_keys(...)` block (lines 34-38):

```rust
cx.bind_keys(vec![
    KeyBinding::new("a", ApproveFocused, None),
    KeyBinding::new("r", RejectFocused, None),
    KeyBinding::new("cmd-r", LaunchSelected, None),
    KeyBinding::new("cmd-\\", ToggleSidebar, None),
]);
```

- [ ] **Step 4: Add the action handlers**

After the existing `cx.on_action(|_: &Quit, cx| cx.quit());` (line 54), add:

```rust
cx.on_action(|_: &AboutRupu, _cx| {
    tracing::info!("About rupu — version {}", env!("CARGO_PKG_VERSION"));
    // TODO(D-10): native About panel via NSAlert.
});

cx.on_action(|_: &ToggleSidebar, cx| {
    // Toggles the `workflows` section as a proxy for the whole sidebar.
    // A future task may add a dedicated `sidebar_hidden` flag to UiState
    // if we want a true show/hide.
    let Some(window) = cx.active_window() else {
        return;
    };
    let _ = window.update(cx, |_root, _w, app_cx| {
        let entity = app_cx.entity::<WorkspaceWindow>();
        if let Some(handle) = entity {
            handle.update(app_cx, |this, cx| {
                this.handle_section_toggle("workflows", cx);
                this.handle_section_toggle("runs", cx);
                this.handle_section_toggle("repos", cx);
                this.handle_section_toggle("agents", cx);
                this.handle_section_toggle("issues", cx);
            });
        }
    });
});
```

> Note: `cx.active_window()` / `entity::<WorkspaceWindow>()` are the GPUI APIs at the pinned rev (matches how the launcher's ⌘R is dispatched). If GPUI exposes a more direct "focused entity" call at this rev, prefer that — investigate before implementing this step. If neither works cleanly, defer ToggleSidebar to a follow-up commit and leave the menu item unbound but visible.

- [ ] **Step 5: Build and smoke test**

Run: `cargo build -p rupu-app && cargo run -p rupu-app`
Expected: the system menubar now shows `rupu  File  Edit  View  Window  Help` with the appropriate items under each. `⌘\` toggles all sidebar sections collapsed/expanded. `⌘C` / `⌘V` work in any focused launcher input field.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/menu/app_menu.rs
git commit -m "feat(app): expand macOS menubar with Edit / View / Window / Help"
```

---

## Wrap-up

- [ ] **Run the full test suite**

Run: `cargo test -p rupu-app --lib`
Expected: all tests pass (manifest tests + titlebar tests + existing).

- [ ] **Run clippy**

Run: `cargo clippy -p rupu-app -- -D warnings`
Expected: clean, no warnings (workspace policy is `#[deny(clippy::all)]`).

- [ ] **Final manual pass**

Boot `cargo run -p rupu-app`, walk every surface in the audit list:
- Traffic-light controls work (close, minimize, zoom).
- Section headers toggle with caret + count badge always visible; state persists across relaunch.
- Workflow row click selects (highlight) + opens drill-down on Run.
- Workflow row right-click opens the launcher.
- Agent row click opens the `.md` file in default app.
- Agent row right-click reveals in Finder.
- Empty AGENTS section shows the italic hint instead of `—`.
- Titlebar shows `N running` badge during an active run.
- Menubar has all five menus + ⌘\ toggles sidebar + ⌘C/V works in inputs.

- [ ] **Open the PR**

```bash
gh pr create --title "feat(app): D-4.5 foundation polish — sidebar accordion, traffic lights, menubar" --body "$(cat <<'EOF'
## Summary

Closes the interactivity / chrome gaps that fell through the cracks during Slice D Plans 1–4. Plan: docs/superpowers/plans/2026-05-13-rupu-slice-d-plan-5-foundation-polish.md

- Sidebar section headers toggle collapse/expand with persistent state (caret + count badge always visible).
- Row hover + selection styling.
- Agent rows: click opens the .md in your default editor, right-click reveals in Finder.
- Empty WORKFLOWS / AGENTS sections show actionable hints instead of `—`.
- Titlebar in-flight count badge wired to the active-run map.
- Native macOS traffic-light controls restored (close / minimize / zoom).
- Menubar gains Edit / View / Window / Help; ⌘\ toggles sidebar.

## Test plan

- [ ] cargo test -p rupu-app --lib
- [ ] cargo clippy -p rupu-app -- -D warnings
- [ ] matt runs the binary and walks the manual pass list in the plan's Wrap-up section
EOF
)"
```

---

## Self-review

**Spec coverage:** This plan addresses the eight items in the audit findings (sidebar accordion, hover, selection, agent click, empty-state, in-flight count, traffic lights, menubar). No keyboard nav for arrow-key sidebar traversal — intentionally deferred; it requires a focus model and arrow-key event handling that's out of scope here. Open Recent submenu in File is mentioned in the design but also deferred — recents storage exists (`crates/rupu-app/src/workspace/recents.rs`) but the dynamic submenu population pattern in GPUI is not trivial; better as its own task.

**Type consistency:** `SectionToggleCb` takes `&'static str`, matching the static section-name constants in `SECTION_ORDER`. `AgentClickCb` takes `PathBuf`, matching `WorkflowClickCb`'s `PathBuf` for consistency. Titlebar's `in_flight_count` returns `u32`, matching the existing `in_flight: u32` local in the unchanged-signature version.

**Placeholder scan:** No "TBD" / "implement later" / "appropriate error handling" — every step has the code or the exact command. Task 8 has a note about the `os_action` API potentially not being present at the pinned rev with a concrete fallback; that's a known unknown made explicit, not a placeholder.
