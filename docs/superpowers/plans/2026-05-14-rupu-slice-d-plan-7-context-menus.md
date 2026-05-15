# rupu Slice D — Plan 7: Right-Click Context Menus

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Right-clicking a sidebar workflow / agent row pops a small context menu at the mouse position with relevant actions, instead of immediately firing one hard-coded action. Closes matt's 2026-05-14 feedback: "actions right-click should open a menu not directly perform an action."

**Architecture:** Add a minimal `ContextMenuState` + render function to `crates/rupu-app/src/widget/context_menu.rs`. State lives as `Option<ContextMenuState>` on `WorkspaceWindow`. Render via `gpui::anchored().position(state.position)` overlaid by `gpui::deferred(...)` so it paints last and the auto-fit handles window-edge overflow. Right-click callbacks gain a `Point<Pixels>` arg (the click position from `MouseDownEvent.position`); the window builds the section-appropriate item list and pops the menu. Click-outside dismisses via a transparent fullscreen backdrop layer that captures mouse-down.

**Tech Stack:** GPUI's `anchored()` (auto-overflow), `deferred()` (overlay paint), `MouseDownEvent.position`, existing CLAUDE.md rule #3 `cx.defer` pattern.

**Companion docs:** [Slice D design](../specs/2026-05-11-rupu-slice-d-app-design.md), [Plan 5 foundation polish](2026-05-13-rupu-slice-d-plan-5-foundation-polish.md), [Plan 6 launcher text input](2026-05-13-rupu-slice-d-plan-6-launcher-text-input.md).

**Audit findings closed:**
- Plan 5 Task 4 wired workflow right-click → direct launcher open, agent right-click → direct reveal in Finder. matt: "right-click should open a menu not directly perform an action."

---

## File structure

### New files

- `crates/rupu-app/src/widget/context_menu.rs` — `ContextMenuItem`, `ContextMenuState`, `pub fn render(state, on_dismiss)` returning an `AnyElement` overlay.

### Modified files

- `crates/rupu-app/src/widget/mod.rs` — `pub mod context_menu;` + re-exports.
- `crates/rupu-app/src/menu/app_menu.rs` — declare `DismissContextMenu` action + `escape` keybinding scoped globally (the menu is the only thing Esc dismisses for now).
- `crates/rupu-app/src/window/sidebar.rs` — change `WorkflowClickCb` for right-click and `AgentClickCb` for right-click to a new shape that includes `Point<Pixels>`; pass the `MouseDownEvent.position` into the callback.
- `crates/rupu-app/src/window/mod.rs` — replace `handle_workflow_right_clicked` and `handle_agent_reveal`'s direct-action behavior with `open_context_menu` that builds an items list. Add `context_menu: Option<ContextMenuState>` field, `handle_dismiss_context_menu`, and overlay rendering in `Render::render`.

---

## Task 1: `ContextMenuItem` + `ContextMenuState` + render

**Files:**
- Create: `crates/rupu-app/src/widget/context_menu.rs`
- Modify: `crates/rupu-app/src/widget/mod.rs`

- [ ] **Step 1: Write the module**

