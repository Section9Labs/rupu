//! NodeMirror — writes artifacts streamed from a tunnel node into the
//! central [`RunStore`] so the existing read endpoints render node runs
//! as first-class runs.
//!
//! Each run is created with [`NodeMirror::create_run`], which allocates
//! the run directory and sets `worker_id = node_id` on the [`RunRecord`]
//! for host attribution.  Subsequent [`NodeMirror::append`] calls mirror
//! `events.jsonl`, `step_results.jsonl`, and `unit_checkpoints.jsonl`
//! from the node, or overwrite `run.json` from the node's own
//! [`RunRecord`] while preserving our `id` and `worker_id`.
//! [`NodeMirror::finish`] transitions the run to its terminal status.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use thiserror::Error;

use rupu_orchestrator::{RunRecord, RunStatus, RunStore, RunStoreError};

use crate::node::protocol::{ArtifactFile, RunSpec};

/// Errors returned by [`NodeMirror`] operations.
#[derive(Debug, Error)]
pub enum MirrorError {
    /// A [`RunStoreError`] from the underlying store.
    #[error("run store: {0}")]
    Store(#[from] RunStoreError),
    /// An I/O error when appending to an artifact file.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A JSON error when processing a `RunJson` artifact line.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// `run_id` was passed to `append` or `finish` without a prior `create_run`.
    #[error("run `{0}` not tracked by mirror (missing create_run?)")]
    NotTracked(String),
    /// `run_id` failed format validation (path traversal or invalid characters).
    #[error("run_id `{0}` is invalid (must start with `run_` and contain only [A-Za-z0-9_])")]
    InvalidRunId(String),
    /// The calling node does not own the run it is trying to update.
    #[error("run `{0}` does not belong to the calling node")]
    WrongNode(String),
}

/// Validates a `run_id` before allowing any store operation.
///
/// A valid run ID:
/// - Is non-empty.
/// - Starts with `run_`.
/// - Contains only ASCII alphanumeric characters and `_`
///   (no `/`, `\`, `.`, or other characters that could enable path traversal).
fn validate_run_id(id: &str) -> Result<(), MirrorError> {
    if id.is_empty()
        || !id.starts_with("run_")
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(MirrorError::InvalidRunId(id.to_string()));
    }
    Ok(())
}

/// Mirrors artifact files streamed from a remote tunnel node into the
/// central [`RunStore`].
///
/// The mirror is thread-safe: all state is behind `Arc`.
pub struct NodeMirror {
    run_store: Arc<RunStore>,
}

impl NodeMirror {
    /// Create a new mirror backed by `run_store`.
    pub fn new(run_store: Arc<RunStore>) -> Self {
        Self { run_store }
    }

    /// Allocate a run directory in the store and record the initial
    /// [`RunRecord`] with `status = Running` and `worker_id = node_id`.
    ///
    /// # Errors
    /// Returns [`MirrorError::InvalidRunId`] when `run_id` fails format
    /// validation.  Returns [`MirrorError::Store`] if the store already
    /// contains `run_id` or if directory creation fails.
    pub fn create_run(
        &self,
        run_id: &str,
        node_id: &str,
        spec: &RunSpec,
    ) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;

        let run_dir = self.run_store.root.join(run_id);
        let record = RunRecord {
            id: run_id.to_string(),
            workflow_name: spec.name.clone(),
            status: RunStatus::Running,
            inputs: spec.inputs.clone(),
            event: None,
            // workspace_id is a workspace identifier, not the node id.
            // Host attribution is carried by worker_id; leave workspace_id
            // empty so the CP never mistakes the node id for a workspace.
            workspace_id: String::new(),
            workspace_path: PathBuf::from("."),
            transcript_dir: run_dir,
            started_at: Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: Some(node_id.to_string()),
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
        };

