//! Sidebar — stub for Task 13. Returns a placeholder.

use crate::palette;
use crate::workspace::Workspace;
use gpui::{IntoElement, div, prelude::*, px};

pub fn render(_workspace: &Workspace) -> impl IntoElement {
    div()
        .w(px(180.0))
        .h_full()
        .bg(palette::BG_SIDEBAR)
        .border_r_1()
        .border_color(palette::BORDER)
        .child("sidebar (stub)")
}
