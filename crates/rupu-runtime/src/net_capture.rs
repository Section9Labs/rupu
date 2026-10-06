//! Process-wide selection of the subprocess network-capture backend.
//!
//! [`shared`] hands every run in the process the same backend, memoized in
//! a `OnceLock`. The decision itself lives in [`choose`], a pure function of
//! the config and the env override, so tests can exercise every branch
//! deterministically without tripping the memoization or mutating the
//! process environment.

use rupu_config::NetflowConfig;
use rupu_netflow::{NoopCapture, SubprocessCapture};
use rupu_netwatch::unsupported::UnsupportedCapture;
use std::sync::{Arc, OnceLock};

/// Env override: `RUPU_NETFLOW_SUBPROCESS=0` forces capture off.
const ENV_VAR: &str = "RUPU_NETFLOW_SUBPROCESS";

static SHARED: OnceLock<Arc<dyn SubprocessCapture>> = OnceLock::new();

/// The process's one capture backend. The first call decides (config + env)
/// and every later call returns the same instance, whatever config it passes.
pub fn shared(cfg: &NetflowConfig) -> Arc<dyn SubprocessCapture> {
    SHARED
        .get_or_init(|| {
            let env_disabled = std::env::var(ENV_VAR).map(|v| v == "0").unwrap_or(false);
            choose(cfg, env_disabled)
        })
        .clone()
}

/// The backend decision, with the env override passed in. Not memoized; use
/// [`shared`] outside tests.
///
/// Enabled on Linux and macOS gets the real backend; other platforms get the
/// backend that announces capture as unavailable until their backend lands.
pub(crate) fn choose(cfg: &NetflowConfig, env_disabled: bool) -> Arc<dyn SubprocessCapture> {
    if !cfg.subprocess_capture || env_disabled {
        return Arc::new(NoopCapture);
    }
    enabled_backend(cfg)
}

/// Upper bound for a configured linger (one day). Backends compute
/// `now - linger`, which panics on overflow for a near-`Duration::MAX` value,
/// so anything absurd is clamped rather than passed through.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
const MAX_LINGER_MS: i64 = 24 * 60 * 60 * 1000;

/// A configured linger in milliseconds as a `chrono::Duration`, clamped so an
/// absurd config value can never panic: it is capped at [`MAX_LINGER_MS`],
/// and anything unrepresentable falls back to 3s.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
fn linger_from(ms: u64) -> chrono::Duration {
    chrono::Duration::try_milliseconds(i64::try_from(ms).unwrap_or(i64::MAX).min(MAX_LINGER_MS))
        .unwrap_or_else(|| chrono::Duration::milliseconds(3000))
}

/// Linux: start the real cgroup/conntrack backend; if it cannot start here
/// (no cgroup delegation, no conntrack...) announce why instead of going
/// silent.
#[cfg(target_os = "linux")]
fn enabled_backend(cfg: &NetflowConfig) -> Arc<dyn SubprocessCapture> {
    let linger = linger_from(cfg.subprocess_linger_ms);
    let poll = std::time::Duration::from_millis(cfg.subprocess_poll_ms);
    match rupu_netwatch::linux::LinuxCapture::start(linger, poll) {
        Ok(cap) => Arc::new(cap),
        Err(reason) => Arc::new(UnsupportedCapture::new(reason)),
    }
}

/// macOS: start the network-statistics backend; if it cannot start here
/// announce why instead of going silent.
#[cfg(target_os = "macos")]
fn enabled_backend(cfg: &NetflowConfig) -> Arc<dyn SubprocessCapture> {
    let linger = linger_from(cfg.subprocess_linger_ms);
    match rupu_netwatch::macos::MacosCapture::start(linger) {
        Ok(cap) => Arc::new(cap),
        Err(reason) => Arc::new(UnsupportedCapture::new(reason)),
    }
}

