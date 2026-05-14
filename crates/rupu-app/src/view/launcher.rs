//! Launcher sheet — GPUI render for `LauncherState`. Pure function;
//! all interactions dispatch through callbacks injected by the window.
//!
//! Task 7 ships the skeleton (header + form-row stubs + footer stub
//! + error band). Task 8 fills in per-widget rendering.
//!
//! # Text-input strategy
//!
//! Text and number rows embed real `Entity<TextInput>` handles stored on
//! `LauncherState::text_inputs`. The entities are constructed by `open_launcher`
//! (Task 4) which also subscribes to `ContentChanged` to forward values back
//! into `LauncherState::inputs`. All other widget kinds (checkbox, select pills,
//! mode/target pickers) use the click-dispatch callback path.

use std::sync::Arc;

use gpui::{div, prelude::*, px, AnyElement, IntoElement, SharedString};

use crate::launcher::{CloneStatus, LauncherMode, LauncherState, LauncherTarget};
use crate::palette;

pub type InputChangeCb =
    Arc<dyn Fn(String, String, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type ModeChangeCb =
    Arc<dyn Fn(LauncherMode, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type TargetChangeCb =
    Arc<dyn Fn(LauncherTarget, &mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type RunCb = Arc<dyn Fn(&mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type CloseCb = Arc<dyn Fn(&mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;
pub type PickDirCb = Arc<dyn Fn(&mut gpui::Window, &mut gpui::App) + Send + Sync + 'static>;

pub fn render(
    state: &LauncherState,
    on_input_change: InputChangeCb,
    on_mode_change: ModeChangeCb,
    on_target_change: TargetChangeCb,
    on_pick_dir: PickDirCb,
    on_run: RunCb,
    on_close: CloseCb,
) -> AnyElement {
    let sheet = render_sheet(
        state,
        on_input_change,
        on_mode_change,
        on_target_change,
        on_pick_dir,
        on_run,
        on_close,
    );

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
    on_pick_dir: PickDirCb,
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
    sheet = sheet.child(render_footer(
        state,
        on_mode_change,
        on_target_change,
        on_pick_dir,
        on_run,
    ));

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
                .text_color(if checked {
                    palette::COMPLETE
                } else {
                    palette::TEXT_MUTED
                })
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
            .id(SharedString::from(format!(
                "select-{name_owned}-{opt_clone}"
            )))
            .px(px(8.0))
            .py(px(3.0))
            .border_1()
            .border_color(if is_selected {
                palette::BRAND
            } else {
                palette::BORDER
            })
            .bg(if is_selected {
                palette::BG_SIDEBAR
            } else {
                palette::BG_PRIMARY
            })
            .text_color(if is_selected {
                palette::BRAND_300
            } else {
                palette::TEXT_MUTED
            })
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
    on_pick_dir: PickDirCb,
    on_run: RunCb,
) -> AnyElement {
    let can_run = state.can_run();
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .items_center()
        .child(render_mode_picker(state.mode, on_mode_change))
        .child(render_target_picker(
            &state.target,
            on_target_change,
            on_pick_dir,
        ))
        .child(div().flex_grow())
        .child(render_run_button(can_run, on_run))
        .into_any_element()
}

/// Mode picker: three pills cycling Ask → Bypass → ReadOnly on click.
fn render_mode_picker(current: LauncherMode, on_mode_change: ModeChangeCb) -> AnyElement {
    let modes = [
        LauncherMode::Ask,
        LauncherMode::Bypass,
        LauncherMode::ReadOnly,
    ];
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
            .border_color(if is_selected {
                palette::BRAND
            } else {
                palette::BORDER
            })
            .bg(if is_selected {
                palette::BG_SIDEBAR
            } else {
                palette::BG_PRIMARY
            })
            .text_color(if is_selected {
                palette::BRAND_300
            } else {
                palette::TEXT_MUTED
            })
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

/// Target picker: three separate pills for ThisWorkspace / Directory / Clone.
/// - "This workspace" → sets target directly via `on_target_change`
/// - "Pick directory…" → invokes `on_pick_dir` which opens NSOpenPanel
/// - "Clone repo…" → sets target to Clone{NotStarted} via `on_target_change`
fn render_target_picker(
    current: &LauncherTarget,
    on_target_change: TargetChangeCb,
    on_pick_dir: PickDirCb,
) -> AnyElement {
    // "This workspace" pill
    let is_workspace = matches!(current, LauncherTarget::ThisWorkspace);
    let cb_ws = on_target_change.clone();
    let pill_workspace = div()
        .id("target-this-workspace")
        .px(px(8.0))
        .py(px(3.0))
        .border_1()
        .border_color(if is_workspace {
            palette::BRAND
        } else {
            palette::BORDER
        })
        .bg(if is_workspace {
            palette::BG_SIDEBAR
        } else {
            palette::BG_PRIMARY
        })
        .text_color(if is_workspace {
            palette::BRAND_300
        } else {
            palette::TEXT_MUTED
        })
        .text_sm()
        .cursor_pointer()
        .child("this workspace")
        .on_click(move |_ev, window, cx| {
            cb_ws(LauncherTarget::ThisWorkspace, window, cx);
        });

    // "Pick directory…" pill — label shows current dir name when active
    let is_dir = matches!(current, LauncherTarget::Directory(_));
    let dir_label: SharedString = match current {
        LauncherTarget::Directory(p) => SharedString::from(
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
        ),
        _ => SharedString::from("pick directory\u{2026}"),
    };
    let pill_dir = div()
        .id("target-pick-dir")
        .px(px(8.0))
        .py(px(3.0))
        .border_1()
        .border_color(if is_dir {
            palette::BRAND
        } else {
            palette::BORDER
        })
        .bg(if is_dir {
            palette::BG_SIDEBAR
        } else {
            palette::BG_PRIMARY
        })
        .text_color(if is_dir {
            palette::BRAND_300
        } else {
            palette::TEXT_MUTED
        })
        .text_sm()
        .cursor_pointer()
        .child(dir_label)
        .on_click(move |_ev, window, cx| {
            on_pick_dir(window, cx);
        });

    // "Clone repo…" pill — label shows repo_ref when active
    let is_clone = matches!(current, LauncherTarget::Clone { .. });
    let clone_label: SharedString = match current {
        LauncherTarget::Clone { repo_ref, status } => match status {
            CloneStatus::NotStarted => SharedString::from(format!("clone: {repo_ref}")),
            CloneStatus::InProgress => SharedString::from("cloning\u{2026}"),
            CloneStatus::Done(_) => SharedString::from(format!("cloned: {repo_ref}")),
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
        .child(clone_label)
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

    div()
        .flex()
        .flex_row()
        .gap(px(4.0))
        .child(pill_workspace)
        .child(pill_dir)
        .child(pill_clone)
        .into_any_element()
}

/// Run button: greyed + non-interactive when `!can_run`.
fn render_run_button(can_run: bool, on_run: RunCb) -> AnyElement {
    let bg = if can_run {
        palette::RUNNING
    } else {
        palette::BG_SIDEBAR
    };
    let text_color = if can_run {
        palette::TEXT_PRIMARY
    } else {
        palette::TEXT_DIMMEST
    };
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
