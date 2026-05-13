//! Sidebar — minimal accordion per spec §6.2.
//!
//! Fixed section order: workflows · runs · repos · agents · issues.
//! Collapse state persists in `Workspace.manifest.ui.sidebar_collapsed_sections`.
//! For D-1, item clicks are no-ops (tabs land in D-2).
//! D-4: workflow row clicks set `focused_workflow`; right-click opens the launcher.

use crate::palette;
use crate::workspace::{Asset, Workspace};
use gpui::{div, prelude::*, px, App, AnyElement, IntoElement, MouseButton, Window};
use rupu_orchestrator::runs::RunStatus;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const SIDEBAR_WIDTH: f32 = 180.0;
const SECTION_ORDER: &[&str] = &["workflows", "runs", "repos", "agents", "issues"];

/// Active run status keyed by workflow path. Built by the caller
/// (`WorkspaceWindow::render`) so the pure render function stays free of
/// async / executor dependencies.
pub type ActiveRunMap = HashMap<PathBuf, RunStatus>;

/// Callback type for sidebar workflow-row clicks. Receives the workflow path,
/// the current GPUI window handle, and the app context.
pub type WorkflowClickCb =
    Arc<dyn Fn(PathBuf, &mut Window, &mut App) + Send + Sync + 'static>;

pub fn render(
    workspace: &Workspace,
    active_runs: &ActiveRunMap,
    on_workflow_click: WorkflowClickCb,
    on_workflow_right_click: WorkflowClickCb,
) -> impl IntoElement {
    let collapsed = &workspace.manifest.ui.sidebar_collapsed_sections;
    let project = &workspace.project_assets;
    let global = &workspace.global_assets;

    let mut container = div()
        .w(px(SIDEBAR_WIDTH))
        .h_full()
        .bg(palette::BG_SIDEBAR)
        .border_r_1()
        .border_color(palette::BORDER)
        .px(px(14.0))
        .py(px(18.0))
        .flex()
        .flex_col();

    for (i, section) in SECTION_ORDER.iter().enumerate() {
        let is_collapsed = collapsed.iter().any(|s| s == *section);
        let items: Vec<&Asset> = match *section {
            "workflows" => project
                .workflows
                .iter()
                .chain(global.workflows.iter())
                .collect(),
            "agents" => project.agents.iter().chain(global.agents.iter()).collect(),
            _ => Vec::new(), // runs/repos/issues land in D-3/D-9
        };
        container = container.child(render_section(
            section,
            &items,
            is_collapsed,
            i == 0,
            active_runs,
            on_workflow_click.clone(),
            on_workflow_right_click.clone(),
        ));
    }

    container
}

fn render_section(
    name: &str,
    items: &[&Asset],
    is_collapsed: bool,
    is_first: bool,
    active_runs: &ActiveRunMap,
    on_workflow_click: WorkflowClickCb,
    on_workflow_right_click: WorkflowClickCb,
) -> impl IntoElement {
    // Header: uppercase label + optional caret + count when collapsed.
    let caret_child: AnyElement = if is_collapsed {
        div()
            .text_color(palette::TEXT_DIMMEST)
            .child("▸")
            .into_any_element()
    } else {
        div().into_any_element()
    };

    let count_child: AnyElement = if is_collapsed {
        div()
            .ml_auto()
            .text_color(palette::TEXT_DIMMEST)
            .child(format!("{}", items.len()))
            .into_any_element()
    } else {
        div().into_any_element()
    };

    let header = div()
        .text_xs()
        .text_color(palette::TEXT_DIMMEST)
        .mb(px(4.0))
        .when(!is_first, |d| d.mt(px(18.0)))
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(div().child(name.to_uppercase()))
        .child(caret_child)
        .child(count_child);

    // Body: nothing when collapsed, em-dash placeholder when empty, items otherwise.
    let body: AnyElement = if is_collapsed {
        div().into_any_element()
    } else if items.is_empty() {
        div()
            .mt(px(2.0))
            .text_xs()
            .text_color(palette::TEXT_DIMMEST)
            .child("—")
            .into_any_element()
    } else {
        let is_workflows = name == "workflows";
        let mut list = div().flex().flex_col().gap(px(2.0));
        for asset in items {
            let dot_color = active_runs
                .get(&asset.path)
                .and_then(|status| match status {
                    RunStatus::Running => Some(palette::RUNNING),
                    RunStatus::AwaitingApproval => Some(palette::AWAITING),
                    RunStatus::Failed => Some(palette::FAILED),
                    _ => None,
                });

            let path = asset.path.clone();
            let mut row = div()
                .id(gpui::SharedString::from(format!(
                    "wf-{}",
                    asset.path.display()
                )))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .text_xs()
                .text_color(palette::TEXT_MUTED)
                .child(div().flex_1().child(asset.name.clone()));

            if let Some(color) = dot_color {
                row = row.child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(color));
            }

            if is_workflows {
                let cb_click = on_workflow_click.clone();
                let cb_right = on_workflow_right_click.clone();
                let path_right = path.clone();
                row = row
                    .cursor_pointer()
                    .on_click({
                        let path = path.clone();
                        move |_, w, cx| cb_click(path.clone(), w, cx)
                    })
                    .on_mouse_down(MouseButton::Right, {
                        move |_, w, cx| cb_right(path_right.clone(), w, cx)
                    });
            }

            list = list.child(row);
        }
        list.into_any_element()
    };

    div().child(header).child(body)
}