```rust
// crates/rupu-app/src/widget/context_menu.rs

//! Minimal right-click context menu — a floating overlay anchored at the
//! mouse position with a vertical list of selectable items. Used by the
//! sidebar's workflow / agent right-click handlers.
//!
//! State (`ContextMenuState`) lives on `WorkspaceWindow`; the window
//! decides what items to show based on which row was right-clicked.
//! `render` returns an overlay element that should be embedded as the
//! last child of the window's root div so it paints over everything else.

use std::sync::Arc;

use gpui::{
    anchored, deferred, div, prelude::*, px, AnyElement, App, Bounds, IntoElement, MouseButton,
    Pixels, Point, SharedString, Window,
};

use crate::palette;

/// One row in a context menu.
#[derive(Clone)]
pub struct ContextMenuItem {
    pub label: SharedString,
    pub on_select: Arc<dyn Fn(&mut Window, &mut App) + Send + Sync + 'static>,
}

impl std::fmt::Debug for ContextMenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextMenuItem")
            .field("label", &self.label)
            .field("on_select", &"<fn>")
            .finish()
    }
}

/// State driving the context-menu overlay. `None` on `WorkspaceWindow` means
/// no menu is open; `Some` means the overlay renders.
#[derive(Clone, Debug)]
pub struct ContextMenuState {
    pub position: Point<Pixels>,
    pub items: Vec<ContextMenuItem>,
}

/// Callback the overlay invokes to dismiss itself (item selection or
/// click-outside both call this).
pub type DismissCb = Arc<dyn Fn(&mut Window, &mut App) + Send + Sync + 'static>;

/// Render the menu as an absolutely-positioned overlay. The caller must wrap
/// the return in `deferred(...)` (the convention here keeps the overlay
/// painting after the rest of the window).
pub fn render(state: &ContextMenuState, on_dismiss: DismissCb) -> AnyElement {
    let dismiss_for_backdrop = on_dismiss.clone();
    // Backdrop: invisible fullscreen layer that catches a mouse-down anywhere
    // outside the menu and dismisses. Sits behind the menu in z-order.
    let backdrop = div()
        .absolute()
        .inset_0()
        .on_mouse_down(MouseButton::Left, move |_ev, w, cx| {
            dismiss_for_backdrop(w, cx)
        })
        .on_mouse_down(MouseButton::Right, {
            let cb = on_dismiss.clone();
            move |_ev, w, cx| cb(w, cx)
        });

    let mut list = div()
        .min_w(px(180.0))
        .bg(palette::BG_SIDEBAR)
        .border_1()
        .border_color(palette::BORDER)
        .rounded(px(4.0))
        .py(px(4.0))
        .flex()
        .flex_col();

    for (idx, item) in state.items.iter().enumerate() {
        let cb_select = item.on_select.clone();
        let cb_dismiss_after = on_dismiss.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("ctxmenu-item-{idx}")))
                .px(px(10.0))
                .py(px(4.0))
                .text_sm()
                .text_color(palette::TEXT_PRIMARY)
                .cursor_pointer()
                .hover(|s| s.bg(palette::BG_ROW_HOVER))
                .child(item.label.clone())
                .on_mouse_down(MouseButton::Left, move |_ev, w, cx| {
                    cb_select(w, cx);
                    cb_dismiss_after(w, cx);
                }),
        );
    }

    let menu = anchored()
        .position(state.position)
        .snap_to_window_with_margin(gpui::Edges {
            top: px(4.0),
            right: px(4.0),
            bottom: px(4.0),
            left: px(4.0),
        })
        .child(list);

    deferred(div().absolute().inset_0().child(backdrop).child(menu))
        .with_priority(1)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_menu_state_clone_preserves_items() {
        let state = ContextMenuState {
            position: Point::new(px(10.0), px(20.0)),
            items: vec![ContextMenuItem {
                label: "Run".into(),
                on_select: Arc::new(|_w, _cx| {}),
            }],
        };
        let cloned = state.clone();
        assert_eq!(cloned.items.len(), 1);
        assert_eq!(cloned.items[0].label.as_ref(), "Run");
        assert_eq!(cloned.position.x, px(10.0));
    }
}
```

- [ ] **Step 2: Re-export from the widget module**

Edit `crates/rupu-app/src/widget/mod.rs`. Append:

```rust
pub mod context_menu;
pub use context_menu::{ContextMenuItem, ContextMenuState, DismissCb};
```

- [ ] **Step 3: Build + clippy + run the unit test**

```bash
cargo build -p rupu-app 2>&1 | tail -5
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
cargo test -p rupu-app --lib widget::context_menu 2>&1 | tail -5
```

Expected: clean build, no warnings, one test passes.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/widget/
git commit -m "feat(app): ContextMenu widget primitive"
```

---

## Task 2: `context_menu` field + `handle_dismiss_context_menu` on `WorkspaceWindow`

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs` — add the field + dismiss method.

- [ ] **Step 1: Add the field**

In `crates/rupu-app/src/window/mod.rs`, find the `pub struct WorkspaceWindow { ... }` block (around line 20). Add the field below `focused_workflow`:

```rust
pub focused_workflow: Option<PathBuf>,
/// `Some` while a right-click context menu is open. Cleared by item
/// selection, click-outside, or Esc.
pub context_menu: Option<crate::widget::ContextMenuState>,
```

In `WorkspaceWindow::open`, initialize the field in the struct literal:

```rust
focused_workflow: None,
context_menu: None,
```

- [ ] **Step 2: Add the dismiss handler**

Add this method to the `impl WorkspaceWindow { ... }` block, next to `handle_section_toggle`:

```rust
/// Close the context menu. Idempotent — safe to call when no menu is open.
pub fn handle_dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
    if self.context_menu.is_some() {
        self.context_menu = None;
        cx.notify();
    }
}
```

- [ ] **Step 3: Build + clippy**

