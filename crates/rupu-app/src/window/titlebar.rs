//! Titlebar: color chip · workspace name · in-flight count badge.
//!
//! Per spec §6.1, the count is this-workspace only (the system
//! menubar in `menu/menubar.rs` carries the cross-workspace
//! count).

use crate::palette;
use crate::window::sidebar::ActiveRunMap;
use crate::workspace::Workspace;
use gpui::{div, prelude::*, px, FontWeight, IntoElement};
use rupu_orchestrator::runs::RunStatus;

/// Count workflows whose active run is Running or AwaitingApproval.
pub fn in_flight_count(active_runs: &ActiveRunMap) -> u32 {
    active_runs
        .values()
        .filter(|s| matches!(s, RunStatus::Running | RunStatus::AwaitingApproval))
        .count() as u32
}

pub fn render(workspace: &Workspace, in_flight: u32) -> impl IntoElement {
    let chip_color = workspace.manifest.color.to_rgba();

    div()
        .h(px(36.0))
        .bg(palette::BG_TITLEBAR)
        .border_b_1()
        .border_color(palette::BORDER)
        // Leave room for native traffic lights on the left edge — Task 7
        // re-enables them at (9, 9).
        .pl(px(80.0))
        .pr(px(14.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(chip_color))
        .child(
            div()
                .text_color(palette::TEXT_PRIMARY)
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(workspace.manifest.name.clone()),
        )
        .child(if in_flight > 0 {
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
        })
}

#[cfg(test)]
mod tests {
    use super::in_flight_count;
    use rupu_orchestrator::runs::RunStatus;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn in_flight_count_counts_running_and_awaiting() {
        let mut m: HashMap<PathBuf, RunStatus> = HashMap::new();
        m.insert("a".into(), RunStatus::Running);
        m.insert("b".into(), RunStatus::AwaitingApproval);
        m.insert("c".into(), RunStatus::Completed);
        m.insert("d".into(), RunStatus::Failed);
        assert_eq!(in_flight_count(&m), 2);
    }

    #[test]
    fn in_flight_count_zero_when_empty() {
        let m: HashMap<PathBuf, RunStatus> = HashMap::new();
        assert_eq!(in_flight_count(&m), 0);
    }
}
