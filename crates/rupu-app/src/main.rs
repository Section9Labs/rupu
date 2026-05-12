//! rupu.app — native macOS desktop app.

use gpui::App;
use rupu_app::{menu, window::WorkspaceWindow, workspace::Workspace};

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

        // If a directory was passed on the command line, open it
        // immediately. Otherwise wait for the user to pick via File menu.
        if let Some(arg) = std::env::args().nth(1) {
            let dir = std::path::PathBuf::from(arg);
            match Workspace::open(&dir) {
                Ok(workspace) => {
                    tracing::info!(id = %workspace.manifest.id, "opened workspace from CLI arg");
                    WorkspaceWindow::open(workspace, cx);
                }
                Err(e) => {
                    tracing::error!(?dir, %e, "failed to open workspace from CLI arg");
                }
            }
        }
    });
}