```bash
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): WorkspaceWindow.context_menu field + dismiss handler"
```

---

## Task 3: Change sidebar right-click callbacks to include click position

**Files:**
- Modify: `crates/rupu-app/src/window/sidebar.rs` — new callback type for right-click, route the event position through.

- [ ] **Step 1: Add the right-click callback type**

In `crates/rupu-app/src/window/sidebar.rs`, just after `pub type AgentClickCb = …`, add:

```rust
/// Callback type for sidebar right-clicks. Receives the asset path,
/// the mouse-down position in window coords (for the context-menu anchor),
/// the GPUI window handle, and the app context.
pub type RightClickCb =
    Arc<dyn Fn(PathBuf, gpui::Point<gpui::Pixels>, &mut Window, &mut App) + Send + Sync + 'static>;
```

- [ ] **Step 2: Update `render` and `render_section` signatures**

Replace the two existing right-click parameters' types in `render` and `render_section`. The signatures currently use `WorkflowClickCb` / `AgentClickCb` for right-clicks; change those two to `RightClickCb`.

In `pub fn render`:

```rust
#[allow(clippy::too_many_arguments)]
pub fn render(
    workspace: &Workspace,
    active_runs: &ActiveRunMap,
    focused_workflow: Option<&PathBuf>,
    on_workflow_click: WorkflowClickCb,
    on_workflow_right_click: RightClickCb,
    on_agent_click: AgentClickCb,
    on_agent_right_click: RightClickCb,
    on_section_toggle: SectionToggleCb,
) -> impl IntoElement {
```

And the call site inside the loop that forwards into `render_section`:

```rust
container = container.child(render_section(
    section,
    &items,
    is_collapsed,
    i == 0,
    focused_workflow,
    active_runs,
    on_workflow_click.clone(),
    on_workflow_right_click.clone(),
    on_agent_click.clone(),
    on_agent_right_click.clone(),
    on_section_toggle.clone(),
));
```

In `fn render_section`, change the two right-click parameter types to `RightClickCb` and update the doc comment to mention the new position arg.

- [ ] **Step 3: Pass the click position into the callbacks**

Find the two `on_mouse_down(MouseButton::Right, ...)` calls in `render_section` (around lines 211 + 225). Replace each:

```rust
.on_mouse_down(MouseButton::Right, {
    move |_, w, cx| cb_right(path_right.clone(), w, cx)
});
```

with:

```rust
.on_mouse_down(MouseButton::Right, {
    move |ev: &gpui::MouseDownEvent, w, cx| {
        cb_right(path_right.clone(), ev.position, w, cx)
    }
});
```

Do this for BOTH the workflow branch and the agent branch in the `match name { "workflows" => ..., "agents" => ... }` block.

- [ ] **Step 4: Build + clippy**

```bash
cargo build -p rupu-app 2>&1 | tail -5
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
```

Expected: the build will fail at `crates/rupu-app/src/window/mod.rs` because the callsite (in `Render::render`) still constructs the old-shape closures. Task 4 fixes that. If clippy complains about anything ELSE, address it; otherwise proceed.

- [ ] **Step 5: Commit**

The build is broken until Task 4. Don't commit yet — Task 4 fixes the callsite. Skip this step; the next commit in Task 4 will cover both changes.

---

## Task 4: Wire context-menu opening from sidebar right-clicks

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs` — rewrite right-click callbacks to populate `context_menu` instead of acting directly.

- [ ] **Step 1: Add `open_workflow_context_menu` + `open_agent_context_menu` methods**

In `crates/rupu-app/src/window/mod.rs`, add these methods to `impl WorkspaceWindow`, next to `handle_workflow_right_clicked` (which we'll keep — it's now invoked from a menu item rather than directly):

```rust
/// Open a context menu at `position` with workflow-row actions.
pub fn open_workflow_context_menu(
    &mut self,
    path: PathBuf,
    position: gpui::Point<gpui::Pixels>,
    cx: &mut Context<Self>,
) {
    let weak_run: WeakEntity<WorkspaceWindow> = cx.weak_entity();
    let path_for_run = path.clone();
    let run_item = crate::widget::ContextMenuItem {
        label: "Run\u{2026}".into(),
        on_select: Arc::new(move |_w, cx| {
            let weak = weak_run.clone();
            let path = path_for_run.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.handle_workflow_right_clicked(path, cx);
                });
            });
        }),
    };
    let weak_reveal: WeakEntity<WorkspaceWindow> = cx.weak_entity();
    let path_for_reveal = path.clone();
    let reveal_item = crate::widget::ContextMenuItem {
        label: "Reveal in Finder".into(),
        on_select: Arc::new(move |_w, cx| {
            let weak = weak_reveal.clone();
            let path = path_for_reveal.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.handle_workflow_reveal(path, cx);
                });
            });
        }),
    };
    self.context_menu = Some(crate::widget::ContextMenuState {
        position,
        items: vec![run_item, reveal_item],
    });
    cx.notify();
}

