//! `rupu` CLI entry point. Tiny `tokio::main` wrapper around
//! [`rupu_cli::run`] — keep this file the thinnest possible so the
//! testable harness in `lib.rs` carries the actual logic.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    rupu_cli::run(args).await
}
