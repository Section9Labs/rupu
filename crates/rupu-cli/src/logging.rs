//! Logging init. Uses `tracing-subscriber` with env-filter so users
//! can `RUPU_LOG=debug rupu run ...` to see internals.
//!
//! `init` writes to stderr by default — fine for one-shot commands
//! whose entire output is line-stream text. For commands that take
//! over the terminal with a live interactive view (`rupu run`, `rupu
//! workflow run`, `rupu watch`, `rupu session attach`), the caller
//! MUST use `init_to_file` before entering raw mode. Otherwise
//! tracing punches through and corrupts the UI.

use std::path::PathBuf;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Filter used when neither `RUPU_LOG` nor `log_level` says otherwise.
///
/// WARN so internal observability (`credential backend = …`, `github: no
/// credentials configured; skipping connector`) doesn't leak into the user's
/// terminal as if it were CLI output. User-facing status / error messages go
/// through `output::diag` instead.
pub const DEFAULT_FILTER: &str = "warn";

/// Resolve the tracing directive: `RUPU_LOG` wins, then `log_level` from
/// `config.toml`, then [`DEFAULT_FILTER`].
///
/// ISSUES.md I-14: `log_level` parsed and had an editable, *lockable* field in
/// CP Settings, but logging read only `RUPU_LOG` — so an operator who locked
/// `log_level` to `warn` changed nothing, and a user who set it saw no effect.
/// The env var still wins: a one-off `RUPU_LOG=debug rupu …` must not require
/// editing config.
///
/// Both values accept any standard `tracing-subscriber` directive
/// (`debug`, `rupu_scm=debug,info`, …). Blank/whitespace values are treated as
/// unset so an empty CP Settings field doesn't silence logging.
pub fn filter_directive(cfg_level: Option<&str>, env_value: Option<&str>) -> String {
    let non_empty = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    non_empty(env_value)
        .or_else(|| non_empty(cfg_level))
        .unwrap_or_else(|| DEFAULT_FILTER.to_string())
}

/// [`filter_directive`] built into an `EnvFilter`, reading `RUPU_LOG` from the
/// process env. An unparseable directive (a typo in either source) falls back
/// to [`DEFAULT_FILTER`] rather than panicking at startup.
pub fn filter_from(cfg_level: Option<&str>) -> EnvFilter {
    let directive = filter_directive(cfg_level, std::env::var("RUPU_LOG").ok().as_deref());
    EnvFilter::try_new(&directive).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

/// Stderr-writing init for non-interactive commands. Idempotent — safe to
/// call multiple times in the same process (tests rely on this).
pub fn init(cfg_level: Option<&str>) {
    let filter = filter_from(cfg_level);
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).with_writer(std::io::stderr))
        .try_init();
}

/// File-writing init for live interactive commands. The live view
/// owns the terminal, so writing tracing lines anywhere on
/// stdout/stderr corrupts it; route them to `~/.rupu/cache/rupu.log`
/// instead. Caller is responsible for telling the user where the log
/// file lives if they need to debug.
///
/// Returns the resolved log file path on success so the caller can
/// surface it (e.g. in a help overlay or a `RUPU_LOG_FILE` echo).
/// Falls back to `init()` (stderr) if the cache dir can't be
/// created — that's worse than ideal but better than silently
/// dropping logs.
pub fn init_to_file(cfg_level: Option<&str>) -> Option<PathBuf> {
    let Some(cache_dir) = dirs::cache_dir().map(|d| d.join("rupu")) else {
        init(cfg_level);
        return None;
    };
    if std::fs::create_dir_all(&cache_dir).is_err() {
        init(cfg_level);
        return None;
    }
    let log_path = cache_dir.join("rupu.log");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    else {
        init(cfg_level);
        return None;
    };
    let filter = filter_from(cfg_level);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_wins_over_config() {
        assert_eq!(filter_directive(Some("info"), Some("debug")), "debug");
    }

    #[test]
    fn config_log_level_is_the_fallback_when_env_is_unset() {
        assert_eq!(filter_directive(Some("info"), None), "info");
        assert_eq!(
            filter_directive(Some("rupu_scm=debug,info"), None),
            "rupu_scm=debug,info"
        );
    }

    #[test]
    fn neither_source_yields_warn() {
        assert_eq!(filter_directive(None, None), "warn");
    }

    #[test]
    fn blank_values_are_treated_as_unset() {
        assert_eq!(filter_directive(Some("info"), Some("   ")), "info");
        assert_eq!(filter_directive(Some(""), None), "warn");
    }

    /// A typo in either source must not take the binary down at startup.
    #[test]
    fn an_unparseable_directive_falls_back_to_warn() {
        assert_eq!(
            filter_from(Some("=====not a directive")).to_string(),
            EnvFilter::new(DEFAULT_FILTER).to_string()
        );
    }
}
