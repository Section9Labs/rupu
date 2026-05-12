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

        // If a directory was passed on the command line, open it
        // immediately. Otherwise wait for the user to pick via File menu.
        if let Some(arg) = std::env::args().nth(1) {
            let dir = std::path::PathBuf::from(arg);
            match Workspace::open(&dir) {
                Ok(workspace) => {
                    tracing::info!(id = %workspace.manifest.id, "opened workspace from CLI arg");
                    let app_executor = executor::build_executor(&workspace);

                    // Spawn the badge updater. The watch receiver is driven on
                    // the GPUI foreground (main) thread so that NSStatusItem
                    // mutation stays main-thread-safe. The tokio task is already
                    // running at this point; the receiver loop below wakes on
                    // each count change.
                    let mut badge_rx =
                        menu::menubar::spawn_badge_updater(app_executor.clone());

                    // Drive the badge receiver on the GPUI main thread.
                    // `cx.spawn` uses the foreground executor — safe to call
                    // `update_badge_title` (which uses MainThreadMarker::new_unchecked)
                    // from inside the async closure.
                    #[cfg(target_os = "macos")]
                    {
                        let status_item = _status_item.clone();
                        cx.spawn(async move |_cx| {
                            loop {
                                // Wait for the count to change. Returns Err only
                                // when the sender is dropped (app exit / task panic).
                                if badge_rx.changed().await.is_err() {
                                    break;
                                }
                                let count = *badge_rx.borrow_and_update();
                                menu::menubar::update_badge_title(&status_item, count);
                            }
                        })
                        .detach();
                    }

                    WorkspaceWindow::open(workspace, app_executor, cx);
                }
                Err(e) => {
                    tracing::error!(?dir, %e, "failed to open workspace from CLI arg");
                }
            }
        }
    });
}
