//! `[storage]` section of `config.toml`.

use serde::{Deserialize, Serialize};

/// Lifecycle retention defaults for archived local artifacts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// Default prune cutoff for archived sessions, e.g. `30d`.
    pub archived_session_retention: Option<String>,
    /// Default prune cutoff for archived standalone transcripts, e.g. `30d`.
    pub archived_transcript_retention: Option<String>,
}
