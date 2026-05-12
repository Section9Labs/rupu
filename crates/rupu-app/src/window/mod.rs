//! WorkspaceWindow — the GPUI view for one workspace's window.

pub mod sidebar;
pub mod titlebar;

use crate::palette;
use crate::workspace::Workspace;
use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, IntoElement, Render, Window, WindowBounds,
    WindowHandle, WindowOptions,
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
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(palette::TEXT_DIMMEST)
                            .child("Open a workflow from the sidebar."),
                    ),
            )
    }
}