/// Open a context menu at `position` with agent-row actions.
pub fn open_agent_context_menu(
    &mut self,
    path: PathBuf,
    position: gpui::Point<gpui::Pixels>,
    cx: &mut Context<Self>,
) {
    let weak_open: WeakEntity<WorkspaceWindow> = cx.weak_entity();
    let path_for_open = path.clone();
    let open_item = crate::widget::ContextMenuItem {
        label: "Open in Editor".into(),
        on_select: Arc::new(move |_w, cx| {
            let weak = weak_open.clone();
            let path = path_for_open.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.handle_agent_clicked(path, cx);
                });
            });
        }),
    };
    let weak_reveal: WeakEntity<WorkspaceWindow> = cx.weak_entity();
    let path_for_reveal = path.clone();
    let reveal_item = crate::widget::ContextMenuItem {
        label: "Reveal in Finder".into(),
        on_select: Arc::new(move |_w, cx| {
            let weak = weak_reveal.clone();
            let path = path_for_reveal.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.handle_agent_reveal(path, cx);
                });
            });
        }),
    };
    self.context_menu = Some(crate::widget::ContextMenuState {
        position,
        items: vec![open_item, reveal_item],
    });
    cx.notify();
}

/// Reveal a workflow's YAML file in Finder. New companion to
/// `handle_agent_reveal`.
pub fn handle_workflow_reveal(&mut self, path: PathBuf, _cx: &mut Context<Self>) {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = std::process::Command::new("open")
            .args(["-R"])
            .arg(&path)
            .spawn()
        {
            tracing::warn!(?path, %e, "reveal workflow in Finder");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        tracing::info!(?path, "reveal: no-op on non-macOS");
    }
}
```

- [ ] **Step 2: Rewrite the right-click callback construction at the sidebar callsite**

In `crates/rupu-app/src/window/mod.rs`, find the existing sidebar `.child({ ... })` block in `Render::render`. The current `on_workflow_right_click` and `on_agent_right_click` builds use `WorkflowClickCb` / `AgentClickCb` (path-only). Replace them with `RightClickCb` (path + position) shape that opens the context menu:

```rust
let on_workflow_right_click: sidebar::RightClickCb =
    Arc::new(move |path, position, _w, cx| {
        let weak_sidebar_right = weak_sidebar_right.clone();
        cx.defer(move |cx| {
            let _ = weak_sidebar_right.update(cx, |this, cx| {
                this.open_workflow_context_menu(path, position, cx)
            });
        });
    });
let on_agent_right_click: sidebar::RightClickCb = Arc::new(move |path, position, _w, cx| {
    let weak = weak_agent_reveal.clone();
    cx.defer(move |cx| {
        let _ = weak.update(cx, |this, cx| {
            this.open_agent_context_menu(path, position, cx)
        });
    });
});
```

Both reuse the existing `weak_sidebar_right` / `weak_agent_reveal` clones declared earlier in the function; no new weak captures needed.

- [ ] **Step 3: Build + clippy**

```bash
cargo build -p rupu-app 2>&1 | tail -5
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
```

Expected: clean. Sidebar and window are now both updated to the new callback shape.

- [ ] **Step 4: Commit**

This commit covers both Task 3's signature change and Task 4's wiring (they're inseparable — both are required for the build to succeed).

```bash
git add crates/rupu-app/src/window/sidebar.rs crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): sidebar right-click opens context menu at mouse position"
```

---

## Task 5: Render the context-menu overlay

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs` — render the menu in `Render::render` when state is `Some`.

- [ ] **Step 1: Build the dismiss callback and overlay element**

In `crates/rupu-app/src/window/mod.rs`'s `Render::render`, find the section that builds the launcher overlay (the `let body: AnyElement = if let Some(state) = self.launcher.as_ref() { ... } else { main_layout.into_any_element() };` block — around the end of the function). The context menu overlay is the same idea but simpler: it always renders ON TOP of whatever `body` is (so it appears above the launcher if both are open).

Just before the final `body` expression returns, add:

