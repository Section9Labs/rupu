//! Logging init. Uses `tracing-subscriber` with env-filter so users
//! can `RUPU_LOG=debug rupu run ...` to see internals.
//!
//! `init` writes to stderr by default — fine for one-shot commands
//! whose entire output is line-stream text. For commands that take
//! over the terminal with an alt-screen TUI (`rupu run`, `rupu
//! workflow run`, `rupu watch`), the caller MUST use `init_to_file`
//! before entering raw mode. Otherwise tracing punches through the
//! alt-screen and corrupts the canvas (the v0.4.x TUI dump bug).

use std::path::PathBuf;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Stderr-writing init for non-TUI commands. Idempotent — safe to
/// call multiple times in the same process (tests rely on this).
pub fn init() {
    let filter = EnvFilter::try_from_env("RUPU_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).with_writer(std::io::stderr))
        .try_init();
}

/// File-writing init for TUI commands. The TUI owns the terminal,
/// so writing tracing lines anywhere on stdout/stderr corrupts the
/// canvas; route them to `~/.rupu/cache/rupu.log` instead. Caller
/// is responsible for telling the user where the log file lives if
/// they need to debug.
///
/// Returns the resolved log file path on success so the caller can
/// surface it (e.g. in a help overlay or a `RUPU_LOG_FILE` echo).
/// Falls back to `init()` (stderr) if the cache dir can't be
/// created — that's worse than ideal but better than silently
/// dropping logs.
pub fn init_to_file() -> Option<PathBuf> {
    let Some(cache_dir) = dirs::cache_dir().map(|d| d.join("rupu")) else {
        init();
        return None;
    };
    if std::fs::create_dir_all(&cache_dir).is_err() {
        init();
        return None;
    }
    let log_path = cache_dir.join("rupu.log");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    else {
        init();
        return None;
    };
    let filter = EnvFilter::try_from_env("RUPU_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(file),
        )
        .try_init();
    Some(log_path)
}
