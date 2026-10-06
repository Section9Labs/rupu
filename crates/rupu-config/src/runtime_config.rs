//! `[runtime]` section of `config.toml` — process-level resource settings.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Soft open-file limit (`RLIMIT_NOFILE`) to raise to at startup, and the
    /// ceiling for automatic growth under fan-out pressure. Unset ⇒ raise to
    /// the hard limit and grow up to the OS per-process maximum. Overridden
    /// by `--max-open-files` / `RUPU_MAX_OPEN_FILES`.
    pub max_open_files: Option<u64>,

    /// Hard ceiling on concurrently-dispatched *local* agent jobs, enforced
    /// process-wide regardless of any larger per-step `max_parallel:` or DAG
    /// `max_concurrency:` a workflow declares. It bounds the memory blast
    /// radius of a wide fan-out or `split:`; the memory watchdog
    /// ([`min_free_memory_mb`]) is the adaptive safety net underneath it.
    /// Unset ⇒ a RAM-derived default of `clamp(total_RAM_GB / 4, 4, 128)`.
    /// Overridden by `RUPU_MAX_CONCURRENT_JOBS`. A value of `0` is treated as
    /// unset (the default applies), never as "forbid all work".
    pub max_concurrent_jobs: Option<usize>,

    /// Memory-watchdog headroom: a new local agent job is held until at least
    /// this many MB of system memory is available, so an in-flight allocation
    /// spike cannot drive the machine out of memory. Unset ⇒ a RAM-derived
    /// default of `max(total_RAM * 0.10, 4096 MB)`. Overridden by
    /// `RUPU_MIN_FREE_MEMORY_MB`. A value of `0` disables the watchdog (the
    /// ceiling alone still applies); use it only when memory probing is
    /// unavailable or unwanted.
    pub min_free_memory_mb: Option<u64>,
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn runtime_section_defaults_are_unset() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.runtime.max_concurrent_jobs, None);
        assert_eq!(cfg.runtime.min_free_memory_mb, None);
        assert_eq!(cfg.runtime.max_open_files, None);
    }

    #[test]
    fn runtime_section_parses_through_config() {
        let toml = "[runtime]\nmax_concurrent_jobs = 24\nmin_free_memory_mb = 8192\nmax_open_files = 10240\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.runtime.max_concurrent_jobs, Some(24));
        assert_eq!(cfg.runtime.min_free_memory_mb, Some(8192));
        assert_eq!(cfg.runtime.max_open_files, Some(10240));
    }

    #[test]
    fn unknown_runtime_key_is_an_error() {
        assert!(toml::from_str::<Config>("[runtime]\nbogus = 1\n").is_err());
    }
}
