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
/// The real OS backends arrive with the Linux/macOS plans; until then every
/// enabled platform gets the backend that announces capture as unavailable.
pub(crate) fn choose(cfg: &NetflowConfig, env_disabled: bool) -> Arc<dyn SubprocessCapture> {
    if !cfg.subprocess_capture || env_disabled {
        return Arc::new(NoopCapture);
    }
    Arc::new(UnsupportedCapture::new(
        "subprocess capture backend not built for this platform yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::{CallAttribution, CaptureState, MemorySink};

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
