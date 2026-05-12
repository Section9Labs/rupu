//! Titlebar: color chip · workspace name · in-flight count badge.
//!
//! Per spec §6.1, the count is this-workspace only (the system
//! menubar in `menu/menubar.rs` carries the cross-workspace
//! count). For D-1 the count is hard-wired to 0; D-3 lights it
//! up when the executor lands.

use crate::palette;
use crate::workspace::Workspace;
use gpui::{div, prelude::*, px, FontWeight, IntoElement};

pub fn render(workspace: &Workspace) -> impl IntoElement {
    let chip_color = workspace.manifest.color.to_rgba();
    let in_flight = 0u32; // wired up in D-3

    div()
        .h(px(36.0))
        .bg(palette::BG_TITLEBAR)
        .border_b_1()
        .border_color(palette::BORDER)
        .px(px(14.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(
            // 10px color chip
            div().w(px(10.0)).h(px(10.0)).rounded_full().bg(chip_color),
        )
        .child(
            div()
                .text_color(palette::TEXT_PRIMARY)
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(workspace.manifest.name.clone()),
        )
        .child(
            // In-flight count — only renders when > 0. D-1 always shows nothing.
            if in_flight > 0 {
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
            },
        )
}
