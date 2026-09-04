//! rupu-cp — control-plane HTTP server for the rupu web UI.
//!
//! `serve` is the main entrypoint; wire it from `rupu cp serve`.

pub mod agent_launcher;
pub mod api;
pub mod config_write;
pub mod definition_generator;
pub mod embed;
pub mod error;
pub mod fleet_inventory;
pub mod host;
pub mod launcher;
pub mod net;
pub mod node;
pub mod pagination;
pub mod repos;
pub mod server;
pub mod session_mutator;
pub mod session_sender;
pub mod session_starter;
pub mod sse;
pub mod state;
pub mod transcript_mutator;
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
    /// Optional session-mutator adapter. rupu-cli's `cp serve` provides the
    /// subprocess impl; `None` → the session archive/restore/delete endpoints
    /// return 501.
    pub session_mutator: Option<std::sync::Arc<dyn crate::session_mutator::SessionMutator>>,
    /// Optional transcript-mutator adapter (standalone agent-run
    /// transcripts). rupu-cli's `cp serve` provides the subprocess impl;
    /// `None` → the transcript archive/delete endpoints return 501.
    pub transcript_mutator:
        Option<std::sync::Arc<dyn crate::transcript_mutator::TranscriptMutator>>,
    /// Optional fleet-inventory adapter (provider probes + SCM inventory).
    /// rupu-cli's `cp serve` provides it; `None` → the dashboard's fleet strip
    /// reports nothing for the provider- and SCM-backed counts rather than
    /// fabricating zeros.
    pub inventory: Option<std::sync::Arc<dyn crate::fleet_inventory::FleetInventory>>,
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

/// Response header `/healthz` sets to the serving process's PID, so a
/// second `rupu cp serve` that loses the bind race can name the daemon
/// that still owns the port (see [`bind`]).
pub const HEALTHZ_PID_HEADER: &str = "x-rupu-pid";
/// Response header `/healthz` sets to the serving crate version.
pub const HEALTHZ_VERSION_HEADER: &str = "x-rupu-version";

/// A live rupu control plane found answering on an address we failed to
/// bind. Both fields come from the [`HEALTHZ_PID_HEADER`] /
/// [`HEALTHZ_VERSION_HEADER`] response headers; a pre-header daemon still
/// answers `ok` and is reported without them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingServer {
    pub pid: Option<u32>,
    pub version: Option<String>,
}

impl std::fmt::Display for ExistingServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.pid, &self.version) {
            (Some(pid), Some(v)) => write!(f, "pid {pid}, rupu {v}"),
            (Some(pid), None) => write!(f, "pid {pid}"),
            (None, Some(v)) => write!(f, "rupu {v}, pid unknown"),
            (None, None) => write!(f, "pid unknown"),
        }
    }
}

/// How long [`bind`] waits for an occupant of a busy port to answer
/// `/healthz` before concluding it isn't a rupu control plane.
const EXISTING_SERVER_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Probe `addr` for a live rupu control plane: `GET /healthz` must answer
/// `200 ok`. Anything else (connection refused, a different service, a
/// timeout) is `None`. Loopback-rewritten like [`click_url`], so an
/// unspecified bind (`0.0.0.0`) is probed on `127.0.0.1`.
pub async fn probe_existing_server(addr: SocketAddr) -> Option<ExistingServer> {
    let url = format!("{}/healthz", click_url(addr));
    // The daemon's own probe of a sibling daemon is not run-scoped traffic:
    // unrecorded, same as every other `cp serve` self-egress (`NullSink`).
    let client = rupu_netflow::http::client_with(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System),
        reqwest::Client::builder().timeout(EXISTING_SERVER_PROBE_TIMEOUT),
        std::sync::Arc::new(rupu_netflow::NullSink),
    )
    .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let pid = header(HEALTHZ_PID_HEADER).and_then(|s| s.parse().ok());
    let version = header(HEALTHZ_VERSION_HEADER);
    let body = resp.text().await.ok()?;
    (body.trim() == "ok").then_some(ExistingServer { pid, version })
}

