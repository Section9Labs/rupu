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

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
}

/// Mirrors artifact files streamed from a remote tunnel node into the
/// central [`RunStore`].
///
/// The mirror is thread-safe: all state is behind `Arc` + `Mutex`.
pub struct NodeMirror {
    run_store: Arc<RunStore>,
    /// Maps `run_id` → `node_id` so that `RunJson` updates can re-apply
    /// host attribution after parsing the node's raw `run.json` content.
    node_ids: Mutex<HashMap<String, String>>,
}

impl NodeMirror {
    /// Create a new mirror backed by `run_store`.
    pub fn new(run_store: Arc<RunStore>) -> Self {
        Self {
            run_store,
            node_ids: Mutex::new(HashMap::new()),
        }
    }

    /// Allocate a run directory in the store and record the initial
    /// [`RunRecord`] with `status = Running` and `worker_id = node_id`.
    ///
    /// # Errors
    /// Returns [`MirrorError::Store`] if the store already contains
    /// `run_id` or if directory creation fails.
    pub fn create_run(
        &self,
        run_id: &str,
        node_id: &str,
        spec: &RunSpec,
    ) -> Result<(), MirrorError> {
        let run_dir = self.run_store.root.join(run_id);
        let record = RunRecord {
            id: run_id.to_string(),
            workflow_name: spec.name.clone(),
            status: RunStatus::Running,
            inputs: spec.inputs.clone(),
            event: None,
            workspace_id: node_id.to_string(),
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

        self.node_ids
            .lock()
            .unwrap()
            .insert(run_id.to_string(), node_id.to_string());

        Ok(())
    }

    /// Append `line` to the artifact file identified by `file` for `run_id`.
    ///
    /// - [`ArtifactFile::Events`] → append to `events.jsonl`.
    /// - [`ArtifactFile::StepResults`] → append to `step_results.jsonl`.
    /// - [`ArtifactFile::UnitCheckpoints`] → append to `unit_checkpoints.jsonl`.
    /// - [`ArtifactFile::RunJson`] → parse `line` as [`RunRecord`], reapply
    ///   `id` and `worker_id`, then overwrite `run.json` via
    ///   [`RunStore::update`].
    ///
    /// # Errors
    /// [`MirrorError::NotTracked`] when `run_id` was never passed to
    /// [`create_run`].  [`MirrorError::Io`] on file-open/write failures.
    /// [`MirrorError::Json`] when a `RunJson` line cannot be parsed.
    pub fn append(
        &self,
        run_id: &str,
        file: ArtifactFile,
        line: &str,
    ) -> Result<(), MirrorError> {
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
                // Parse the node's run.json, then reapply our run_id and
                // node_id so host attribution is never clobbered.
                let node_id = {
                    let guard = self.node_ids.lock().unwrap();
                    guard
                        .get(run_id)
                        .ok_or_else(|| MirrorError::NotTracked(run_id.to_string()))?
                        .clone()
                };
                let mut incoming: RunRecord = serde_json::from_str(line)?;
                incoming.id = run_id.to_string();
                incoming.worker_id = Some(node_id);
                self.run_store.update(&incoming)?;
            }
        }
        Ok(())
    }

    /// Transition `run_id` to `status` and set `finished_at = now()`.
    ///
    /// `status` is parsed leniently: unrecognised strings map to
    /// [`RunStatus::Failed`] so a malformed node status never leaves
    /// the run permanently in `Running`.
    ///
    /// # Errors
    /// [`MirrorError::Store`] when the run cannot be loaded or written.
    pub fn finish(&self, run_id: &str, status: &str) -> Result<(), MirrorError> {
        let mut record = self.run_store.load(run_id)?;
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
