//! macOS app menu. `rupu / File / View / Window / Help`. Edit menu
//! intentionally skipped at this rev — Zed wires Cut/Copy/Paste via its
//! own `editor::actions` crate we don't depend on; macOS default routing
//! handles clipboard ops for focused text fields anyway.
//! `Open Recent` is a TODO for when the tab system lands.

use crate::window::WorkspaceWindow;
use crate::workspace::Workspace;
use gpui::{actions, App, KeyBinding, Menu, MenuItem};

actions!(
    rupu_app,
    [
        NewWorkspace,
        OpenWorkspace,
        Quit,
        ApproveFocused,
        RejectFocused,
        LaunchSelected,
        ToggleSidebar,
        AboutRupu,
    ]
);

/// Register the menu and wire its action handlers. Call once on app boot.
pub fn install(cx: &mut App) {
    cx.set_menus(vec![
        Menu::new("rupu").items(vec![
            MenuItem::action("About rupu", AboutRupu),
            MenuItem::separator(),
            MenuItem::action("Quit rupu", Quit),
        ]),
        Menu::new("File").items(vec![
            MenuItem::action("New Workspace\u{2026}", NewWorkspace),
            MenuItem::action("Open Workspace\u{2026}", OpenWorkspace),
        ]),
        Menu::new("View").items(vec![MenuItem::action("Toggle Sidebar", ToggleSidebar)]),
        // AppKit auto-populates the Window menu with Minimize / Zoom /
        // Bring All to Front when the bundle has a standard titlebar
        // (restored in the traffic-light task). Leaving items empty is
        // intentional.
        Menu::new("Window").items(vec![]),
        Menu::new("Help").items(vec![]),
    ]);

    // Keyboard shortcuts. `a` / `r` only fire when a step is focused and
    // in the Awaiting state; the guard lives in WorkspaceWindow render's
    // on_action handlers. `cmd-r` launches the focused workflow; `cmd-\`
    // toggles every sidebar section (sidebar-hide proxy until a dedicated
    // `sidebar_hidden` flag lands).
    cx.bind_keys(vec![
        KeyBinding::new("a", ApproveFocused, None),
        KeyBinding::new("r", RejectFocused, None),
        KeyBinding::new("cmd-r", LaunchSelected, None),
        KeyBinding::new("cmd-\\", ToggleSidebar, None),
        // TextInput shortcuts — scoped to focused text inputs only.
        KeyBinding::new(
            "backspace",
            crate::widget::text_input::Backspace,
            Some("TextInput"),
        ),
        KeyBinding::new(
            "delete",
            crate::widget::text_input::Delete,
            Some("TextInput"),
        ),
        KeyBinding::new("left", crate::widget::text_input::Left, Some("TextInput")),
        KeyBinding::new(
            "right",
            crate::widget::text_input::Right,
            Some("TextInput"),
        ),
        KeyBinding::new(
            "shift-left",
            crate::widget::text_input::SelectLeft,
            Some("TextInput"),
        ),
        KeyBinding::new(
            "shift-right",
            crate::widget::text_input::SelectRight,
            Some("TextInput"),
        ),
        KeyBinding::new(
            "cmd-a",
            crate::widget::text_input::SelectAll,
            Some("TextInput"),
        ),
        KeyBinding::new("home", crate::widget::text_input::Home, Some("TextInput")),
        KeyBinding::new("end", crate::widget::text_input::End, Some("TextInput")),
        KeyBinding::new(
            "ctrl-cmd-space",
            crate::widget::text_input::ShowCharacterPalette,
            Some("TextInput"),
        ),
        KeyBinding::new(
            "cmd-v",
            crate::widget::text_input::Paste,
            Some("TextInput"),
        ),
        KeyBinding::new("cmd-c", crate::widget::text_input::Copy, Some("TextInput")),
        KeyBinding::new("cmd-x", crate::widget::text_input::Cut, Some("TextInput")),
    ]);

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
    cx.on_action(|_: &AboutRupu, _cx| {
        tracing::info!(
            version = env!("CARGO_PKG_VERSION"),
            "About rupu — native About panel deferred to D-10"
        );
    });
    // ToggleSidebar is wired per-window in WorkspaceWindow::open so the
    // handler can reach the focused workspace via WeakEntity. Registering
    // it globally here would require synthesizing the active entity at
    // dispatch time, which the GPUI API doesn't expose cleanly at this rev.
}

fn open_workspace_window(dir: &std::path::Path, cx: &mut App) {
    match Workspace::open(dir) {
        Ok(workspace) => {
            tracing::info!(id = %workspace.manifest.id, path = ?dir, "open workspace");
            let app_executor = crate::executor::build_executor(&workspace);
            WorkspaceWindow::open(workspace, app_executor, cx);
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
pub fn pick_directory_modal(prompt: &str) -> Option<std::path::PathBuf> {
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
pub fn pick_directory_modal(_prompt: &str) -> Option<std::path::PathBuf> {
    // On non-macOS dev builds, fall back to env var so devs can
    // exercise the open flow without a native picker.
    std::env::var("RUPU_APP_OPEN_DIR")
        .ok()
        .map(std::path::PathBuf::from)
}