/// Bind the control-plane listener, or explain exactly why not.
///
/// Split out of [`serve`] so `rupu cp serve` can take the address FIRST —
/// before it spawns any of its background loops — and fail fast, non-zero,
/// when the port is taken. Binding only after those loops were already
/// running meant a bind failure had to wait for every in-flight tick (an
/// SCM inventory refresh mid-GitHub-call, say) to drain before the error
/// was even printed: the process looked alive for minutes, held outbound
/// connections, and served nothing (observed 2026-09-04).
///
/// On `EADDRINUSE` the address is also probed for a live rupu control
/// plane ([`probe_existing_server`]); when one answers, the error names
/// its PID so an operator whose "restart" forgot to stop the old daemon
/// learns which process still owns the port instead of guessing.
pub async fn bind(addr: SocketAddr) -> anyhow::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => Ok(listener),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if let Some(existing) = probe_existing_server(addr).await {
                anyhow::bail!(
                    "failed to bind to {addr}: a rupu control plane is already serving there \
                     ({existing}); stop it first or pass a different --bind"
                );
            }
            Err(anyhow::Error::new(e).context(format!("failed to bind to {addr}")))
        }
        Err(e) => Err(anyhow::Error::new(e).context(format!("failed to bind to {addr}"))),
    }
}

/// [`bind`] then [`serve_on`]. Callers that run work of their own between
/// the two (`rupu cp serve`'s background loops) should call them
/// separately so a bind failure never has to wait on that work.
pub async fn serve(opts: ServeOpts) -> anyhow::Result<()> {
    let listener = bind(opts.bind).await?;
    serve_on(listener, opts).await
}

/// Serve the control plane on an already-bound listener (see [`bind`]).
/// `opts.bind` is informational here (`GET /api/config`'s `status.bind`);
/// the listener's own address is what gets printed and served.
pub async fn serve_on(listener: tokio::net::TcpListener, opts: ServeOpts) -> anyhow::Result<()> {
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
        .with_generator(opts.generator)
        .with_session_mutator(opts.session_mutator.clone())
        .with_transcript_mutator(opts.transcript_mutator.clone())
        .with_bind(opts.bind.to_string())
        .with_token_set(opts.token.is_some());

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
    .with_pricing(pricing)
    .with_inventory(opts.inventory);
    let store = rupu_workspace::HostStore {
        root: app_state.global_dir.join("hosts"),
    };
    let registry = crate::host::registry::HostRegistry::new(store, std::sync::Arc::new(local))
        .with_tunnel_deps(
            std::sync::Arc::clone(&app_state.node_registry),
            std::sync::Arc::clone(&app_state.node_mirror),
            std::sync::Arc::clone(&app_state.run_store),
            app_state.pricing.clone(),
        );
    let app_state = app_state.with_hosts(std::sync::Arc::new(registry));

    let app = server::router(app_state, opts.token);

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

    /// Regression for the 2026-09-04 linger: a bind failure must surface as
    /// an error naming the address, immediately, not after unrelated work.
    #[tokio::test]
    async fn bind_fails_fast_with_address_when_port_is_taken_by_a_stranger() {
        // A plain socket that never speaks HTTP: the probe must not
        // mistake it for a rupu control plane.
        let occupant = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = occupant.local_addr().unwrap();
        let started = std::time::Instant::now();
        let err = bind(addr).await.expect_err("second bind must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&format!("failed to bind to {addr}")),
            "got: {msg}"
        );
        assert!(
            !msg.contains("already serving"),
            "stranger is not a rupu daemon: {msg}"
        );
        assert!(
            started.elapsed() < EXISTING_SERVER_PROBE_TIMEOUT * 2,
            "bind must not hang on a silent occupant"
        );
    }

    #[tokio::test]
    async fn bind_names_the_existing_control_plane_pid() {
        let dir = std::env::temp_dir().join(format!("rupu-cp-bind-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = state::AppState::new(dir.clone(), PricingConfig::default());
        let app = server::router(state, None);
        let occupant = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = occupant.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(occupant, app).await.unwrap();
        });

        let found = probe_existing_server(addr)
            .await
            .expect("healthz must answer");
        assert_eq!(found.pid, Some(std::process::id()));
        assert_eq!(found.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));

        let err = bind(addr).await.expect_err("second bind must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains(&addr.to_string()), "got: {msg}");
        assert!(msg.contains("already serving"), "got: {msg}");
        assert!(
            msg.contains(&format!("pid {}", std::process::id())),
            "got: {msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_server_display_degrades_without_headers() {
        let full = ExistingServer {
            pid: Some(42),
            version: Some("0.1.0".into()),
        };
        assert_eq!(full.to_string(), "pid 42, rupu 0.1.0");
        let bare = ExistingServer {
            pid: None,
            version: None,
        };
        assert_eq!(bare.to_string(), "pid unknown");
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
