//! `rupu` CLI entry point. Tiny `tokio::main` wrapper around
//! [`rupu_cli::run`] — keep this file the thinnest possible so the
//! testable harness in `lib.rs` carries the actual logic.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    // The dependency tree enables both rustls crypto providers (aws-lc-rs via
    // object_store/reqwest, ring via octocrab/jsonwebtoken), so rustls 0.23
    // cannot auto-select one and panics on first TLS use (e.g. `rupu cp serve`
    // building the SCM registry). Install aws-lc-rs as the process-level
    // default once at startup. `install_default` errors only if a provider is
    // already installed, which is harmless here.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let args = std::env::args().collect::<Vec<_>>();
    rupu_cli::run(args).await
}
