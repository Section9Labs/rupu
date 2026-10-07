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
    /// Stable identity, minted by `Board::put_directive` when left empty; what
    /// `Board::retract_directive` names. Empty on a directive written before
    /// directives had ids, which therefore can never be retracted.
    #[serde(default)]
    pub id: String,
    pub author: String,
    pub ts: String,
    pub body: String,
    /// Participant id or role this directive targets; `None` = all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressed_to: Option<String>,
}

/// One line of the board's append-only `directives.jsonl`: a directive put, or
/// the retraction of an earlier one. The live set is the file folded in order
/// (`Board::read_directives`); nothing is ever rewritten.
///
/// Untagged, with `Retract` listed first so each shape is unambiguous and a
/// legacy line still reads:
///
/// - `{"retract": "<id>"}` is a retraction. A put line has no `retract` key, so
///   it cannot match here;
/// - anything else is tried as a bare [`Directive`]: its `author`, `ts` and
///   `body` are required, so a `{"retract": ..}` line (or any half-formed line)
///   is never mistaken for a put, while a directive written before ids existed
///   (no `id`, no wrapper) parses as a put with an empty id.
///
/// A put therefore serializes as the bare directive, exactly the shape older
/// builds wrote and read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DirectiveEvent {
    /// Lift the directive with this id from the live set.
    Retract { retract: String },
    /// Add (or, for a live id, replace) a directive.
    Put(Directive),
}

/// A directed message delivered to a participant's inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetMessage {
    pub from: String,
    pub ts: String,
    pub body: String,
}
