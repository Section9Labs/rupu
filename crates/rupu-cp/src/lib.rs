//! rupu-cp — control-plane HTTP server for the rupu web UI.
//!
//! `serve` is the main entrypoint; wire it from `rupu cp serve`.

pub mod agent_launcher;
pub mod api;
pub mod definition_generator;
pub mod embed;
pub mod error;
pub mod host;
pub mod launcher;
pub mod pagination;
pub mod repos;
pub mod server;
pub mod session_sender;
pub mod session_starter;
pub mod sse;
pub mod state;
pub mod transcript_tail;
pub mod usage;

use anyhow::Context as _;
use rupu_config::PricingConfig;
use std::io::IsTerminal as _;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use tracing::info;

pub struct ServeOpts {
    pub bind: SocketAddr,
    /// If set, require `Authorization: Bearer <token>` on `/api/*` routes.
    pub token: Option<String>,
    pub global_dir: PathBuf,
    /// Open the served URL in the default browser on startup (best-effort, and
    /// only when stdout is a terminal). The URL is always printed regardless.
    pub open_browser: bool,
    /// Optional run-launcher adapter. rupu-cli's `cp serve` provides the
    /// subprocess-spawning impl; `None` disables launching from the web UI.
    pub launcher: Option<std::sync::Arc<dyn crate::launcher::RunLauncher>>,
    /// Optional session-sender adapter. rupu-cli's `cp serve` provides the
    /// subprocess-spawning impl; `None` disables sending to sessions from the
    /// web UI.
    pub session_sender: Option<std::sync::Arc<dyn crate::session_sender::SessionSender>>,
    /// Optional repo-lister adapter. rupu-cli's `cp serve` provides the
    /// registry-backed impl; `None` → `/api/repos` returns 501.
    pub repos: Option<std::sync::Arc<dyn crate::repos::RepoLister>>,
    /// Optional agent-launcher adapter. rupu-cli's `cp serve` provides the
    /// subprocess-spawning impl; `None` disables agent launching from the web UI.
    pub agent_launcher: Option<std::sync::Arc<dyn crate::agent_launcher::AgentLauncher>>,
    /// Optional session-starter adapter. rupu-cli's `cp serve` provides the
    /// subprocess-spawning impl; `None` disables session starting from the web UI.
    pub session_starter: Option<std::sync::Arc<dyn crate::session_starter::SessionStarter>>,
    /// Optional definition-generator adapter. rupu-cli's `cp serve` provides the
    /// orchestrator-backed impl; `None` → the generate endpoints return 501.
    pub generator: Option<std::sync::Arc<dyn crate::definition_generator::DefinitionGenerator>>,
}

/// The browser-clickable URL for a bound address. An unspecified bind host
/// (`0.0.0.0` / `::`) is rewritten to loopback so the printed link works.
fn click_url(addr: SocketAddr) -> String {
    let host = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
        IpAddr::V4(ip) => ip.to_string(),
    };
    format!("http://{host}:{}", addr.port())
}

/// Best-effort browser launch (macOS `open`, other Unix `xdg-open`). Never
/// fails the server — a missing opener or headless session is silently skipped.
fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let opener: Option<&str> = Some("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener: Option<&str> = Some("xdg-open");
    #[cfg(not(unix))]
    let opener: Option<&str> = None;
    if let Some(opener) = opener {
        let _ = std::process::Command::new(opener).arg(url).spawn();
    }
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
    let open_browser = opts.open_browser;
    let pricing = load_pricing(&opts.global_dir);

    // Build the AppState with the default (read-only) registry first so that
    // `run_store` and `global_dir` are available for the fully-wired registry
    // we build next.
    let app_state = state::AppState::new(opts.global_dir.clone(), pricing.clone())
        .with_launcher(opts.launcher.clone())
        .with_session_sender(opts.session_sender.clone())
        .with_repos(opts.repos)
        .with_agent_launcher(opts.agent_launcher.clone())
        .with_session_starter(opts.session_starter.clone())
        .with_generator(opts.generator);

    // Replace the default read-only registry with a fully-wired one that
    // holds the real launcher / sender / starter adapters.
    let local = crate::host::local::LocalHostConnector::new(
        opts.launcher,
        opts.agent_launcher,
        opts.session_starter,
        opts.session_sender,
        std::sync::Arc::clone(&app_state.run_store),
        app_state.global_dir.clone(),
    )
    .with_pricing(pricing);
    let store = rupu_workspace::HostStore {
        root: app_state.global_dir.join("hosts"),
    };
    let registry = crate::host::registry::HostRegistry::new(store, std::sync::Arc::new(local));
    let app_state = app_state.with_hosts(std::sync::Arc::new(registry));

    let app = server::router(app_state, opts.token);

    let listener = tokio::net::TcpListener::bind(opts.bind)
        .await
        .with_context(|| format!("failed to bind to {}", opts.bind))?;

    let addr = listener.local_addr()?;
    let url = click_url(addr);
    // Always surface the URL prominently — independent of RUST_LOG / tracing.
    println!("\n  ➜  rupu Control Plane  →  {url}\n");
    info!("rupu cp serving on {url}");

    // Auto-open only when interactive (a real terminal), so headless / scripted
    // / supervised runs don't spawn a surprise browser. `--no-open` forces off.
    if open_browser && std::io::stdout().is_terminal() {
        open_in_browser(&url);
    }

    axum::serve(listener, app)
        .await
        .context("control-plane server error")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_url_rewrites_unspecified_to_loopback() {
        let p = |s: &str| click_url(s.parse::<SocketAddr>().unwrap());
        assert_eq!(p("0.0.0.0:7878"), "http://127.0.0.1:7878");
        assert_eq!(p("127.0.0.1:7878"), "http://127.0.0.1:7878");
        assert_eq!(p("192.168.1.5:9000"), "http://192.168.1.5:9000");
        assert_eq!(p("[::]:7878"), "http://[::1]:7878");
    }

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
        let p = rupu_config::pricing::lookup(&pricing, "anthropic", "claude-sonnet-4-6", "any")
            .unwrap();
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
        let p = rupu_config::pricing::lookup(&pricing, "anthropic", "claude-sonnet-4-6", "any")
            .unwrap();
        assert_eq!(p.input_per_mtok, 3.0); // builtin
        let _ = std::fs::remove_dir_all(&dir);
    }
}
