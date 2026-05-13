//! Launcher sheet — GPUI render for `LauncherState`. Pure function;
//! all interactions dispatch through callbacks injected by the window.
//!
//! Task 7 ships the skeleton (header + form-row stubs + footer stub
//! + error band). Task 8 fills in per-widget rendering.
//!
//! # Text-input strategy (Option B)
//!
//! The GPUI text-input primitive (see `crates/gpui/examples/input.rs`) requires
//! an `Entity<TextInput>` with a `FocusHandle` and must be rendered from a
//! stateful `Render` impl — it is not compatible with the pure-function render
//! pattern used here. For D-4 we therefore use **Option B**: text/number rows
//! render the current value as styled text plus a small "(edit)" affordance.
//! The `on_input_change` callback is still wired; a Task 18+ polish pass can
//! promote text rows to a real text-input entity once the window model holds
//! per-input `Entity<TextInput>` handles. All other widget kinds (checkbox,
//! select pills, mode/target pickers) use the full click-dispatch path.

use std::sync::Arc;

use gpui::{div, prelude::*, px, AnyElement, IntoElement, SharedString};

use crate::launcher::{CloneStatus, LauncherMode, LauncherTarget, LauncherState};
use crate::palette;

pub type InputChangeCb =
    Arc<dyn Fn(String, String, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type ModeChangeCb =
    Arc<dyn Fn(LauncherMode, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type TargetChangeCb =
    Arc<dyn Fn(LauncherTarget, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
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
    let sheet = render_sheet(state, on_input_change, on_mode_change, on_target_change, on_run, on_close);

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

fn render_sheet(
    state: &LauncherState,
    on_input_change: InputChangeCb,
    on_mode_change: ModeChangeCb,
    on_target_change: TargetChangeCb,
    on_run: RunCb,
    on_close: CloseCb,
) -> AnyElement {
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

    sheet = sheet.child(render_header(state, on_close));
    sheet = sheet.child(render_inputs_form(state, on_input_change));
    sheet = sheet.child(render_footer(state, on_mode_change, on_target_change, on_run));

    if let Some(err) = &state.validation {
        sheet = sheet.child(
            div()
                .text_color(palette::FAILED)
                .text_sm()
                .child(err.message.clone()),
        );
    } else if let LauncherTarget::Clone {
        status: CloneStatus::Failed(msg),
        ..
    } = &state.target
    {
        sheet = sheet.child(
            div()
                .text_color(palette::FAILED)
                .text_sm()
                .child(format!("clone failed: {msg}")),
        );
    }

    sheet.into_any_element()
}

fn render_header(state: &LauncherState, on_close: CloseCb) -> AnyElement {
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
        .child(
            div()
                .id("launcher-close")
                .px(px(8.0))
                .py(px(4.0))
                .text_color(palette::TEXT_MUTED)
                .cursor_pointer()
                .child("✕")
                .on_click(move |_ev, window, cx| {
                    on_close(window, cx);
                }),
        )
        .into_any_element()
}

fn render_inputs_form(state: &LauncherState, on_input_change: InputChangeCb) -> AnyElement {
    let mut form = div().flex().flex_col().gap(px(8.0));
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
    form.into_any_element()
}

/// Render a text string input row (Option B: value label + "(edit)" affordance).
/// A Task 18+ polish pass can promote this to a real Entity<TextInput>.
fn render_text_row(
    name: &str,
    current: &str,
    required: bool,
    on_input_change: InputChangeCb,
) -> AnyElement {
    let label = format_label(name, required);
    let display = if current.is_empty() {
        SharedString::from("—")
    } else {
        SharedString::from(current.to_owned())
    };
    // "(edit)" is a stub affordance; in a future polish pass this triggers
    // an overlay or inline text-input entity.
    let name_owned = name.to_owned();
    let current_owned = current.to_owned();
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
        .child(
            div()
                .flex_1()
                .text_color(palette::TEXT_PRIMARY)
                .text_sm()
                .child(display),
        )
        .child(
            div()
                .id(SharedString::from(format!("edit-{name_owned}")))
                .px(px(6.0))
                .py(px(2.0))
                .border_1()
                .border_color(palette::BORDER)
                .text_color(palette::TEXT_MUTED)
                .text_sm()
                .cursor_pointer()
                .child("edit")
                // Stub: cycles through a placeholder toggle so the callback
                // is exercised; real text entry deferred to Task 18+.
                .on_click(move |_ev, window, cx| {
                    // Toggle a placeholder so tests can see the callback fires.
                    let next = if current_owned.is_empty() {
                        "(value)".to_owned()
                    } else {
                        String::new()
                    };
                    on_input_change(name_owned.clone(), next, window, cx);
                }),
        )
        .into_any_element()
}

/// Render a numeric (Int) input row. Same Option B treatment as text.
fn render_number_row(
    name: &str,
    current: &str,
    required: bool,
    on_input_change: InputChangeCb,
) -> AnyElement {
    // Numbers share the same Option B affordance as strings; the callback
    // is still wired. A future polish pass can add an increment/decrement
    // control or a real text-input entity here.
    render_text_row(name, current, required, on_input_change)
}

/// Render a bool toggle as a clickable checkbox glyph.
fn render_checkbox_row(
    name: &str,
    current: &str,
    required: bool,
    on_input_change: InputChangeCb,
) -> AnyElement {
    let label = format_label(name, required);
    let checked = current == "true";
    let glyph = if checked { "☑" } else { "☐" };
    let name_owned = name.to_owned();
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
        .child(
            div()
                .id(SharedString::from(format!("checkbox-{name_owned}")))
                .px(px(6.0))
                .py(px(2.0))
                .text_color(if checked { palette::COMPLETE } else { palette::TEXT_MUTED })
                .cursor_pointer()
                .child(glyph)
                .on_click(move |_ev, window, cx| {
                    let next = if checked { "" } else { "true" };
                    on_input_change(name_owned.clone(), next.to_owned(), window, cx);
                }),
        )
        .into_any_element()
}

/// Render an enum (String with allowed values) as clickable option pills.
fn render_select_row(
    name: &str,
    allowed: &[String],
    current: &str,
    required: bool,
    on_input_change: InputChangeCb,
) -> AnyElement {
    let label = format_label(name, required);
    let name_owned = name.to_owned();
    let mut pills = div().flex().flex_row().gap(px(4.0)).flex_wrap();
    for option in allowed {
        let is_selected = option == current;
        let opt_clone = option.clone();
        let name_clone = name_owned.clone();
        let cb = on_input_change.clone();
        let pill = div()
            .id(SharedString::from(format!("select-{name_owned}-{opt_clone}")))
            .px(px(8.0))
            .py(px(3.0))
            .border_1()
            .border_color(if is_selected { palette::BRAND } else { palette::BORDER })
            .bg(if is_selected { palette::BG_SIDEBAR } else { palette::BG_PRIMARY })
            .text_color(if is_selected { palette::BRAND_300 } else { palette::TEXT_MUTED })
            .text_sm()
            .cursor_pointer()
            .child(SharedString::from(opt_clone.clone()))
            .on_click(move |_ev, window, cx| {
                cb(name_clone.clone(), opt_clone.clone(), window, cx);
            });
        pills = pills.child(pill);
    }
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
        .child(pills)
        .into_any_element()
}

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

/// Mode picker: three pills cycling Ask → Bypass → ReadOnly on click.
fn render_mode_picker(current: LauncherMode, on_mode_change: ModeChangeCb) -> AnyElement {
    let modes = [LauncherMode::Ask, LauncherMode::Bypass, LauncherMode::ReadOnly];
    let mut row = div().flex().flex_row().gap(px(4.0));
    for mode in modes {
        let is_selected = mode == current;
        // Each pill advances the cycle by one step.
        let next_mode = match mode {
            LauncherMode::Ask => LauncherMode::Bypass,
            LauncherMode::Bypass => LauncherMode::ReadOnly,
            LauncherMode::ReadOnly => LauncherMode::Ask,
        };
        let cb = on_mode_change.clone();
        let pill = div()
            .id(SharedString::from(format!("mode-{}", mode.as_str())))
            .px(px(8.0))
            .py(px(3.0))
            .border_1()
            .border_color(if is_selected { palette::BRAND } else { palette::BORDER })
            .bg(if is_selected { palette::BG_SIDEBAR } else { palette::BG_PRIMARY })
            .text_color(if is_selected { palette::BRAND_300 } else { palette::TEXT_MUTED })
            .text_sm()
            .cursor_pointer()
            .child(mode.as_str())
            .on_click(move |_ev, window, cx| {
                cb(next_mode, window, cx);
            });
        row = row.child(pill);
    }
    row.into_any_element()
}

/// Target picker: shows a summary pill for each variant; clicking cycles.
fn render_target_picker(current: &LauncherTarget, on_target_change: TargetChangeCb) -> AnyElement {
    let label = match current {
        LauncherTarget::ThisWorkspace => "this workspace".to_owned(),
        LauncherTarget::Directory(p) => p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string()),
        LauncherTarget::Clone { repo_ref, status } => match status {
            CloneStatus::NotStarted => format!("clone: {repo_ref}"),
            CloneStatus::InProgress => "cloning\u{2026}".to_string(),
            CloneStatus::Done(_) => format!("cloned: {repo_ref}"),
            CloneStatus::Failed(_) => "clone failed".to_string(),
        },
    };

    // Clicking cycles back to ThisWorkspace — sufficient for D-4;
    // a Task 18+ pass will add a proper target-selector sheet.
    let cb = on_target_change.clone();
    div()
        .id("target-picker")
        .px(px(8.0))
        .py(px(3.0))
        .border_1()
        .border_color(palette::BORDER)
        .text_color(palette::TEXT_MUTED)
        .text_sm()
        .cursor_pointer()
        .child(SharedString::from(label))
        .on_click(move |_ev, window, cx| {
            cb(LauncherTarget::ThisWorkspace, window, cx);
        })
        .into_any_element()
}

/// Run button: greyed + non-interactive when `!can_run`.
fn render_run_button(can_run: bool, on_run: RunCb) -> AnyElement {
    let bg = if can_run { palette::RUNNING } else { palette::BG_SIDEBAR };
    let text_color = if can_run { palette::TEXT_PRIMARY } else { palette::TEXT_DIMMEST };
    let mut btn = div()
        .id("launcher-run")
        .px(px(16.0))
        .py(px(6.0))
        .bg(bg)
        .text_color(text_color)
        .child("Run");
    if can_run {
        btn = btn.cursor_pointer().on_click(move |_ev, window, cx| {
            on_run(window, cx);
        });
    }
    btn.into_any_element()
}

/// Format a field label, appending `*` for required fields.
fn format_label(name: &str, required: bool) -> String {
    if required {
        format!("{name} *")
    } else {
        name.to_owned()
    }
}
