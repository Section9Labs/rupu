//! rupu-cp — control-plane HTTP server for the rupu web UI.
//!
//! `serve` is the main entrypoint; wire it from `rupu cp serve`.

pub mod api;
pub mod embed;
pub mod error;
pub mod server;
pub mod sse;
pub mod state;
pub mod transcript_tail;

use anyhow::Context as _;
use rupu_config::PricingConfig;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct ServeOpts {
    pub bind: SocketAddr,
    /// If set, require `Authorization: Bearer <token>` on `/api/*` routes.
    pub token: Option<String>,
    pub global_dir: PathBuf,
}

/// Load the user's `[pricing]` overrides from `<global_dir>/config.toml`.
///
/// Returns an empty `PricingConfig` when the file is absent, and falls back
/// to `default()` (with a warning) when it exists but cannot be read/parsed.
/// `rupu_config::pricing::lookup` falls back to the builtin price table, so
/// cost still resolves for common models even when this is empty.
fn load_pricing(global_dir: &Path) -> PricingConfig {
    let config_path = global_dir.join("config.toml");
    if !config_path.exists() {
        return PricingConfig::default();
    }
    match rupu_config::layer_files(Some(&config_path), None) {
        Ok(cfg) => cfg.pricing,
        Err(e) => {
            tracing::warn!(path = %config_path.display(), error = %e, "failed to load [pricing]; using builtin prices only");
            PricingConfig::default()
        }
    }
}

pub async fn serve(opts: ServeOpts) -> anyhow::Result<()> {
    let pricing = load_pricing(&opts.global_dir);
    let app_state = state::AppState::new(opts.global_dir, pricing);
    let app = server::router(app_state, opts.token);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_pricing_empty_when_no_config_file() {
        let dir = std::env::temp_dir().join(format!("rupu-cp-pricing-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pricing = load_pricing(&dir);
        assert!(pricing.models.is_empty());
        assert!(pricing.agents.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_pricing_reads_user_overrides() {
        let dir = std::env::temp_dir().join(format!("rupu-cp-pricing-some-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[pricing.anthropic.\"claude-sonnet-4-6\"]\ninput_per_mtok = 99.0\noutput_per_mtok = 99.0\n",
        )
        .unwrap();
        let pricing = load_pricing(&dir);
        let p = rupu_config::pricing::lookup(&pricing, "anthropic", "claude-sonnet-4-6", "any").unwrap();
        assert_eq!(p.input_per_mtok, 99.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_pricing_falls_back_on_malformed_config() {
        let dir = std::env::temp_dir().join(format!("rupu-cp-pricing-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "this is not = valid = toml = [[[").unwrap();
        let pricing = load_pricing(&dir);
        assert!(pricing.models.is_empty());
        let p = rupu_config::pricing::lookup(&pricing, "anthropic", "claude-sonnet-4-6", "any").unwrap();
        assert_eq!(p.input_per_mtok, 3.0); // builtin
        let _ = std::fs::remove_dir_all(&dir);
    }
}