/// Platforms without a capture backend yet announce capture as unavailable.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn enabled_backend(_cfg: &NetflowConfig) -> Arc<dyn SubprocessCapture> {
    Arc::new(UnsupportedCapture::new(
        "subprocess capture backend not built for this platform yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::{CallAttribution, MemorySink};
    // Only the macOS backend test and the unsupported-platform test inspect
    // `CaptureState`; the linux backend test does not, so importing it there
    // trips `-D unused-imports` (the musl lint job).
    #[cfg(not(target_os = "linux"))]
    use rupu_netflow::CaptureState;

    fn cfg(on: bool) -> NetflowConfig {
        NetflowConfig {
            subprocess_capture: on,
            ..NetflowConfig::default()
        }
    }

    fn attribution(sink: &Arc<MemorySink>) -> CallAttribution {
        CallAttribution {
            run_id: "run_1".into(),
            step_id: None,
            agent: None,
            codename: None,
            tool_call_id: "toolu_1".into(),
            sink: sink.clone(),
        }
    }

    async fn settle() {
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn disabled_config_returns_noop() {
        let cap = choose(&cfg(false), false);
        let sink = Arc::new(MemorySink::default());
        let call = cap.begin(attribution(&sink));
        settle().await;
        assert!(call.shell_prefix().is_none());
        assert!(sink.capture_states().is_empty());
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    #[tokio::test]
    async fn enabled_config_returns_backend_that_announces_unavailable() {
        let cap = choose(&cfg(true), false);
        let sink = Arc::new(MemorySink::default());
        let call = cap.begin(attribution(&sink));
        settle().await;
        assert!(call.shell_prefix().is_none());
        let lines = sink.capture_states();
        assert_eq!(lines.len(), 1);
        assert!(matches!(lines[0].state, CaptureState::Unavailable { .. }));
    }

    /// Proves the linux arm wires `LinuxCapture` (a real cgroup-backed
    /// prefix), not `UnsupportedCapture`. Needs cgroup delegation.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial_test::serial]
    #[ignore = "creates a cgroup root; needs cgroup v2 delegation (run on kali6)"]
    async fn enabled_on_linux_returns_a_working_backend() {
        let cap = choose(&cfg(true), false);
        let sink = Arc::new(MemorySink::default());
        let call = cap.begin(attribution(&sink));
        settle().await;
        assert!(
            call.shell_prefix().is_some(),
            "linux backend should hand back a cgroup shell prefix; states: {:?}",
            sink.capture_states()
        );
    }

    /// Proves the macOS arm wires `MacosCapture` (an Active capture_state),
    /// not `UnsupportedCapture` (an Unavailable one).
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[serial_test::serial]
    #[ignore = "opens a network-statistics control socket (live; run on a Mac)"]
    async fn enabled_on_macos_returns_a_working_backend() {
        let cap = choose(&cfg(true), false);
        let sink = Arc::new(MemorySink::default());
        let mut call = cap.begin(attribution(&sink));
        call.spawned(std::process::id());
        call.finished();
        // run_finished flushes the watcher's queued state lines to the sink.
        cap.run_finished("run_1");
        let mut lines = sink.capture_states();
        for _ in 0..30 {
            if !lines.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            lines = sink.capture_states();
        }
        assert!(
            lines
                .iter()
                .all(|l| !matches!(l.state, CaptureState::Unavailable { .. })),
            "macos backend should not announce unavailable; states: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l.state, CaptureState::Active { .. })),
            "macos backend should announce Active; states: {lines:?}"
        );
    }

    #[test]
    fn linger_from_clamps_absurd_values() {
        assert_eq!(linger_from(3000), chrono::Duration::milliseconds(3000));
        assert_eq!(linger_from(0), chrono::Duration::zero());
        // Must not panic, and must stay small enough for `now - linger`.
        let huge = linger_from(u64::MAX);
        assert_eq!(huge, chrono::Duration::milliseconds(MAX_LINGER_MS));
        let _ = chrono::Utc::now() - huge;
    }

    #[tokio::test]
    async fn env_override_forces_noop() {
        let cap = choose(&cfg(true), true);
        let sink = Arc::new(MemorySink::default());
        let call = cap.begin(attribution(&sink));
        settle().await;
        assert!(call.shell_prefix().is_none());
        assert!(sink.capture_states().is_empty());
    }

    #[test]
    fn shared_is_memoized() {
        let a = shared(&cfg(true));
        let b = shared(&cfg(false));
        assert!(Arc::ptr_eq(&a, &b));
    }
}
