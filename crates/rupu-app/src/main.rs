//! rupu.app — native macOS desktop app.

use gpui::App;
use rupu_app::{executor, menu, window::WorkspaceWindow, workspace::Workspace};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();

    // Install a multi-thread Tokio runtime kept alive for the app's
    // lifetime. GPUI's event loop is not tokio-aware; without an
    // ambient runtime, InProcessExecutor::start panics when it
    // tokio::spawn's the workflow task.
    let rt: &'static tokio::runtime::Runtime = Box::leak(Box::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime"),
    ));
    let _rt_guard = rt.enter();

    gpui_platform::application().run(|cx: &mut App| {
        cx.activate(true);

        menu::app_menu::install(cx);

        // Install the menubar status item. Keep the retain alive for
        // the lifetime of the app loop — dropping it removes the
        // status item from the system menubar.
        #[cfg(target_os = "macos")]
        let _status_item = menu::menubar::install();

        // Best-effort cleanup of stale launcher-clone tempdirs. Spawned on a
        // regular OS thread to keep GC off the GPUI main thread and out of
        // the tokio runtime startup path.
        std::thread::spawn(rupu_app::workspace::storage::gc_clones_dir);

        // If a directory was passed on the command line, open it immediately.
        // Otherwise re-open the most-recently-used workspaces from the
        // recents list so the user lands in a familiar state.
        if let Some(arg) = std::env::args().nth(1) {
            let dir = std::path::PathBuf::from(arg);
            match Workspace::open(&dir) {
                Ok(workspace) => {
                    tracing::info!(id = %workspace.manifest.id, "opened workspace from CLI arg");
                    let app_executor = executor::build_executor(&workspace);

                    // D-3 simplification: badge updater deferred to D-4 when the app
                    // fully initializes its main event loop. For now, the menubar shows
                    // a static 0 count, which is acceptable for smoke test.

                    WorkspaceWindow::open(workspace, app_executor, cx);
                }
                Err(e) => {
                    tracing::error!(?dir, %e, "failed to open workspace from CLI arg");
                }
            }
        } else {
            // No CLI arg — restore the most recently opened workspaces.
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
