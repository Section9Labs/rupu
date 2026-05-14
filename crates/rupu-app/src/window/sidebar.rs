//! Sidebar — minimal accordion per spec §6.2.
//!
//! Fixed section order: workflows · runs · repos · agents · issues.
//! Collapse state persists in `Workspace.manifest.ui.sidebar_collapsed_sections`.
//! For D-1, item clicks are no-ops (tabs land in D-2).
//! D-4: workflow row clicks set `focused_workflow`; right-click opens the launcher.

use crate::palette;
use crate::workspace::{Asset, Workspace};
use gpui::{div, prelude::*, px, AnyElement, App, IntoElement, MouseButton, Window};
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
pub type WorkflowClickCb = Arc<dyn Fn(PathBuf, &mut Window, &mut App) + Send + Sync + 'static>;

/// Callback type for sidebar section-header clicks. Receives the section
/// name (`"workflows"` / `"runs"` / `"repos"` / `"agents"` / `"issues"`),
/// the current GPUI window handle, and the app context.
pub type SectionToggleCb = Arc<dyn Fn(&'static str, &mut Window, &mut App) + Send + Sync + 'static>;

/// Callback type for sidebar agent-row clicks. Receives the agent file
/// path, the current GPUI window handle, and the app context.
pub type AgentClickCb = Arc<dyn Fn(PathBuf, &mut Window, &mut App) + Send + Sync + 'static>;

#[allow(clippy::too_many_arguments)]
pub fn render(
    workspace: &Workspace,
    active_runs: &ActiveRunMap,
    focused_workflow: Option<&PathBuf>,
    on_workflow_click: WorkflowClickCb,
    on_workflow_right_click: WorkflowClickCb,
    on_agent_click: AgentClickCb,
    on_agent_right_click: AgentClickCb,
    on_section_toggle: SectionToggleCb,
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
            focused_workflow,
            on_workflow_click.clone(),
            on_workflow_right_click.clone(),
            on_agent_click.clone(),
            on_agent_right_click.clone(),
            on_section_toggle.clone(),
        ));
    }

    container
}

#[allow(clippy::too_many_arguments)]
fn render_section(
    name: &'static str,
    items: &[&Asset],
    is_collapsed: bool,
    is_first: bool,
    active_runs: &ActiveRunMap,
    focused_workflow: Option<&PathBuf>,
    on_workflow_click: WorkflowClickCb,
    on_workflow_right_click: WorkflowClickCb,
    on_agent_click: AgentClickCb,
    on_agent_right_click: AgentClickCb,
    on_section_toggle: SectionToggleCb,
) -> impl IntoElement {
    // Header: clickable row with caret + uppercase label + count badge.
    let caret = if is_collapsed { "▸" } else { "▾" };
    let header = div()
        .id(gpui::SharedString::from(format!("sec-{name}")))
        .text_xs()
        .text_color(palette::TEXT_DIMMEST)
        .mb(px(4.0))
        .when(!is_first, |d| d.mt(px(18.0)))
        .flex()
        .items_center()
        .gap(px(6.0))
        .cursor_pointer()
        .child(div().child(caret))
        .child(div().child(name.to_uppercase()))
        .child(
            // Count badge always shown
            div()
                .ml_auto()
                .text_color(palette::TEXT_DIMMEST)
                .child(format!("{}", items.len())),
        )
        .on_click({
            let cb = on_section_toggle.clone();
            move |_, w, cx| cb(name, w, cx)
        });

    // Body: nothing when collapsed, em-dash placeholder when empty, items otherwise.
    let body: AnyElement = if is_collapsed {
        div().into_any_element()
    } else if items.is_empty() {
        let hint = match name {
            "workflows" => Some("File → New Workspace to add"),
            "agents" => Some("Drop a `.md` agent into `~/.rupu/agents/`"),
            // Runs / repos / issues come alive in D-3 / D-9; leave a dash
            // until then.
            _ => None,
        };
        match hint {
            Some(h) => div()
                .mt(px(2.0))
                .text_xs()
                .text_color(palette::TEXT_DIMMEST)
                .italic()
                .child(h)
                .into_any_element(),
            None => div()
                .mt(px(2.0))
                .text_xs()
                .text_color(palette::TEXT_DIMMEST)
                .child("—")
                .into_any_element(),
        }
    } else {
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
            let is_selected = name == "workflows"
                && focused_workflow.map(|p| p == &asset.path).unwrap_or(false);
            let row_bg = if is_selected {
                palette::BG_ROW_SELECTED
            } else {
                palette::BG_SIDEBAR
            };
            let mut row = div()
                .id(gpui::SharedString::from(format!(
                    "wf-{}",
                    asset.path.display()
                )))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .px(px(4.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .bg(row_bg)
                .hover(|s| s.bg(palette::BG_ROW_HOVER))
                .text_xs()
                .text_color(palette::TEXT_MUTED)
                .child(div().flex_1().child(asset.name.clone()));

            if let Some(color) = dot_color {
                row = row.child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(color));
            }

            match name {
                "workflows" => {
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
                "agents" => {
                    let cb_click = on_agent_click.clone();
                    let cb_right = on_agent_right_click.clone();
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
                _ => {} // runs / repos / issues — wired in D-3 / D-9
            }

            list = list.child(row);
        }
        list.into_any_element()
    };

    div().child(header).child(body)
}
