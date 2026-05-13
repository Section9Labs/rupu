//! rupu.app — native macOS desktop app.
//!
//! DIAGNOSTIC BUILD: opens an embedded hello-world window alongside
//! the workspace window. If hello-world renders text but the workspace
//! window doesn't, the issue is in our WorkspaceWindow render path,
//! not the app setup.

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, IntoElement, Render, SharedString,
    Window, WindowBounds, WindowOptions,
};
use rupu_app::{executor, menu, window::WorkspaceWindow, workspace::Workspace};

struct HelloWorldDiag;

impl Render for HelloWorldDiag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .justify_center()
            .items_center()
            .text_xl()
            .text_color(rgb(0xffffff))
            .child(SharedString::from("Hello, embedded!"))
    }
}

fn open_hello_world(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(400.0), px(300.0)), cx);
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        },
        |_, cx| cx.new(|_| HelloWorldDiag),
    );
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();

    let rt: &'static tokio::runtime::Runtime = Box::leak(Box::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime"),
    ));
    let _rt_guard = rt.enter();

    gpui_platform::application().run(|cx: &mut App| {
        cx.activate(true);

        // DIAGNOSTIC: open an embedded hello-world window FIRST, before
        // any of our app setup. If this renders text, GPUI works inside
        // our process. If not, something in our app's startup is
        // breaking text rendering.
        open_hello_world(cx);

        menu::app_menu::install(cx);

        #[cfg(target_os = "macos")]
        let _status_item = menu::menubar::install();

        std::thread::spawn(rupu_app::workspace::storage::gc_clones_dir);

        if let Some(arg) = std::env::args().nth(1) {
            let dir = std::path::PathBuf::from(arg);
            match Workspace::open(&dir) {
                Ok(workspace) => {
                    tracing::info!(id = %workspace.manifest.id, "opened workspace from CLI arg");
                    let app_executor = executor::build_executor(&workspace);
                    WorkspaceWindow::open(workspace, app_executor, cx);
                }
                Err(e) => {
                    tracing::error!(?dir, %e, "failed to open workspace from CLI arg");
                }
            }
        } else {
            match rupu_app::workspace::recents::list() {
                Ok(manifests) if !manifests.is_empty() => {
                    for manifest in manifests {
                        let dir = std::path::PathBuf::from(&manifest.path);
                        match Workspace::open(&dir) {
                            Ok(workspace) => {
                                tracing::info!(
                                    id = %workspace.manifest.id,
                                    path = ?dir,
                                    "restored workspace from recents"
                                );
                                let app_executor = executor::build_executor(&workspace);
                                WorkspaceWindow::open(workspace, app_executor, cx);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    path = ?dir,
                                    %e,
                                    "skip recent workspace that could not be opened"
                                );
                            }
                        }
                    }
                }
                Ok(_) => {
                    tracing::info!("no workspaces found; use File > Open Workspace\u{2026}");
                }
                Err(e) => {
                    tracing::warn!(%e, "could not read recents list; starting with no windows");
                }
            }
        }
    });
}
