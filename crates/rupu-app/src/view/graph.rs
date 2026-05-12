//! Graph view — GPUI renderer for `Vec<GraphRow>` from
//! `rupu-app-canvas::render_rows`. Each row becomes a horizontal
//! flex of styled monospace text spans. Layout is pure tree-walk
//! (no col×row grid for D-2 — that lives in D-6's Canvas view).

use crate::palette;
use crate::view::ApproveCallback;
use gpui::{div, prelude::*, px, AnyElement, IntoElement, Rgba};
use rupu_app_canvas::{GraphCell, GraphRow, NodeStatus};
use rupu_orchestrator::Workflow;

/// Top-level entry point: render a parsed `Workflow` as the git-
/// graph view, colouring each node according to live status in
/// `model`. For a static (no live run) view, pass a default
/// `RunModel` whose `nodes` map is empty — all nodes render as
/// `Waiting`.
///
/// `on_approve` is invoked with the `step_id` when the user clicks
/// the approve button on an awaiting node.
pub fn render(
    workflow: &Workflow,
    model: &crate::run_model::RunModel,
    on_approve: ApproveCallback,
) -> impl IntoElement {
    let rows = rupu_app_canvas::render_rows(workflow, |id| {
        model.nodes.get(id).copied().unwrap_or(NodeStatus::Waiting)
    });

    let mut container = div()
        .size_full()
        .bg(palette::BG_PRIMARY)
        .px(px(24.0))
        .py(px(20.0))
        .flex()
        .flex_col()
        .gap(px(2.0));

    for row in &rows {
        container = container.child(render_row(row, on_approve.clone()));
    }

    container
}

fn render_row(row: &GraphRow, on_approve: ApproveCallback) -> AnyElement {
    let mut hbox = div()
        .flex()
        .flex_row()
        .items_center()
        .text_sm()
        // Monospace font so the glyphs line up vertically across rows.
        // "Menlo" is the macOS system monospace family.
        .font_family("Menlo");

    for cell in &row.cells {
        hbox = hbox.child(render_cell(cell));
    }

    // Append an inline approve pill button when this row is an Awaiting anchor.
    if let Some((step_id, NodeStatus::Awaiting)) = &row.anchor {
        let step_id = step_id.clone();
        hbox = hbox
            .child(div().w(px(12.0))) // spacer before pill
            .child(
                div()
                    .id(gpui::SharedString::from(format!("approve-inline-{step_id}")))
                    .px(px(8.0))
                    .py(px(2.0))
                    .bg(palette::COMPLETE)
                    .text_color(palette::TEXT_PRIMARY)
                    .text_sm()
                    .font_family("Menlo")
                    .child("✓ Approve")
                    .cursor_pointer()
                    .on_click(move |_event, window, cx| {
                        on_approve(step_id.clone(), window, cx);
                    }),
            );
    }

    hbox.into_any_element()
}

fn render_cell(cell: &GraphCell) -> AnyElement {
    match cell {
        GraphCell::Pipe(status) => div()
            .text_color(status_rgba(*status))
            .child("│")
            .into_any_element(),
        GraphCell::Branch(glyph, status) => div()
            .text_color(status_rgba(*status))
            .child(glyph.as_str().to_string())
            .into_any_element(),
        GraphCell::Bullet(status) => {
            // Use the status glyph (○, ●, ◐, ✓, ✗, ⏸, ⊘, ↻) rather than
            // a fixed `●` — that way Waiting renders as ○ (hollow) so an
            // unstarted workflow doesn't look like it's mid-run.
            div()
                .text_color(status_rgba(*status))
                .child(status.glyph().to_string())
                .into_any_element()
        }
        GraphCell::Space(n) => div().child(" ".repeat(*n as usize)).into_any_element(),
        GraphCell::Label(s) => div()
            .text_color(palette::TEXT_PRIMARY)
            .child(s.clone())
            .into_any_element(),
        GraphCell::Meta(s) => div()
            .text_color(palette::TEXT_DIMMEST)
            .child(s.clone())
            .into_any_element(),
    }
}

fn status_rgba(status: NodeStatus) -> Rgba {
    let (r, g, b) = status.rgb();
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}
