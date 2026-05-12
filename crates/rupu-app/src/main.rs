//! rupu.app — native macOS desktop app.

use gpui::App;
use rupu_app::{window::WorkspaceWindow, workspace::Workspace};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();

    // For D-1 development: open whichever directory the user passes
    // as the first CLI arg, or fall back to cwd. The proper "File >
    // Open Workspace…" picker lands in Task 15.
    let project_dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let workspace = Workspace::open(&project_dir).expect("open workspace");
    tracing::info!(id = %workspace.manifest.id, "opened workspace");

    gpui_platform::application().run(move |cx: &mut App| {
        cx.activate(true);
        WorkspaceWindow::open(workspace, cx);
    });
}
