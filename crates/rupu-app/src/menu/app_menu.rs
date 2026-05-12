//! macOS app menu — at the moment, just `File > New / Open`.
//! `Open Recent` lands in D-2 alongside the tab system.

use crate::window::WorkspaceWindow;
use crate::workspace::Workspace;
use gpui::{actions, App, Menu, MenuItem};

actions!(rupu_app, [NewWorkspace, OpenWorkspace, Quit]);

/// Register the menu and wire its action handlers. Call once on app boot.
pub fn install(cx: &mut App) {
    cx.set_menus(vec![Menu::new("rupu").items(vec![MenuItem::submenu(
        Menu::new("File").items(vec![
            MenuItem::action("New Workspace\u{2026}", NewWorkspace),
            MenuItem::action("Open Workspace\u{2026}", OpenWorkspace),
            MenuItem::separator(),
            MenuItem::action("Quit rupu", Quit),
        ]),
    )])]);

    cx.on_action(|_: &NewWorkspace, cx| {
        if let Some(dir) = pick_directory_modal("Choose a directory for the new workspace") {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::error!(?dir, %e, "create workspace dir");
                return;
            }
            open_workspace_window(&dir, cx);
        }
    });
    cx.on_action(|_: &OpenWorkspace, cx| {
        if let Some(dir) = pick_directory_modal("Open a workspace directory") {
            open_workspace_window(&dir, cx);
        }
    });
    cx.on_action(|_: &Quit, cx| cx.quit());
}

fn open_workspace_window(dir: &std::path::Path, cx: &mut App) {
    match Workspace::open(dir) {
        Ok(workspace) => {
            tracing::info!(id = %workspace.manifest.id, path = ?dir, "open workspace");
            WorkspaceWindow::open(workspace, cx);
        }
        Err(e) => {
            tracing::error!(?dir, %e, "failed to open workspace");
            // TODO(D-2): surface this as a toast/modal once tab system exists.
        }
    }
}

/// Show a native NSOpenPanel directory picker. Returns Some(path) on
/// user confirm, None on cancel.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn pick_directory_modal(prompt: &str) -> Option<std::path::PathBuf> {
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{MainThreadMarker, NSString};

    unsafe {
        // SAFETY: pick_directory_modal is only called from action handlers
        // dispatched by GPUI on the main thread.
        let mtm = MainThreadMarker::new_unchecked();

        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        panel.setAllowsMultipleSelection(false);
        panel.setCanCreateDirectories(true);
        let msg = NSString::from_str(prompt);
        panel.setMessage(Some(&msg));

        if panel.runModal() == NSModalResponseOK {
            let url = panel.URL()?;
            let path = url.path()?;
            Some(std::path::PathBuf::from(path.to_string()))
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn pick_directory_modal(_prompt: &str) -> Option<std::path::PathBuf> {
    // On non-macOS dev builds, fall back to env var so devs can
    // exercise the open flow without a native picker.
    std::env::var("RUPU_APP_OPEN_DIR")
        .ok()
        .map(std::path::PathBuf::from)
}