```rust
// Context menu overlay — paints on top of everything (including the
// launcher sheet) when state.is_some(). Built as `deferred(...)` inside
// the widget so paint order is correct.
let weak_dismiss = weak_launcher.clone();
let on_dismiss: crate::widget::DismissCb = Arc::new(move |_w, cx| {
    let weak = weak_dismiss.clone();
    cx.defer(move |cx| {
        let _ = weak.update(cx, |this, cx| this.handle_dismiss_context_menu(cx));
    });
});

let body_with_context: AnyElement = if let Some(menu_state) = self.context_menu.as_ref() {
    div()
        .relative()
        .size_full()
        .child(body)
        .child(crate::widget::context_menu::render(menu_state, on_dismiss))
        .into_any_element()
} else {
    body
};
body_with_context
```

Replace the final `body` return with `body_with_context`.

Note: `weak_launcher` is already declared earlier in the render function (it's reused for the launcher overlay). We clone it again here. If `weak_launcher` doesn't exist for some reason in the post-Plan-6 file, add `let weak_context_dismiss = weak.clone();` near the other weak clones at the top of `render` and use that instead.

- [ ] **Step 2: Build + clippy**

```bash
cargo build -p rupu-app 2>&1 | tail -5
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): render context-menu overlay when state is set"
```

---

## Task 6: Esc-to-dismiss + smoke verification

**Files:**
- Modify: `crates/rupu-app/src/menu/app_menu.rs` — declare `DismissContextMenu` action + bind to Esc.
- Modify: `crates/rupu-app/src/window/mod.rs` — register the action handler.

- [ ] **Step 1: Declare the action**

In `crates/rupu-app/src/menu/app_menu.rs`, extend the `actions!(rupu_app, [...])` macro:

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
        DismissContextMenu,
    ]
);
```

Extend the `cx.bind_keys(vec![...])` block with:

```rust
KeyBinding::new("escape", DismissContextMenu, None),
```

(Add it alongside the other rupu_app-namespace bindings, not the TextInput-scoped ones.)

- [ ] **Step 2: Register the handler in `WorkspaceWindow::open`**

In `crates/rupu-app/src/window/mod.rs`, find the block of `cx.on_action(...)` registrations inside `WorkspaceWindow::open` (currently registers ApproveFocused, RejectFocused, ToggleSidebar, LaunchSelected). Add `DismissContextMenu` to the imported items in the top-of-file `use crate::menu::app_menu::...` line:

```rust
use crate::menu::app_menu::{
    ApproveFocused, DismissContextMenu, LaunchSelected, RejectFocused, ToggleSidebar,
};
```

Then add a handler beside the others:

```rust
let weak_dctx = entity.downgrade();
cx.on_action(move |_: &DismissContextMenu, cx| {
    let _ = weak_dctx.update(cx, |this, cx| {
        this.handle_dismiss_context_menu(cx);
    });
});
```

- [ ] **Step 3: Build + clippy + test**

```bash
cargo build -p rupu-app 2>&1 | tail -5
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
cargo test -p rupu-app --lib 2>&1 | tail -5
```

Expected: clean. All existing tests pass.

> **Esc and the launcher:** the launcher sheet doesn't currently listen for Esc to dismiss either. Esc now dismisses the context menu globally; if you press Esc while the menu is closed AND the launcher is open, nothing happens (`handle_dismiss_context_menu` is idempotent). A later task can extend `DismissContextMenu` to also close the launcher when no context menu is open; out of scope here.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/menu/app_menu.rs crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): Esc dismisses the context menu"
```

---

## Task 7: Add a smoke test for `open_workflow_context_menu`

**Files:**
- Modify: `crates/rupu-app/src/widget/context_menu.rs` — extend the test module with one more case.

- [ ] **Step 1: Add a test asserting state shape after building items**

The end-to-end (open menu → click item → dismiss) test requires a full GPUI smoke harness which doesn't exist in rupu-app yet. The most useful unit-level check is that constructing a `ContextMenuState` with the items we use is sound. Extend `crates/rupu-app/src/widget/context_menu.rs`'s `mod tests`:

