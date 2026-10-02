use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// RAII guard for a held claim. Dropping it releases the claim by removing the
/// lock file (same pattern as `rupu-workspace`'s `ClaimLockGuard`).
#[derive(Debug)]
pub struct ClaimGuard {
    pub(crate) path: PathBuf,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Result of attempting to claim a work unit.
#[derive(Debug)]
pub enum ClaimOutcome {
    Granted(ClaimGuard),
    Denied { holder: String },
}

/// The record persisted inside a claim lock file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClaimRecord {
    pub owner: String,
    pub acquired_at: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostKind {
    Observation,
    Question,
    Answer,
    Vote,
    Note,
}

/// An append-only board post.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardPost {
    pub author: String,
    pub ts: String,
    pub kind: PostKind,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressed_to: Option<String>,
}

/// A lead→fleet standing instruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Directive {
    pub author: String,
    pub ts: String,
    pub body: String,
    /// Participant id or role this directive targets; `None` = all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressed_to: Option<String>,
}

/// A directed message delivered to a participant's inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetMessage {
    pub from: String,
    pub ts: String,
    pub body: String,
}
