//! Logging init. Uses `tracing-subscriber` with env-filter so users
//! can `RUPU_LOG=debug rupu run ...` to see internals.

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize logging. Idempotent — safe to call multiple times in
/// the same process (tests rely on this).
pub fn init() {
    let filter = EnvFilter::try_from_env("RUPU_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).with_writer(std::io::stderr))
        .try_init();
}
