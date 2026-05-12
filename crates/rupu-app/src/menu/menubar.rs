//! macOS menubar status item — the "cross-workspace runs badge".
//!
//! Spec §6.1 / §8.7: an always-on menubar item whose icon shows the
//! total in-flight run count across all open workspaces. For D-1
//! this is hard-wired to 0; D-3 lights up the count via a callback
//! the executor registers, and D-4 fills in the dropdown.

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::Arc;

    use tokio::sync::watch;

    use crate::executor::AppExecutor;

    #[allow(unsafe_code)]
    pub fn install() -> objc2::rc::Retained<objc2_app_kit::NSStatusItem> {
        use objc2_app_kit::{NSStatusBar, NSVariableStatusItemLength};
        use objc2_foundation::{MainThreadMarker, NSString};

        // SAFETY: NSStatusBar::systemStatusBar + statusItemWithLength: are
        // Apple-documented entry points. The returned NSStatusItem is
        // retained by the status bar AND by us until we drop it.
        // SAFETY: install() is called from the GPUI app closure which runs on
        // the main thread. NSThread feature is not enabled so we use
        // new_unchecked(); the main-thread requirement is upheld by the
        // call-site contract documented on this function.
        unsafe {
            let mtm = MainThreadMarker::new_unchecked();

            let bar = NSStatusBar::systemStatusBar();
            let item = bar.statusItemWithLength(NSVariableStatusItemLength);

            // Title — for D-1 just the rupu glyph + the count.
            let title = NSString::from_str("\u{25D0} 0");
            if let Some(button) = item.button(mtm) {
                button.setTitle(&title);
            }

            item
        }
    }

    /// Update the NSStatusItem title to reflect the current pending-approval
    /// count. Shows `"◐ 0"` when idle and `"◐ N"` when N approvals are
    /// waiting. Called from the foreground (main) thread only.
    #[allow(unsafe_code)]
    pub fn update_badge_title(
        item: &objc2::rc::Retained<objc2_app_kit::NSStatusItem>,
        count: usize,
    ) {
        use objc2_foundation::{MainThreadMarker, NSString};

        let label = format!("\u{25D0} {count}");
        // SAFETY: update_badge_title is only called from the GPUI foreground
        // executor (main thread). See App::spawn / foreground_executor note in
        // main.rs. Same pattern as install() above.
        unsafe {
            let mtm = MainThreadMarker::new_unchecked();
            let title = NSString::from_str(&label);
            if let Some(button) = item.button(mtm) {
                button.setTitle(&title);
            }
        }
    }

    /// Spawn a background tokio task that polls the given executor every 2 s
    /// and publishes the pending-approval count (runs in
    /// `RunStatus::AwaitingApproval`) to the returned watch receiver.
    ///
    /// The receiver is meant to be consumed on the main thread via
    /// `cx.spawn` so that `update_badge_title` can be called safely.
    pub fn spawn_badge_updater(app_executor: Arc<AppExecutor>) -> watch::Receiver<usize> {
        let (tx, rx) = watch::channel(0usize);
        tokio::spawn(async move {
            loop {
                let count = app_executor
                    .list_active_runs(None)
                    .into_iter()
                    .filter(|r| {
                        matches!(
                            r.status,
                            rupu_orchestrator::runs::RunStatus::AwaitingApproval
                        )
                    })
                    .count();
                // send() only errors when all receivers are dropped — app exit.
                if tx.send(count).is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
        rx
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::sync::Arc;

    use tokio::sync::watch;

    use crate::executor::AppExecutor;

    /// No-op on non-macOS — the menubar is a Mac-only surface.
    pub fn install() {}

    /// No-op badge updater on non-macOS. Returns a receiver that never changes.
    pub fn spawn_badge_updater(_app_executor: Arc<AppExecutor>) -> watch::Receiver<usize> {
        let (_tx, rx) = watch::channel(0usize);
        rx
    }
}

pub use imp::install;
pub use imp::spawn_badge_updater;

#[cfg(target_os = "macos")]
pub use imp::update_badge_title;
