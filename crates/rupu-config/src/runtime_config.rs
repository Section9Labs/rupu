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
}
