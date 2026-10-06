use rustix::fs::{flock, FlockOperation};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// RAII guard for a held claim. Dropping it releases the claim by removing the
/// lock file (same pattern as `rupu-workspace`'s `ClaimLockGuard`), but only if
/// the file still holds THIS grant's token.
#[derive(Debug)]
pub struct ClaimGuard {
    pub(crate) path: PathBuf,
    /// Per-key `.reaplock`; serializes this guard's delete against reaps.
    pub(crate) lock_path: PathBuf,
    /// Unique per grant; matches the `token` in the claim file we wrote.
    pub(crate) token: String,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        // Compare-and-delete under the per-key reap lock: remove the file only
        // if it still holds OUR grant's token. A successor's re-grant has a
        // different token, so a stale guard can never delete a live claim.
        let Ok(lock) = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock_path)
        else {
            return;
        };
        // Blocking lock: Drop is best-effort but must be correct. The OS drops
        // the lock when `lock` closes, so this can only wait on a live reaper.
        if flock(&lock, FlockOperation::LockExclusive).is_err() {
            return;
        }
        if let Ok(bytes) = std::fs::read(&self.path) {
            if let Ok(rec) = serde_json::from_slice::<ClaimRecord>(&bytes) {
                if rec.token == self.token {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }
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
    /// Unique per grant; lets a guard delete only its OWN claim file.
    #[serde(default)]
    pub token: String,
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
