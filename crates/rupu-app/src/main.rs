//! rupu.app — native macOS desktop app.

use gpui::App;
use rupu_app::{executor, menu, window::WorkspaceWindow, workspace::Workspace};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();

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

        // If a directory was passed on the command line, open it
        // immediately. Otherwise wait for the user to pick via File menu.
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
        }
    });
}
