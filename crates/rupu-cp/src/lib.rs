//! rupu-cp — control-plane HTTP server for the rupu web UI.
//!
//! `serve` is the main entrypoint; wire it from `rupu cp serve`.

pub mod error;
pub mod server;
pub mod state;

use anyhow::Context as _;
use rupu_config::PricingConfig;
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::info;

pub struct ServeOpts {
    pub bind: SocketAddr,
    /// If set, require `Authorization: Bearer <token>` on `/api/*` routes.
    pub token: Option<String>,
    pub global_dir: PathBuf,
}

pub async fn serve(opts: ServeOpts) -> anyhow::Result<()> {
    let app_state = state::AppState::new(opts.global_dir, PricingConfig::default());
    let app = server::router(app_state);

    // TODO(T2+): when token is set, add a tower middleware layer that
    // validates `Authorization: Bearer <token>` for all `/api/*` routes.
    if opts.token.is_some() {
        tracing::warn!("--token set but bearer auth middleware not yet implemented (T2+)");
    }

    let listener = tokio::net::TcpListener::bind(opts.bind)
        .await
        .with_context(|| format!("failed to bind to {}", opts.bind))?;

    let addr = listener.local_addr()?;
    info!("rupu cp serving on http://{addr}");

    axum::serve(listener, app)
        .await
        .context("control-plane server error")?;

    Ok(())
}
