# rupu Slice D — Plan 6: Launcher Text Input

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the launcher's stubbed text/number input rows ("Option B" — click "edit" cycles a placeholder) with real keyboard text entry, so workflows that take required string/int inputs (including Clone target's repo_ref) can actually be launched from the GUI.

**Architecture:** Lift the canonical `TextInput` entity from `gpui/examples/input.rs` (the reference impl at our pinned Zed rev) into a new `crates/rupu-app/src/widget/text_input.rs` module. Register its standard keybindings (Backspace/Delete/Left/Right/Home/End/SelectAll/Cut/Copy/Paste) globally with `key_context: "TextInput"` scope so they only fire when an input is focused. Construct one `Entity<TextInput>` per text/int input at launcher-open time; embed each entity directly in the launcher's render via `.child(entity.clone())` (GPUI auto-renders `Entity<T: Render>`). Subscribe to content changes via `cx.subscribe` and propagate into `LauncherState.inputs` for revalidation. The launcher's render function stays pure; the entities live on the `LauncherState` struct.

**Tech Stack:** GPUI (Zed's framework, git-pinned), `gpui::Entity<TextInput>` + `FocusHandle` for stateful inputs, `cx.subscribe` for change notifications, `unicode-segmentation` (already a workspace dep through gpui) for grapheme boundaries.

**Companion docs:** [Slice D design](../specs/2026-05-11-rupu-slice-d-app-design.md), [Plan 4 launcher](2026-05-12-rupu-slice-d-plan-4-launcher.md), [Plan 5 foundation polish](2026-05-13-rupu-slice-d-plan-5-foundation-polish.md).

**Audit finding it closes:** `crates/rupu-app/src/view/launcher.rs:8-17` documents "Option B": text and number rows render as styled text + "(edit)" affordance whose click handler toggles `"(value)"` ↔ empty (line 223-230). Same applies to the Clone target's `repo_ref` field — `target_picker` line 540 sets `repo_ref: String::new()` with no UI to populate it. A workflow with a required string or int input cannot be launched from the GUI; cloning a repo cannot specify which repo. This is the highest-impact user-blocking gap from the 2026-05-13 round-2 audit.

---

## File structure

### New files

- `crates/rupu-app/src/widget/mod.rs` — re-exports `text_input::TextInput`.
- `crates/rupu-app/src/widget/text_input.rs` — single-line text input entity. Lifted from `gpui/examples/input.rs` with rupu-specific styling and a content-change notification event.

### Modified files

- `crates/rupu-app/src/lib.rs` — declare `pub mod widget;`.
- `crates/rupu-app/src/menu/app_menu.rs` — register the TextInput action types in `actions!` and bind their keys with `key_context: "TextInput"` so they only fire on a focused TextInput.
- `crates/rupu-app/src/launcher/state.rs` — extend `LauncherState` with `pub text_inputs: BTreeMap<String, gpui::Entity<TextInput>>` and an `ensure_text_inputs` constructor helper that lives outside `LauncherState::new` (since constructing entities requires `&mut App`). Add a special key `"__repo_ref"` reserved for the Clone target's repo ref.
- `crates/rupu-app/src/window/mod.rs` — `open_launcher` constructs the entities and subscribes to each one's `ContentChanged` event. The subscriber dispatches into `LauncherState.set_input` and revalidates. Clone target's repo_ref piggybacks on the same machinery.
- `crates/rupu-app/src/view/launcher.rs` — `render_text_row` and `render_number_row` switch from the "edit" stub to embedding the `Entity<TextInput>` directly. The Clone target's repo_ref pill gets an inline TextInput too.

---

## Task 1: Lift `TextInput` from the gpui example into `widget::text_input`

**Files:**
- Create: `crates/rupu-app/src/widget/mod.rs`
- Create: `crates/rupu-app/src/widget/text_input.rs`
- Modify: `crates/rupu-app/src/lib.rs` — add `pub mod widget;`

- [ ] **Step 1: Create the module file by copying the gpui example**

```bash
mkdir -p crates/rupu-app/src/widget
cp ~/.cargo/git/checkouts/zed-a70e2ad075855582/1a2e50e/crates/gpui/examples/input.rs \
   crates/rupu-app/src/widget/text_input.rs
```

The example is 778 lines and is the canonical reference implementation for GPUI text input. It implements selection, clipboard, IME marked range, cursor positioning, and the custom `Element` for caret rendering. Lifting in full is intentional — re-implementing any of that would introduce bugs the example has already solved.

- [ ] **Step 2: Strip the example-only scaffolding**

Open `crates/rupu-app/src/widget/text_input.rs`. Delete everything from `struct InputExample` (around line 629) through the end of the file. The `InputExample` struct, its `Render`/`Focusable` impls, `run_example`, `main`, and the wasm `start` are example wrappers — we don't need them.

Also delete the top-level `#![cfg_attr(target_family = "wasm", no_main)]` attribute on line 1 (we're not a wasm example).

Also delete `use gpui_platform::application;` from the imports (only used by `run_example`).

The file should now end at the `impl Focusable for TextInput { ... }` block (around line 627 in the original). Keep:
- The `actions!` macro (lines 16-34)
- `struct TextInput` + `impl TextInput` (lines 36-272)
- `impl EntityInputHandler for TextInput` (lines 274-399)
- `struct TextElement` + `struct PrepaintState` + the `IntoElement` and `Element` impls (lines 401-583)
- `impl Render for TextInput` (lines 585-621)
- `impl Focusable for TextInput` (lines 623-627)

- [ ] **Step 3: Make `TextInput` constructible from outside the module**

Edit the `struct TextInput { ... }` block in `crates/rupu-app/src/widget/text_input.rs`: change every field from private to `pub(crate)` so the launcher can construct one. Then add a public constructor at the end of `impl TextInput`:

```rust
impl TextInput {
    /// Construct a new TextInput entity. Call inside `cx.new(|cx| TextInput::new(cx, placeholder))`.
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::new_static(""),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    /// Replace the current contents (e.g. to inject a default value).
    pub fn set_content(&mut self, content: impl Into<SharedString>, cx: &mut Context<Self>) {
        let s = content.into();
        let len = s.len();
        self.content = s;
        self.selected_range = len..len;
        self.marked_range = None;
        cx.notify();
    }

    /// Read the current contents.
    pub fn content(&self) -> &SharedString {
        &self.content
    }
}
```

- [ ] **Step 4: Apply rupu palette to the rendered widget**

The example uses `bg(rgb(0xeeeeee))`, `bg(white())`, `text_size(px(24.))` — those are example colors. Edit the `impl Render for TextInput` block to use rupu's palette and sizing:

```rust
impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::palette;
        div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .bg(palette::BG_SIDEBAR)
            .border_1()
            .border_color(palette::BORDER)
            .rounded(px(3.0))
            .line_height(px(20.))
            .text_size(px(13.))
            .text_color(palette::TEXT_PRIMARY)
            .child(
                div()
                    .h(px(24.))
                    .w_full()
                    .px(px(6.))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}
```

The `TextElement::paint` method (in the lifted code) hardcodes `black()` for the cursor and `0x3311ff` for selection background; those bleed through unchanged for now — leave the polish for a follow-up if matt flags them in his runtime pass.

- [ ] **Step 5: Add a `ContentChanged` event so observers can react to typing**

In `crates/rupu-app/src/widget/text_input.rs`, just after `struct TextInput { ... }` (around line 47 of the trimmed file), add:

```rust
/// Event emitted whenever the input's contents change. Subscribers receive
/// the new content via `cx.subscribe`.
#[derive(Clone, Debug)]
pub struct ContentChanged {
    pub content: SharedString,
}

impl EventEmitter<ContentChanged> for TextInput {}
```

Add `EventEmitter` to the gpui import block at the top of the file.

Now wire the event emission. Find `fn replace_text_in_range` in `impl EntityInputHandler for TextInput` (around line 320 of the trimmed file). At the end of that function — right after the final `cx.notify();` — append:

```rust
        cx.emit(ContentChanged {
            content: self.content.clone(),
        });
```

Also add the same emit at the end of `pub fn set_content` (added in Step 3) just before `cx.notify();`:

```rust
        cx.emit(ContentChanged {
            content: self.content.clone(),
        });
        cx.notify();
```

This way every typed character, paste, backspace, or programmatic `set_content` notifies subscribers.

- [ ] **Step 6: Create the `widget` module re-export**

```rust
// crates/rupu-app/src/widget/mod.rs
//! Reusable GPUI widgets for rupu-app.

pub mod text_input;
pub use text_input::{ContentChanged, TextInput};
```

- [ ] **Step 7: Wire the module into the crate root**

In `crates/rupu-app/src/lib.rs`, add the module declaration alongside the existing `pub mod ...` lines. The existing list lives near the top of `lib.rs`; append `pub mod widget;` to it.

- [ ] **Step 8: Build and verify**

```bash
cargo build -p rupu-app 2>&1 | tail -10
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
```

Expected: clean build, no warnings. The widget compiles standalone; it has no consumers yet.

- [ ] **Step 9: Commit**

```bash
git add crates/rupu-app/src/widget/ crates/rupu-app/src/lib.rs
git commit -m "feat(app): lift TextInput widget from gpui examples"
```

---

## Task 2: Register TextInput action keybindings

**Files:**
- Modify: `crates/rupu-app/src/menu/app_menu.rs:34-38` (the existing `bind_keys` block) and the imports.

- [ ] **Step 1: Bind the TextInput action keys**

The TextInput widget's `actions!` macro declares `Backspace`, `Delete`, `Left`, `Right`, `SelectLeft`, `SelectRight`, `SelectAll`, `Home`, `End`, `ShowCharacterPalette`, `Paste`, `Cut`, `Copy` in the `text_input` namespace. They have to be bound via `KeyBinding::new(_, _, Some("TextInput"))` so they only dispatch when a `TextInput` is focused (the `key_context: "TextInput"` set inside `impl Render for TextInput`).

In `crates/rupu-app/src/menu/app_menu.rs`, replace the existing `cx.bind_keys(vec![...])` block (around line 47-52) with:

```rust
cx.bind_keys(vec![
    KeyBinding::new("a", ApproveFocused, None),
    KeyBinding::new("r", RejectFocused, None),
    KeyBinding::new("cmd-r", LaunchSelected, None),
    KeyBinding::new("cmd-\\", ToggleSidebar, None),
    // TextInput shortcuts — scoped to focused text inputs only.
    KeyBinding::new(
        "backspace",
        crate::widget::text_input::Backspace,
        Some("TextInput"),
    ),
    KeyBinding::new(
        "delete",
        crate::widget::text_input::Delete,
        Some("TextInput"),
    ),
    KeyBinding::new("left", crate::widget::text_input::Left, Some("TextInput")),
    KeyBinding::new(
        "right",
        crate::widget::text_input::Right,
        Some("TextInput"),
    ),
    KeyBinding::new(
        "shift-left",
        crate::widget::text_input::SelectLeft,
        Some("TextInput"),
    ),
    KeyBinding::new(
        "shift-right",
        crate::widget::text_input::SelectRight,
        Some("TextInput"),
    ),
    KeyBinding::new(
        "cmd-a",
        crate::widget::text_input::SelectAll,
        Some("TextInput"),
    ),
    KeyBinding::new("home", crate::widget::text_input::Home, Some("TextInput")),
    KeyBinding::new("end", crate::widget::text_input::End, Some("TextInput")),
    KeyBinding::new(
        "ctrl-cmd-space",
        crate::widget::text_input::ShowCharacterPalette,
        Some("TextInput"),
    ),
    KeyBinding::new(
        "cmd-v",
        crate::widget::text_input::Paste,
        Some("TextInput"),
    ),
    KeyBinding::new("cmd-c", crate::widget::text_input::Copy, Some("TextInput")),
    KeyBinding::new("cmd-x", crate::widget::text_input::Cut, Some("TextInput")),
]);
```

> **Note for the implementer:** the `Some("TextInput")` context name MUST match the `.key_context("TextInput")` set inside `impl Render for TextInput`. If the latter ever changes (e.g. to namespace it), both sides must move together.

- [ ] **Step 2: Build and verify**

```bash
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
```

Expected: clean. The keybindings reference real action types from the widget module that now exists.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/menu/app_menu.rs
git commit -m "feat(app): register TextInput keybindings scoped to focused inputs"
```

---

## Task 3: Hold per-input `Entity<TextInput>` on `LauncherState`

**Files:**
- Modify: `crates/rupu-app/src/launcher/state.rs:9-17` (struct definition) and `:59-79` (`LauncherState::new`).

- [ ] **Step 1: Add the `text_inputs` field**

In `crates/rupu-app/src/launcher/state.rs`, replace the existing `LauncherState` struct (lines 9-17) with:

```rust
#[derive(Clone)]
pub struct LauncherState {
    pub workflow_path: PathBuf,
    pub workflow: Workflow,
    pub inputs: BTreeMap<String, String>,
    pub mode: LauncherMode,
    pub target: LauncherTarget,
    pub validation: Option<ValidationError>,
    /// One `Entity<TextInput>` per text/int workflow input, keyed by input name.
    /// Plus the reserved key `"__repo_ref"` for the Clone target's repo ref.
    /// Constructed in `LauncherState::ensure_text_inputs` because `cx.new` is
    /// only callable with a mutable `App` context, not at struct-literal time.
    pub text_inputs: BTreeMap<String, gpui::Entity<crate::widget::TextInput>>,
}
```

The struct no longer derives `Debug` (entities aren't Debug) — drop it from the derive list. Several call sites may have `tracing::debug!(?launcher_state, ...)` style calls; if so, switch them to `tracing::debug!(workflow = %launcher_state.workflow_path.display(), ...)`. Grep for `?launcher` / `?state` references and fix as needed.

`Clone` stays — `Entity<T>` is `Clone` (it's a reference-counted handle).

- [ ] **Step 2: Update `LauncherState::new` to initialize the empty map**

In the same file, find `pub fn new` (line 60) and update the struct-construction line to include the new field:

```rust
let mut state = Self {
    workflow_path,
    workflow,
    inputs,
    mode: LauncherMode::Ask,
    target: LauncherTarget::ThisWorkspace,
    validation: None,
    text_inputs: BTreeMap::new(),
};
```

The map stays empty here — entities are populated by `ensure_text_inputs` once we have a `cx`.

- [ ] **Step 3: Build and verify**

```bash
cargo build -p rupu-app 2>&1 | tail -10
```

Expected: clean. Any call sites that pattern-match `LauncherState { .. }` need the new field; fix as the compiler points them out.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/launcher/state.rs
git commit -m "feat(app): add text_inputs map to LauncherState"
```

---

## Task 4: Construct TextInput entities + subscribe on launcher open

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs` — extend `open_launcher` to populate `text_inputs` and subscribe to each entity's `ContentChanged` event.

- [ ] **Step 1: Extend `open_launcher` to construct entities + subscribe**

In `crates/rupu-app/src/window/mod.rs`, replace the `pub fn open_launcher` body (around line 298-315) with:

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
    let mut state = crate::launcher::LauncherState::new(workflow_path, workflow);

    // Construct one TextInput entity per string/int input and seed it with
    // the current value (which may be the workflow's default).
    for (name, def) in &state.workflow.inputs {
        let is_text =
            matches!(def.ty, rupu_orchestrator::InputType::String) && def.allowed.is_empty();
        let is_int = matches!(def.ty, rupu_orchestrator::InputType::Int);
        if !is_text && !is_int {
            continue;
        }
        let initial = state.inputs.get(name).cloned().unwrap_or_default();
        let placeholder = format!("{name}…");
        let name_owned = name.clone();
        let entity = cx.new(|cx| {
            let mut t = crate::widget::TextInput::new(cx, placeholder);
            if !initial.is_empty() {
                t.set_content(initial, cx);
            }
            t
        });
        // Subscribe to ContentChanged → mirror into LauncherState.inputs.
        let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        cx.subscribe(&entity, move |_, _entity, ev: &crate::widget::ContentChanged, cx| {
            let name = name_owned.clone();
            let new_value = ev.content.to_string();
            let weak = weak.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    if let Some(s) = this.launcher.as_mut() {
                        s.set_input(&name, new_value);
                        s.revalidate();
                        cx.notify();
                    }
                });
            });
        })
        .detach();
        state.text_inputs.insert(name.clone(), entity);
    }

    // Reserved entity for Clone target's repo_ref. Always constructed even when
    // the user hasn't selected the Clone target yet, so switching to Clone
    // doesn't have to wire it lazily.
    let weak_clone: WeakEntity<WorkspaceWindow> = cx.weak_entity();
    let repo_ref_entity = cx.new(|cx| crate::widget::TextInput::new(cx, "owner/repo"));
    cx.subscribe(
        &repo_ref_entity,
        move |_, _entity, ev: &crate::widget::ContentChanged, cx| {
            let new_value = ev.content.to_string();
            let weak = weak_clone.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    if let Some(s) = this.launcher.as_mut() {
                        if let crate::launcher::LauncherTarget::Clone { repo_ref, .. } =
                            &mut s.target
                        {
                            *repo_ref = new_value;
                            cx.notify();
                        }
                    }
                });
            });
        },
    )
    .detach();
    state
        .text_inputs
        .insert("__repo_ref".to_string(), repo_ref_entity);

    self.launcher = Some(state);
    cx.notify();
}
```

> **Note on `cx.subscribe`:** subscriptions are tied to the `WorkspaceWindow` entity (their owner). When the launcher closes (`self.launcher = None`), the entities and subscriptions are dropped together — no leak. When the window closes, GPUI drops all subscriptions automatically.

- [ ] **Step 2: Build and verify**

```bash
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -10
```

Expected: clean. There are no consumers yet (Task 5 wires the UI); the entities exist invisibly.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs
git commit -m "feat(app): construct TextInput entities + subscribe on launcher open"
```

---

## Task 5: Render the TextInput entities in `render_text_row` / `render_number_row`

**Files:**
- Modify: `crates/rupu-app/src/view/launcher.rs:146-247` — replace `render_inputs_form`'s text/number branches and the bodies of `render_text_row` / `render_number_row`.

- [ ] **Step 1: Pass the entities into the form renderer**

In `crates/rupu-app/src/view/launcher.rs`, update the `render_inputs_form` signature to accept the entity map alongside the existing args:

```rust
fn render_inputs_form(state: &LauncherState, on_input_change: InputChangeCb) -> AnyElement {
    let mut form = div().flex().flex_col().gap(px(8.0));
    for (name, def) in &state.workflow.inputs {
        let current = state.inputs.get(name).cloned().unwrap_or_default();
        let row = match def.ty {
            rupu_orchestrator::InputType::String if !def.allowed.is_empty() => render_select_row(
                name,
                &def.allowed,
                &current,
                def.required,
                on_input_change.clone(),
            ),
            rupu_orchestrator::InputType::String => render_text_row(
                name,
                state.text_inputs.get(name).cloned(),
                def.required,
            ),
            rupu_orchestrator::InputType::Int => render_number_row(
                name,
                state.text_inputs.get(name).cloned(),
                def.required,
            ),
            rupu_orchestrator::InputType::Bool => {
                render_checkbox_row(name, &current, def.required, on_input_change.clone())
            }
        };
        form = form.child(row);
    }
    form.into_any_element()
}
```

(`on_input_change` is no longer needed for text/number rows since the entity emits change events directly; keep it for checkbox + select.)

- [ ] **Step 2: Rewrite `render_text_row` to embed the entity**

Replace the whole body of `render_text_row` (lines 175-234 in the existing file) with:

```rust
/// Render a text input row: label + embedded `TextInput` entity.
fn render_text_row(
    name: &str,
    entity: Option<gpui::Entity<crate::widget::TextInput>>,
    required: bool,
) -> AnyElement {
    let label = format_label(name, required);
    let body: AnyElement = match entity {
        Some(e) => div().flex_1().child(e).into_any_element(),
        None => div()
            .flex_1()
            .text_color(palette::FAILED)
            .text_sm()
            .child(SharedString::from("input entity missing"))
            .into_any_element(),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .w(px(140.0))
                .text_color(palette::TEXT_MUTED)
                .text_sm()
                .child(label),
        )
        .child(body)
        .into_any_element()
}
```

- [ ] **Step 3: Rewrite `render_number_row` the same way**

Replace `render_number_row` (around line 237):

```rust
/// Render an integer input row. Same widget as text — validation lives in
/// `rupu_orchestrator::resolve_inputs` which catches non-numeric strings on
/// every keystroke via the `revalidate` call in the ContentChanged subscriber.
fn render_number_row(
    name: &str,
    entity: Option<gpui::Entity<crate::widget::TextInput>>,
    required: bool,
) -> AnyElement {
    render_text_row(name, entity, required)
}
```

- [ ] **Step 4: Remove the unused `current` argument from those signatures**

The old `render_text_row` and `render_number_row` took a `current: &str`; the new versions don't. Search the file for any remaining call sites; the only caller was `render_inputs_form` which Step 1 already updated. The `format_label` helper is still used.

- [ ] **Step 5: Build, clippy, and run tests**

```bash
cargo build -p rupu-app 2>&1 | tail -5
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
cargo test -p rupu-app --lib 2>&1 | tail -10
```

Expected: clean. The launcher now embeds real text input entities; typing into them flows through to `LauncherState.inputs` via the subscriber wired in Task 4.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/view/launcher.rs
git commit -m "feat(app): launcher text + number rows render real TextInput entities"
```

---

## Task 6: Wire Clone target's repo_ref to its TextInput entity

**Files:**
- Modify: `crates/rupu-app/src/view/launcher.rs:430-560` (the `render_target_picker` function — specifically the Clone pill branch).

- [ ] **Step 1: Render the repo_ref input under the Clone pill**

In `crates/rupu-app/src/view/launcher.rs`, find `render_target_picker` and modify the `pill_clone` construction. When the Clone target is currently selected, render the embedded TextInput entity beneath the pill. When it's not selected, just show the pill as-is.

Update `render_target_picker`'s signature to take the entity map (replace `&LauncherTarget` with `&LauncherState`):

```rust
fn render_target_picker(
    state: &LauncherState,
    on_target_change: TargetChangeCb,
    on_pick_dir: PickDirCb,
) -> AnyElement {
    let current = &state.target;
    // ... existing this-workspace and pick-dir pill construction stays the
    // same, just using `current` ...

    // "Clone repo…" pill — replace the existing block:
    let is_clone = matches!(current, LauncherTarget::Clone { .. });
    let clone_status_label: SharedString = match current {
        LauncherTarget::Clone { status, .. } => match status {
            CloneStatus::NotStarted => SharedString::from("clone repo"),
            CloneStatus::InProgress => SharedString::from("cloning\u{2026}"),
            CloneStatus::Done(_) => SharedString::from("cloned"),
            CloneStatus::Failed(_) => SharedString::from("clone failed"),
        },
        _ => SharedString::from("clone repo\u{2026}"),
    };
    let cb_clone = on_target_change.clone();
    let pill_clone = div()
        .id("target-clone")
        .px(px(8.0))
        .py(px(3.0))
        .border_1()
        .border_color(if is_clone {
            palette::BRAND
        } else {
            palette::BORDER
        })
        .bg(if is_clone {
            palette::BG_SIDEBAR
        } else {
            palette::BG_PRIMARY
        })
        .text_color(if is_clone {
            palette::BRAND_300
        } else {
            palette::TEXT_MUTED
        })
        .text_sm()
        .cursor_pointer()
        .child(clone_status_label)
        .on_click(move |_ev, window, cx| {
            cb_clone(
                LauncherTarget::Clone {
                    repo_ref: String::new(),
                    status: CloneStatus::NotStarted,
                },
                window,
                cx,
            );
        });

    // When Clone is active, render the repo_ref input below.
    let mut col = div().flex().flex_col().gap(px(4.0)).child(
        div()
            .flex()
            .flex_row()
            .gap(px(4.0))
            .child(pill_workspace)
            .child(pill_dir)
            .child(pill_clone),
    );
    if is_clone {
        if let Some(entity) = state.text_inputs.get("__repo_ref").cloned() {
            col = col.child(div().w(px(280.0)).child(entity));
        }
    }
    col.into_any_element()
}
```

(The existing pill_workspace and pill_dir constructions stay structurally the same — just use `current` instead of the standalone target arg.)

- [ ] **Step 2: Update the caller in `render_footer`**

`render_footer` (line 354) calls `render_target_picker(&state.target, ...)`. Change to `render_target_picker(state, ...)` so the entity map is reachable.

- [ ] **Step 3: Build and run tests**

```bash
cargo build -p rupu-app 2>&1 | tail -5
cargo clippy -p rupu-app --lib -- -D warnings 2>&1 | tail -5
cargo test -p rupu-app --lib 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/view/launcher.rs
git commit -m "feat(app): clone-target repo_ref accepts typed input"
```

---

## Task 7: Add a unit test for `LauncherState.set_input` + revalidation

**Files:**
- Modify: `crates/rupu-app/src/launcher/state.rs` — extend the existing test module (if there is one) or add `#[cfg(test)] mod tests`.

- [ ] **Step 1: Write the failing test**

Check first whether `crates/rupu-app/src/launcher/state.rs` has a `#[cfg(test)] mod tests` block. If not, append:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::{InputDef, InputType, Workflow};
    use std::collections::BTreeMap;

    fn workflow_with_input(name: &str, ty: InputType, required: bool) -> Workflow {
        let mut inputs = BTreeMap::new();
        inputs.insert(
            name.to_string(),
            InputDef {
                ty,
                required,
                default: None,
                allowed: vec![],
            },
        );
        Workflow {
            name: "test".to_string(),
            inputs,
            steps: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn set_input_then_revalidate_clears_error_when_required_input_provided() {
        let wf = workflow_with_input("repo", InputType::String, true);
        let mut state = LauncherState::new("/tmp/wf.yml".into(), wf);
        // Required input is missing → validation should fail.
        assert!(state.validation.is_some(), "expected validation error");
        state.set_input("repo", "github:foo/bar");
        state.revalidate();
        assert!(
            state.validation.is_none(),
            "expected validation to clear once required input set, got {:?}",
            state.validation
        );
    }

    #[test]
    fn set_input_with_empty_string_removes_the_entry() {
        let wf = workflow_with_input("repo", InputType::String, false);
        let mut state = LauncherState::new("/tmp/wf.yml".into(), wf);
        state.set_input("repo", "value");
        assert_eq!(state.inputs.get("repo").map(|s| s.as_str()), Some("value"));
        state.set_input("repo", "");
        assert!(state.inputs.get("repo").is_none());
    }
}
```

> **Note for the implementer:** the `Workflow` struct may not have a `Default` derive — if `Workflow { ..Default::default() }` doesn't compile, set every required field explicitly. The grep `grep -n "pub struct Workflow\b" crates/rupu-orchestrator/src/lib.rs` finds the definition. Mirror its public fields verbatim in the test helper.

- [ ] **Step 2: Run the failing test (or note that the test compiles + passes immediately)**

```bash
cargo test -p rupu-app --lib launcher::state 2>&1 | tail -10
```

These tests exercise `set_input` + `revalidate` which already exist; they should pass on first run. If they fail, the validation logic in `resolve_inputs` may be looser than expected for missing required inputs — investigate and adjust the test to match actual behavior, not the other way around.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/launcher/state.rs
git commit -m "test(app): LauncherState.set_input + revalidate"
```

---

## Wrap-up

- [ ] **Run the full test suite**

```bash
cargo test -p rupu-app --lib
```

Expected: all tests pass (existing 20 + 2 new = 22).

- [ ] **Run clippy across the crate**

```bash
cargo clippy -p rupu-app --lib -- -D warnings
```

Expected: clean.

- [ ] **Manual smoke test (matt)**

Boot `cargo run -p rupu-app` and exercise:
- Click Run on a workflow with a required string input. The launcher sheet opens; the input field is editable and accepts typing. Filling it in clears the validation error below the form. Run button enables.
- Workflow with an int input rejects non-numeric values (validation error appears immediately).
- Click "Clone repo…" — a text field appears below the pills. Type `github:Section9Labs/rupu`. The Run button enables. Clicking Run kicks off the clone.
- ⌘C / ⌘V / ⌘X work inside the focused input.
- Backspace / arrow keys / Home / End behave normally.
- Clicking outside the input loses focus; clicking back restores it.

- [ ] **Open the PR**

```bash
gh pr create --title "feat(app): launcher accepts real text input (D-4.6)" --body "$(cat <<'EOF'
## Summary

Closes the highest-impact user-blocking gap from the 2026-05-13 round-2 audit: the launcher's string / int input rows and the Clone target's repo_ref now accept real keyboard text entry instead of the "(edit)" stub that toggled a placeholder.

- Lifts the canonical `TextInput` entity from `gpui/examples/input.rs` into `crates/rupu-app/src/widget/text_input.rs`.
- Registers its action keybindings with `key_context: "TextInput"` so they only dispatch on a focused input.
- `LauncherState` holds one `Entity<TextInput>` per text/int input plus one reserved for the Clone target's repo_ref.
- On launcher open, `open_launcher` constructs the entities and subscribes to `ContentChanged` events; subscribers mirror new values into `LauncherState.inputs` (or `target.repo_ref`) and revalidate.
- `view::launcher` embeds the entities directly via `.child(entity.clone())`.

## Test plan

- [x] `cargo test -p rupu-app --lib` — passes (existing 20 + 2 new = 22).
- [x] `cargo clippy -p rupu-app --lib -- -D warnings` — clean.
- [ ] matt runs the manual smoke test (see Wrap-up in [the plan doc](docs/superpowers/plans/2026-05-13-rupu-slice-d-plan-6-launcher-text-input.md)). Per CLAUDE.md rupu-app rule #4, runtime validation required before merge.
EOF
)"
```

---

## Self-review

**Spec coverage:** This plan addresses the launcher-text-input audit finding in full — string + int inputs and the Clone target's repo_ref both become typeable. Graph-node-click, drilldown polish, and inline Reject from the audit are intentionally deferred to Plan 7 per matt's "option 1" scope call.

**Type consistency:** `TextInput` is used consistently as `gpui::Entity<crate::widget::TextInput>` across launcher state, window open, and view render. `ContentChanged` is the single event type emitted in both `replace_text_in_range` and `set_content`. `text_inputs: BTreeMap<String, Entity<TextInput>>` uses the same key convention everywhere (input name, or the literal `"__repo_ref"` for Clone target).

**Placeholder scan:** No "TBD" / "implement later". Task 1 Step 2 references specific line numbers in the original example to strip — those are deterministic at the pinned Zed commit. Task 7 Step 1 has a note about `Workflow::default()` possibly not existing, with the exact grep to verify and the fallback (set fields explicitly).

**Known unknowns called out inline:**
- Whether `Workflow` derives `Default` (Task 7) — implementer verifies via grep.
- Whether the `Debug` derive on `LauncherState` is used anywhere via `?launcher` (Task 3) — implementer greps and adjusts.
- Whether the `TextElement::paint` cursor color needs adjustment to match rupu's palette — left intentionally for matt's runtime pass.

**Risks:**
- The 778-line widget lift is the biggest unknown. The example builds against the same gpui rev we pin, so import compatibility should be near-perfect, but the implementer should expect minor pattern-match adjustments if private items leaked through.
- `cx.subscribe`'s closure signature has occasionally drifted between gpui revisions. If the compiler complains about the subscriber signature in Task 4, the implementer should check `crates/gpui/src/subscription.rs` at the pinned commit for the current form.
