//! Launcher sheet — GPUI render for `LauncherState`. Pure function;
//! all interactions dispatch through callbacks injected by the window.
//!
//! Task 7 ships the skeleton (header + form-row stubs + footer stub
//! + error band). Task 8 fills in per-widget rendering.

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

pub fn render(
    state: &LauncherState,
    _on_input_change: InputChangeCb,
    _on_mode_change: ModeChangeCb,
    _on_target_change: TargetChangeCb,
    _on_run: RunCb,
    _on_close: CloseCb,
) -> AnyElement {
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

    sheet = sheet.child(render_header(state));
    sheet = sheet.child(render_inputs_form(state));
    sheet = sheet.child(render_footer(state));

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
    for name in state.workflow.inputs.keys() {
        // Task 8 fills in per-widget rendering. For now, render a label
        // so the sheet structure is testable.
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
    // Task 8 fills in mode picker + target picker + Run button.
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .child(div().flex_grow())
        .child(div().px(px(12.0)).py(px(6.0)).child("Run"))
        .into_any_element()
}
