//! Workspace record. Stored at `~/.rupu/workspaces/<id>.toml`.

use serde::{Deserialize, Serialize};

/// On-disk workspace record. One file per workspace under
/// `~/.rupu/workspaces/<id>.toml`. Created on first `rupu run` in a
/// directory; reused on subsequent runs in the same canonical path.
///
/// Note: marked `#[non_exhaustive]` is intentionally omitted here; the
/// integration tests construct this struct with literal syntax, so callers
/// can too. A constructor helper will be added in Slice C if new fields land.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// ULID-prefixed unique id, e.g. `ws_01HXXX...`.
    pub id: String,
    /// Canonical absolute path to the workspace root.
    pub path: String,
    /// Detected git remote URL of `origin` (if the path is a git repo
    /// with an `origin` remote configured at workspace creation time).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_remote: Option<String>,
    /// Detected default branch (current `HEAD` symbolic-ref short name
    /// at workspace creation time).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default_branch: Option<String>,
    /// RFC3339 timestamp of workspace creation.
    pub created_at: String,
    /// RFC3339 timestamp of the most recent `rupu run` against this
    /// workspace. Bumped on each `upsert`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_run_at: Option<String>,
}

/// ULID-prefixed workspace id, e.g. `ws_01HXXX...`. Generated fresh
/// each call; collisions are practically impossible (ULID's 80-bit
/// random component).
pub fn new_id() -> String {
    format!("ws_{}", ulid::Ulid::new())
}
