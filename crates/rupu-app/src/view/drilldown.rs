//! Drill-down pane — focused step's transcript stream + approval bar.
//!
//! Renders only when `model.focused_step` is `Some`. The approval bar
//! appears when the focused step's status is `Awaiting`.
//!
//! Approval button interactions (Task 17) are wired via handlers in
//! `window/mod.rs`; this module is pure rendering.

use gpui::{div, prelude::*, px, AnyElement, IntoElement};

use crate::palette;
use crate::run_model::RunModel;
use crate::view::transcript_tail::TranscriptLine;

/// Render the drill-down pane. Returns an empty element when no step is
/// focused so the main area does not allocate any pane width.
pub fn render(model: &RunModel, transcript: &[TranscriptLine]) -> AnyElement {
    let focused_id = match &model.focused_step {
        Some(id) => id.clone(),
        None => return div().into_any_element(),
    };
    let status = model.nodes.get(&focused_id).copied();

    let mut pane = div()
        .flex()
        .flex_col()
        .w(px(420.0))
        .h_full()
        .bg(palette::BG_PRIMARY)
        .border_l_1()
        .border_color(palette::BORDER);

    // Header row: step id on the left, status glyph on the right.
    pane = pane.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .px(px(16.0))
            .py(px(12.0))
            .child(
                div()
                    .text_color(palette::TEXT_PRIMARY)
                    .font_family("Menlo")
                    .child(focused_id.clone()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_color(palette::TEXT_MUTED)
                    .child(status.map(|s| s.glyph().to_string()).unwrap_or_default()),
            ),
    );

    // Approval bar — only shown while the step is awaiting approval.
    if status == Some(rupu_app_canvas::NodeStatus::Awaiting) {
        pane = pane.child(approval_bar());
    }

    // Transcript body — scrollable list of raw transcript lines.
    let mut log = div().flex().flex_col().px(px(16.0)).py(px(8.0));
    for line in transcript {
        log = log.child(
            div()
                .text_color(palette::TEXT_PRIMARY)
                .font_family("Menlo")
                .text_sm()
                .child(format!("• {} {}", line.kind, line.payload)),
        );
    }
    pane = pane.child(log);

    pane.into_any_element()
}

fn approval_bar() -> AnyElement {
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(8.0))
        .child(
            div()
                .px(px(12.0))
                .py(px(6.0))
                .bg(palette::COMPLETE)
                .text_color(palette::TEXT_PRIMARY)
                .child("Approve"),
        )
        .child(
            div()
                .px(px(12.0))
                .py(px(6.0))
                .bg(palette::FAILED)
                .text_color(palette::TEXT_PRIMARY)
                .child("Reject"),
        )
        .into_any_element()
}