```rust
#[test]
fn context_menu_state_with_two_items() {
    let mut item_a_count = 0;
    let mut item_b_count = 0;
    let _ = (&mut item_a_count, &mut item_b_count);
    let state = ContextMenuState {
        position: Point::new(px(50.0), px(60.0)),
        items: vec![
            ContextMenuItem {
                label: "Run\u{2026}".into(),
                on_select: Arc::new(|_w, _cx| {}),
            },
            ContextMenuItem {
                label: "Reveal in Finder".into(),
                on_select: Arc::new(|_w, _cx| {}),
            },
        ],
    };
    assert_eq!(state.items.len(), 2);
    assert_eq!(state.items[0].label.as_ref(), "Run\u{2026}");
    assert_eq!(state.items[1].label.as_ref(), "Reveal in Finder");
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p rupu-app --lib widget::context_menu 2>&1 | tail -5
cargo test -p rupu-app --lib 2>&1 | tail -3
```

Expected: 2 widget::context_menu tests pass; full crate test suite green.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/widget/context_menu.rs
git commit -m "test(app): ContextMenuState construction with multiple items"
```

---

## Wrap-up

- [ ] **Full crate verify**

```bash
cargo test -p rupu-app --lib
cargo clippy -p rupu-app --lib -- -D warnings
```

Expected: all tests pass; clippy clean.

- [ ] **Manual smoke test (matt)**

Boot `cargo run -p rupu-app`. Exercise:
- Right-click a workflow row. A small menu appears at the cursor: `Run…`, `Reveal in Finder`. Clicking `Run…` opens the launcher; clicking `Reveal in Finder` reveals the YAML in Finder; the menu dismisses after either.
- Right-click an agent row. Menu shows: `Open in Editor`, `Reveal in Finder`. Each item works; the menu dismisses on click.
- Right-click somewhere, then click OUTSIDE the menu (e.g., on the graph view). Menu dismisses.
- Right-click somewhere, then press Esc. Menu dismisses.
- Right-click near the right edge of the window. The menu snaps left to stay in-window.

- [ ] **Open the PR**

```bash
gh pr create --title "feat(app): right-click context menus for sidebar workflows + agents" --body "$(cat <<'EOF'
## Summary

Right-clicking a sidebar workflow or agent row now opens a small context menu at the cursor instead of immediately running an action.

- Workflow rows: \`Run…\`, \`Reveal in Finder\`.
- Agent rows: \`Open in Editor\`, \`Reveal in Finder\`.
- Menu dismisses on item selection, click-outside, or Esc.
- Auto-fits within the window edges (snaps to corners when near the right/bottom).

Implements [Plan 7](docs/superpowers/plans/2026-05-14-rupu-slice-d-plan-7-context-menus.md) across 6 task commits + 1 test commit.

## Test plan

- [x] cargo test -p rupu-app --lib — pass
- [x] cargo clippy -p rupu-app --lib -- -D warnings — clean
- [ ] matt's runtime validation per CLAUDE.md rupu-app rule #4
EOF
)"
```

---

## Self-review

**Spec coverage:** Each of matt's 2026-05-14 feedback items maps to a task:
- "right-click should open a menu not directly perform an action" → Tasks 3-4 redirect right-click into context-menu state; menu items dispatch the actual actions.
- "Esc to dismiss" (implicit standard) → Task 6.
- "Click outside to dismiss" → Task 1 (backdrop layer).
- "Edge-aware positioning" → Task 1 (`snap_to_window_with_margin`).

**Type consistency:** `RightClickCb` is the unified type for both workflow and agent right-clicks. `ContextMenuState` / `ContextMenuItem` / `DismissCb` are used consistently across widget + window. `gpui::Point<gpui::Pixels>` for click positions matches `MouseDownEvent.position`'s type.

**Placeholder scan:** No "TBD" / "implement later". Task 3 explicitly says NOT to commit between Tasks 3 and 4 (the signature change breaks compile until the callsite updates land), with rationale.

**Known unknowns called out inline:**
- Task 5 has a fallback for `weak_launcher` not existing in the post-Plan-6 file (suggests re-cloning from `weak`).
- Task 6 notes the launcher doesn't yet listen for Esc — explicitly out of scope.

**Risk:**
- The backdrop click-outside-to-dismiss approach absorbs ALL clicks anywhere on screen while the menu is open. If a user right-clicks the workflow row, then clicks Run in the launcher behind the menu... actually no, the menu is `deferred()` so it paints on top, and the backdrop only fills the menu's `absolute().inset_0()` parent — which is window-sized via the `deferred(div().absolute().inset_0()...)` wrapper. Click-outside means click outside the visible menu list, anywhere in the window. The next mouse-down after dismiss falls through to the regular handlers (after the next render frame). Standard pattern; should work.
- Task 7's test is shallow (state construction only). End-to-end requires a GPUI smoke harness that's not in scope for this plan. The integration behavior is matt-validated.
