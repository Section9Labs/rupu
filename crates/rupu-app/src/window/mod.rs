//! WorkspaceWindow — the GPUI view for one workspace's window.

pub mod sidebar;
pub mod titlebar;

use crate::palette;
use crate::workspace::Workspace;
use gpui::{
    div, prelude::*, px, size, App, AnyElement, Bounds, Context, IntoElement, Render, Window,
    WindowBounds, WindowHandle, WindowOptions,
};

pub struct WorkspaceWindow {
    pub workspace: Workspace,
}

impl WorkspaceWindow {
    /// Open a new top-level window for the given workspace. The
    /// window owns the workspace handle for its lifetime.
    pub fn open(workspace: Workspace, cx: &mut App) -> WindowHandle<Self> {
        let bounds = Bounds::centered(None, size(px(1240.0), px(800.0)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None, // we draw our own titlebar inside the view
            ..Default::default()
        };
        cx.open_window(opts, |_window, cx| {
            cx.new(|_cx| WorkspaceWindow { workspace })
        })
        .expect("open workspace window")
    }
}

impl Render for WorkspaceWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let main_area = match self.workspace.project_assets.workflows.first() {
            Some(asset) => render_main_for_workflow(asset),
            None => render_main_placeholder(),
        };

        div()
            .size_full()
            .bg(palette::BG_PRIMARY)
            .text_color(palette::TEXT_PRIMARY)
            .flex()
            .flex_col()
            .child(titlebar::render(&self.workspace))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(sidebar::render(&self.workspace))
                    .child(main_area),
            )
    }
}

fn render_main_placeholder() -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(palette::TEXT_DIMMEST)
        .child("Open a workflow from the sidebar.")
        .into_any_element()
}

fn render_main_for_workflow(asset: &crate::workspace::Asset) -> AnyElement {
    use rupu_orchestrator::Workflow;

    let body = match std::fs::read_to_string(&asset.path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(path = ?asset.path, %e, "read workflow");
            return render_main_error(format!(
                "failed to read {}: {e}",
                asset.path.display()
            ));
        }
    };
    let wf = match Workflow::parse(&body) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(path = ?asset.path, %e, "parse workflow");
            return render_main_error(format!(
                "failed to parse {}: {e}",
                asset.path.display()
            ));
        }
    };

    div()
        .flex_1()
        .child(crate::view::graph::render(&wf))
        .into_any_element()
}

fn render_main_error(msg: String) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(palette::FAILED)
        .child(msg)
        .into_any_element()
}
