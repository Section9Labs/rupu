//! Titlebar — stub for Task 12. Returns a placeholder.

use crate::palette;
use crate::workspace::Workspace;
use gpui::{IntoElement, div, prelude::*, px};

pub fn render(_workspace: &Workspace) -> impl IntoElement {
    div()
        .h(px(36.0))
        .bg(palette::BG_TITLEBAR)
        .border_b_1()
        .border_color(palette::BORDER)
        .child("titlebar (stub)")
}
