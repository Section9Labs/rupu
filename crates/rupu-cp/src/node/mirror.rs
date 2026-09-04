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
            awaiting: Vec::new(),
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
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
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
                let mut f = OpenOptions::new().create(true).append(true).open(path)?;
                writeln!(f, "{line}")?;
            }
            ArtifactFile::StepResults => {
                let path = self.run_store.root.join(run_id).join("step_results.jsonl");
                let mut f = OpenOptions::new().create(true).append(true).open(path)?;
                writeln!(f, "{line}")?;
            }
            ArtifactFile::UnitCheckpoints => {
                let path = self
                    .run_store
                    .root
                    .join(run_id)
                    .join("unit_checkpoints.jsonl");
                let mut f = OpenOptions::new().create(true).append(true).open(path)?;
                writeln!(f, "{line}")?;
            }
            ArtifactFile::Transcript => {
                let path = self.transcript_mirror_path(run_id);
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut f = OpenOptions::new().create(true).append(true).open(path)?;
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
                // Defense-in-depth: a tunnel mirror run must never carry the
                // resume marker that would cause the central resume worker to
                // act on it.  Force all four resume fields to None regardless
                // of what the node sent — the node should never set them, but
                // this guard holds even if a future change ever does.
                incoming.resume_requested_at = None;
                incoming.resume_claimed_at = None;
                incoming.resume_claimed_by = None;
                incoming.resume_mode = None;
                self.run_store.update(&incoming)?;
            }
        }
        Ok(())
    }

    /// The CP-local path the run's mirrored agent transcript is written to.
    ///
    /// Mirrors the coordinator's own layout: the store root is
    /// `<global>/runs`, so the transcript lands at
    /// `<global>/transcripts/<run_id>.jsonl` — the exact path a locally-run
    /// agent's transcript would occupy, inside `/api/transcript`'s allowed
    /// roots (the CP global dir). `run_id` is validated by every caller
    /// before this is used for I/O.
    pub fn transcript_mirror_path(&self, run_id: &str) -> PathBuf {
        let global = self.run_store.root.parent().unwrap_or(&self.run_store.root);
        global.join("transcripts").join(format!("{run_id}.jsonl"))
    }

    /// Truncate (or create empty) the mirrored transcript for `run_id`.
    ///
    /// Called by the tail pump when it first starts replaying the remote
    /// transcript: `tail -n +1 -F` always replays the file from byte zero,
    /// so a respawned pump would otherwise append a second copy of every
    /// already-mirrored line. Ownership rules match [`NodeMirror::append`].
    pub fn reset_transcript(&self, run_id: &str, node_id: &str) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let existing = self.run_store.load(run_id)?;
        if existing.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        let path = self.transcript_mirror_path(run_id);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::File::create(path)?;
        Ok(())
    }

    /// Overwrite the mirrored transcript for `run_id` with `content`.
    ///
    /// Used by the tail pump's terminal path: once the remote run reaches a
    /// terminal status, a one-shot `cat` of the remote transcript replaces
    /// the tailed copy wholesale. This closes the gap where the pump's
    /// select loop is torn down with transcript lines still buffered in the
    /// tail stream — the final copy is authoritative and complete, and the
    /// overwrite also makes replays idempotent. Ownership rules match
    /// [`NodeMirror::append`].
    pub fn replace_transcript(
        &self,
        run_id: &str,
        node_id: &str,
        content: &str,
    ) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let existing = self.run_store.load(run_id)?;
        if existing.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        let path = self.transcript_mirror_path(run_id);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, content)?;
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
        record.active_step_id = None;
        record.active_step_transcript_path = None;
        self.run_store.update(&record)?;
        self.synthesize_transcript_step_result(&record);
        Ok(())
    }

    /// The store this mirror writes into. Readers that need the run's own
    /// artifacts (the tail pump's terminal pull) go through here.
    pub fn run_store(&self) -> &RunStore {
        &self.run_store
    }

    /// The CP global dir (`<global>/runs` is the store root).
    pub fn global_dir(&self) -> PathBuf {
        self.run_store
            .root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.run_store.root.clone())
    }

    /// Spec §8: the pump saw the agent transcript's first line. Point the
    /// local record's active step at the mirrored copy so the frontends'
    /// existing active-step fallback opens it live. No-op once terminal.
    pub fn note_transcript_started(&self, run_id: &str, node_id: &str) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let mut record = self.run_store.load(run_id)?;
        if record.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        if record.status.is_terminal() {
            return Ok(());
        }
        record.active_step_id = Some("agent".into());
        record.active_step_transcript_path = Some(self.transcript_mirror_path(run_id));
        self.run_store.update(&record)?;
        Ok(())
    }

    /// Make a mirrored agent transcript reachable through the existing read
    /// endpoints by synthesizing a single step-result row for it.
    ///
    /// A placed agent run produces NO step results on the executing host —
    /// its only content is the transcript. But every CP read path locates
    /// transcripts through `step_results.jsonl` rows: `/api/runs/:id`'s
    /// `steps`, the run-graph's per-node `transcript_path` (matched against
    /// `agent_run_dag`'s single `"agent"` node), and the usage rollup
    /// (`usage::run_transcript_paths`). Without a row, the mirrored bytes
    /// are unreachable — the run renders with `steps: 0` and no transcript.
    ///
    /// So: when a run finishes with an empty `step_results.jsonl` and a
    /// non-empty mirrored transcript on disk, append one linear step-result
    /// row whose `step_id` is `"agent"` (matching the synthesized DAG node)
    /// and whose `transcript_path` is the CP-local mirrored copy. Workflow
    /// runs never hit this: they don't produce a `Transcript` artifact, so
    /// the mirrored transcript file doesn't exist for them. The empty-
    /// step-results guard also makes a repeated `finish` idempotent.
    ///
    /// Best-effort by design: `finish`'s status transition must never fail
    /// because this bookkeeping did.
    fn synthesize_transcript_step_result(&self, record: &RunRecord) {
        let transcript = self.transcript_mirror_path(&record.id);
        let has_transcript = std::fs::metadata(&transcript)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if !has_transcript {
            return;
        }
        let steps_empty = self
            .run_store
            .read_step_results(&record.id)
            .map(|s| s.is_empty())
            .unwrap_or(true);
        if !steps_empty {
            return;
        }
        let row = rupu_orchestrator::StepResultRecord {
            // Matches the single node `api::graph::agent_run_dag` synthesizes
            // for bare agent runs — the graph joins on step_id.
            step_id: "agent".to_string(),
            run_id: record.id.clone(),
            transcript_path: transcript,
            output: record.final_output.clone().unwrap_or_default(),
            success: record.status == RunStatus::Completed,
            skipped: false,
            rendered_prompt: String::new(),
            kind: rupu_orchestrator::StepKind::default(),
            items: Vec::new(),
            findings: Vec::new(),
            iterations: 0,
            resolved: true,
            finished_at: record.finished_at.unwrap_or_else(Utc::now),
            loop_iteration: None,
            run_outcome: None,
        };
        if let Err(e) = self.run_store.append_step_result(&record.id, &row) {
            tracing::warn!(
                run_id = %record.id,
                error = %e,
                "failed to synthesize step-result row for mirrored transcript"
            );
        }
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
