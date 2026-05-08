//! Tracked repo registry record. Stored at `~/.rupu/repos/<key>.toml`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedRepo {
    pub repo_ref: String,
    pub preferred_path: String,
    #[serde(default)]
    pub known_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origin_urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default_branch: Option<String>,
    pub last_seen_at: String,
}