        // Empty workflow YAML: node runs don't carry a local workflow snapshot.
        self.run_store.create(record, "")?;
        Ok(())
    }

    /// Append `line` to the artifact file identified by `file` for `run_id`.
    ///
    /// Only the node that created the run (identified by `node_id`) may
    /// append to it.  Both `run_id` format and node ownership are validated
    /// before any I/O is performed.
    ///
    /// - [`ArtifactFile::Events`] → append to `events.jsonl`.
    /// - [`ArtifactFile::StepResults`] → append to `step_results.jsonl`.
    /// - [`ArtifactFile::UnitCheckpoints`] → append to `unit_checkpoints.jsonl`.
    /// - [`ArtifactFile::RunJson`] → parse `line` as [`RunRecord`], reapply
    ///   `id` and `worker_id`, then overwrite `run.json` via
    ///   [`RunStore::update`].
    ///
    /// # Errors
    /// [`MirrorError::InvalidRunId`] when `run_id` fails format validation.
    /// [`MirrorError::Store`] when the run cannot be found in the store.
    /// [`MirrorError::WrongNode`] when `node_id` does not match the run's
    /// recorded `worker_id`.  [`MirrorError::Io`] on file-open/write failures.
    /// [`MirrorError::Json`] when a `RunJson` line cannot be parsed.
    pub fn append(
        &self,
        run_id: &str,
        node_id: &str,
        file: ArtifactFile,
        line: &str,
    ) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;

        // Ownership check: the run must exist in the store and must belong to
        // `node_id`.  This prevents a connected node from writing into runs
        // that belong to a different node.
        let existing = self.run_store.load(run_id)?;
        if existing.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }

        match file {
            ArtifactFile::Events => {
                let path = self.run_store.events_path(run_id);
                let mut f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                writeln!(f, "{line}")?;
            }
            ArtifactFile::StepResults => {
                let path = self
                    .run_store
                    .root
                    .join(run_id)
                    .join("step_results.jsonl");
                let mut f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                writeln!(f, "{line}")?;
            }
            ArtifactFile::UnitCheckpoints => {
                let path = self
                    .run_store
                    .root
                    .join(run_id)
                    .join("unit_checkpoints.jsonl");
                let mut f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                writeln!(f, "{line}")?;
            }
            ArtifactFile::RunJson => {
                // Parse the node's run.json.  Re-pin the CP-local identity /
                // location fields from the record that `create_run` persisted
                // — the node's values point at paths that don't exist on the
                // CP and its workspace_id is meaningless here.  Run-state
                // fields (status, finished_at, active_step_*, etc.) are taken
                // from `incoming` — that is the point of the RunJson update.
                // Ownership was already verified above; `existing` carries the
                // CP-local fields to re-apply.
                let mut incoming: RunRecord = serde_json::from_str(line)?;
                incoming.id = existing.id;
                incoming.worker_id = existing.worker_id;
                incoming.workspace_id = existing.workspace_id;
                incoming.transcript_dir = existing.transcript_dir;
                incoming.workspace_path = existing.workspace_path;
                self.run_store.update(&incoming)?;
            }
        }
        Ok(())
    }

    /// Transition `run_id` to `status` and set `finished_at = now()`.
    ///
    /// Only the node that created the run (identified by `node_id`) may
    /// finish it.  `run_id` format is validated before any store operation.
    ///
    /// `status` is parsed leniently: unrecognised strings map to
    /// [`RunStatus::Failed`] so a malformed node status never leaves
    /// the run permanently in `Running`.
    ///
    /// # Errors
    /// [`MirrorError::InvalidRunId`] when `run_id` fails format validation.
    /// [`MirrorError::Store`] when the run cannot be loaded or written.
    /// [`MirrorError::WrongNode`] when `node_id` does not match the run's
    /// recorded `worker_id`.
    pub fn finish(&self, run_id: &str, node_id: &str, status: &str) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let mut record = self.run_store.load(run_id)?;
        if record.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        record.status = parse_status(status);
        record.finished_at = Some(Utc::now());
        self.run_store.update(&record)?;
        Ok(())
    }
}

/// Parse a status string into a [`RunStatus`] variant.
/// Unknown strings fall back to `Failed` (safe default for terminal state).
fn parse_status(s: &str) -> RunStatus {
    match s {
        "completed" => RunStatus::Completed,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        "rejected" => RunStatus::Rejected,
        _ => RunStatus::Failed,
    }
}
