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
//!
//! Every operation that touches disk is `async` and runs its `std::fs` body
//! on tokio's blocking pool, never on an executor thread: the callers are the
//! tunnel read pump, the bucket poller and the SSH tail pump, all on the
//! runtime.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
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
    /// The blocking-pool task running the operation panicked, or the runtime
    /// shut down before it started.
    #[error("mirror task failed: {0}")]
    Task(String),
}

/// Validates a `run_id` before allowing any store operation, by the one rule
/// [`crate::host::connector::valid_run_id`] owns.
fn validate_run_id(id: &str) -> Result<(), MirrorError> {
    if !crate::host::connector::valid_run_id(id) {
        return Err(MirrorError::InvalidRunId(id.to_string()));
    }
    Ok(())
}

/// Append `line` plus its newline to `path` (created if absent) in ONE
/// `write_all` of one buffer on an `O_APPEND` descriptor, so a concurrent
/// appender to the same file can never land between a line and its newline
/// (`writeln!` would issue two writes: the line, then the `\n`).
fn append_line(path: &Path, mut line: String) -> std::io::Result<()> {
    line.push('\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// Mirrors artifact files streamed from a remote tunnel node into the
/// central [`RunStore`].
///
/// The mirror is thread-safe: all state is behind `Arc`.
///
/// Each disk-touching method makes one hop to the blocking pool
/// ([`Self::off_runtime`]). A caller dropped mid-await does not cancel the
/// operation: it still runs to completion, so a line still lands whole and a
/// replace still renames whole.
pub struct NodeMirror {
    run_store: Arc<RunStore>,
}

impl NodeMirror {
    /// Create a new mirror backed by `run_store`.
    pub fn new(run_store: Arc<RunStore>) -> Self {
        Self { run_store }
    }

    /// Run `op` against this mirror's store on tokio's blocking pool. Only a
    /// panicked (or never-started) task maps to [`MirrorError::Task`].
    async fn off_runtime<T: Send + 'static>(
        &self,
        op: impl FnOnce(&NodeMirror) -> Result<T, MirrorError> + Send + 'static,
    ) -> Result<T, MirrorError> {
        let mirror = NodeMirror::new(Arc::clone(&self.run_store));
        tokio::task::spawn_blocking(move || op(&mirror))
            .await
            .map_err(|e| MirrorError::Task(e.to_string()))?
    }

    /// Allocate a run directory in the store and record the initial
    /// [`RunRecord`] with `status = Running` and `worker_id = node_id`.
    ///
    /// # Errors
    /// Returns [`MirrorError::InvalidRunId`] when `run_id` fails format
    /// validation.  Returns [`MirrorError::Store`] if the store already
    /// contains `run_id` or if directory creation fails.
    pub async fn create_run(
        &self,
        run_id: &str,
        node_id: &str,
        spec: &RunSpec,
    ) -> Result<(), MirrorError> {
        let (run_id, node_id, spec) = (run_id.to_owned(), node_id.to_owned(), spec.clone());
        self.off_runtime(move |m| m.create_run_blocking(&run_id, &node_id, &spec))
            .await
    }

    fn create_run_blocking(
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
            resume_rerequested_at: None,
            reject_cleanup_pending: None,
            engagement_profiles: Vec::new(),
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
            gate_decisions: Vec::new(),
            system_prompt_suffix: None,
            codename: None,
            // The launch spec carries no customer; the node's own run.json
            // (`ArtifactFile::RunJson` below) brings the customer the remote
            // recorded, and that update keeps it.
            customer: None,
            cause: None,
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
    /// - [`ArtifactFile::Usage`] → append to `usage.jsonl` (the run's usage
    ///   ledger; the CP fold dedups rows by their ULID `id`, so a replayed
    ///   line is harmless).
    /// - [`ArtifactFile::Coverage`] → append to `coverage.jsonl`.
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
    pub async fn append(
        &self,
        run_id: &str,
        node_id: &str,
        file: ArtifactFile,
        line: &str,
    ) -> Result<(), MirrorError> {
        let (run_id, node_id, line) = (run_id.to_owned(), node_id.to_owned(), line.to_owned());
        self.off_runtime(move |m| m.append_blocking(&run_id, &node_id, file, line))
            .await
    }

    fn append_blocking(
        &self,
        run_id: &str,
        node_id: &str,
        file: ArtifactFile,
        line: String,
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
                append_line(&self.run_store.events_path(run_id), line)?;
            }
            ArtifactFile::StepResults => {
                let path = self.run_store.root.join(run_id).join("step_results.jsonl");
                append_line(&path, line)?;
            }
            ArtifactFile::UnitCheckpoints => {
                let path = self
                    .run_store
                    .root
                    .join(run_id)
                    .join("unit_checkpoints.jsonl");
                append_line(&path, line)?;
            }
            ArtifactFile::Usage => {
                append_line(&self.run_store.usage_ledger_path(run_id), line)?;
            }
            ArtifactFile::Transcript => {
                let path = self.transcript_mirror_path(run_id);
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                append_line(&path, line)?;
            }
            ArtifactFile::Coverage => {
                append_line(&self.coverage_path(run_id), line)?;
            }
            ArtifactFile::Netflow => {
                append_line(&self.netflow_path(run_id), line)?;
            }
            ArtifactFile::RunJson => {
                // Parse the node's run.json.  Re-pin the CP-local identity /
                // location fields from the record that `create_run` persisted
                // — the node's values point at paths that don't exist on the
                // CP and its workspace_id is meaningless here.  Run-state
                // fields (status, finished_at, active_step_*, etc.) are taken
                // from `incoming` — that is the point of the RunJson update.
                // Ownership was already verified above; `existing` carries the
                // CP-local fields to re-apply. `customer` is the node's own
                // recorded customer and is kept from `incoming`.
                let mut incoming: RunRecord = serde_json::from_str(&line)?;
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
                // Spec 2026-10-14 §N3: the landing record exists before the
                // mirrored status flips to terminal, so a dispatcher that
                // sees the run end through `get_run` finds it.
                if incoming.status.is_terminal() {
                    self.land_netflow_if_terminal(run_id, node_id);
                }
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
        crate::host::transcript_paths::agent_mirror_path(&self.global_dir(), run_id)
    }

    /// Truncate (or create empty) the mirrored transcript for `run_id`.
    ///
    /// Called by the tail pump when it first starts replaying the remote
    /// transcript: `tail -n +1 -F` always replays the file from byte zero,
    /// so a respawned pump would otherwise append a second copy of every
    /// already-mirrored line. Ownership rules match [`NodeMirror::append`].
    pub async fn reset_transcript(&self, run_id: &str, node_id: &str) -> Result<(), MirrorError> {
        let (run_id, node_id) = (run_id.to_owned(), node_id.to_owned());
        self.off_runtime(move |m| m.reset_transcript_blocking(&run_id, &node_id))
            .await
    }

    fn reset_transcript_blocking(&self, run_id: &str, node_id: &str) -> Result<(), MirrorError> {
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
    pub async fn replace_transcript(
        &self,
        run_id: &str,
        node_id: &str,
        content: &str,
    ) -> Result<(), MirrorError> {
        let (run_id, node_id, content) =
            (run_id.to_owned(), node_id.to_owned(), content.to_owned());
        self.off_runtime(move |m| m.replace_transcript_blocking(&run_id, &node_id, &content))
            .await
    }

    fn replace_transcript_blocking(
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

    /// Atomically REPLACE the mirrored usage ledger for `run_id` with `content`.
    ///
    /// Used by the SSH tail pump's terminal pull: a one-shot `cat` of the
    /// remote `usage.jsonl` is the authoritative ledger, whereas the tailed
    /// copy can miss rows written in the last poll interval before the run
    /// went terminal. The content is written to a temp file in the run
    /// directory and renamed over [`RunStore::usage_ledger_path`], so a
    /// reader never sees a torn ledger and the file gets a fresh inode (the
    /// live usage fold treats that as a reset and rebuilds, deduping rows by
    /// their ULID `id`). Ownership rules match [`NodeMirror::append`].
    ///
    /// # Errors
    /// [`MirrorError::InvalidRunId`], [`MirrorError::Store`],
    /// [`MirrorError::WrongNode`] as for `append`; [`MirrorError::Io`] when
    /// the temp file cannot be written or renamed. Either way the (possibly
    /// partly written) temp file is removed and the existing ledger is left
    /// untouched.
    pub async fn replace_usage_ledger(
        &self,
        run_id: &str,
        node_id: &str,
        content: &str,
    ) -> Result<(), MirrorError> {
        let (run_id, node_id, content) =
            (run_id.to_owned(), node_id.to_owned(), content.to_owned());
        self.off_runtime(move |m| m.replace_usage_ledger_blocking(&run_id, &node_id, &content))
            .await
    }

    fn replace_usage_ledger_blocking(
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
        let path = self.run_store.usage_ledger_path(run_id);
        let tmp = path.with_file_name(format!("usage.jsonl.{}.tmp", ulid::Ulid::new()));
        if let Err(e) = std::fs::write(&tmp, content).and_then(|()| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// `<global>/runs/<run_id>/coverage.jsonl` — the mirrored run stream.
    pub fn coverage_path(&self, run_id: &str) -> PathBuf {
        rupu_coverage::stream_path(&self.run_store.root, run_id)
    }

    /// Replace the mirrored stream with `body`, the host's file. `complete`
    /// says the run was terminal when the host's file was read, so `body` is
    /// every line it wrote (the tail may still have been behind): after the
    /// rename lands, a `.complete` mark is written next to the stream — the
    /// signal [`Self::coverage_complete`] reads. A replace that is not
    /// complete (a snapshot of a run that may still be writing) removes any
    /// mark FIRST, so a reader never pairs "complete" with a snapshot. Same
    /// ownership check as [`Self::append`].
    ///
    /// The stream can be large and the caller is the SSH tail pump on the
    /// async runtime: like every mirror write, the ownership check, the
    /// whole-file write and the rename run on the blocking pool.
    pub async fn replace_coverage(
        &self,
        run_id: &str,
        node_id: &str,
        body: &str,
        complete: bool,
    ) -> Result<(), MirrorError> {
        let (run_id, node_id, body) = (run_id.to_owned(), node_id.to_owned(), body.to_owned());
        self.off_runtime(move |m| m.replace_coverage_blocking(&run_id, &node_id, &body, complete))
            .await
    }

    fn replace_coverage_blocking(
        &self,
        run_id: &str,
        node_id: &str,
        body: &str,
        complete: bool,
    ) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let existing = self.run_store.load(run_id)?;
        if existing.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        let path = self.coverage_path(run_id);
        let mark = crate::host::transcript_paths::complete_marker(&path);
        if !complete {
            match std::fs::remove_file(&mark) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        // A per-call temp name (two replaces for one run can overlap) removed
        // on any failure, like `replace_usage_ledger`.
        let tmp = path.with_file_name(format!("coverage.jsonl.{}.tmp", ulid::Ulid::new()));
        if let Err(e) = std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        if complete {
            std::fs::write(&mark, b"")?;
        }
        Ok(())
    }

    /// Whether the mirrored stream is the host's whole file — a
    /// [`Self::replace_coverage`] with `complete` landed. `false` for an
    /// invalid run id, and for a check whose task failed (the conservative
    /// answer: the coordinator then warns rather than trusts a short stream).
    pub async fn coverage_complete(&self, run_id: &str) -> bool {
        if validate_run_id(run_id).is_err() {
            return false;
        }
        let path = self.coverage_path(run_id);
        self.off_runtime(move |_| Ok(crate::host::transcript_paths::is_complete(&path)))
            .await
            .unwrap_or(false)
    }

    /// `<global>/runs/<run_id>/netflow.jsonl` — the mirrored copy of the
    /// run's netflow stream (spec 2026-10-14 §N1).
    pub fn netflow_path(&self, run_id: &str) -> PathBuf {
        rupu_netflow::stream::stream_path(&self.run_store.root, run_id)
    }

    /// `netflow.jsonl.partial`: written by an SSH terminal pull whose copy
    /// was not confirmed whole, holding the reason; removed by a whole one.
    fn netflow_partial_marker(&self, run_id: &str) -> PathBuf {
        let mut p = self.netflow_path(run_id).into_os_string();
        p.push(".partial");
        PathBuf::from(p)
    }

    /// Replace the mirrored netflow stream with `body` (SSH's terminal
    /// pull). When `complete` is false a `netflow.jsonl.partial` marker
    /// holding `reason` says the copy was not confirmed whole; when true the
    /// marker is removed. Same ownership check as [`Self::append`]; like
    /// every mirror write, it runs on the blocking pool.
    pub async fn replace_netflow(
        &self,
        run_id: &str,
        node_id: &str,
        body: &str,
        complete: bool,
        reason: Option<String>,
    ) -> Result<(), MirrorError> {
        let (run_id, node_id, body) = (run_id.to_owned(), node_id.to_owned(), body.to_owned());
        self.off_runtime(move |m| {
            m.replace_netflow_blocking(&run_id, &node_id, &body, complete, reason)
        })
        .await
    }

    fn replace_netflow_blocking(
        &self,
        run_id: &str,
        node_id: &str,
        body: &str,
        complete: bool,
        reason: Option<String>,
    ) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let existing = self.run_store.load(run_id)?;
        if existing.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        let path = self.netflow_path(run_id);
        let marker = self.netflow_partial_marker(run_id);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_file_name(format!("netflow.jsonl.{}.tmp", ulid::Ulid::new()));
        if let Err(e) = std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        if complete {
            match std::fs::remove_file(&marker) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            std::fs::write(&marker, reason.unwrap_or_default().as_bytes())?;
        }
        Ok(())
    }

    /// Land the run's mirrored netflow stream once its run turns terminal
    /// (spec 2026-10-14 §N3): complete unless an SSH `.partial` marker says
    /// otherwise; `unavailable` when no stream arrived. Best-effort: a
    /// failure is logged and the run's terminal update proceeds (the missing
    /// record reads as "not collected"). A run that already landed complete
    /// is left alone while its mirrored copy is still the length that
    /// landed (`LandingRecord.bytes`), so a bucket poller re-mirroring
    /// `run.json` every tick costs one small read and one `metadata` call;
    /// a copy that grew since (a host-side resume appended to the stream)
    /// is landed again — complete replacing complete is allowed by the
    /// never-downgrade rule.
    fn land_netflow_if_terminal(&self, run_id: &str, node_id: &str) {
        use rupu_netflow::land::{land_and_record, read_landing_record, LandingStatus};
        let root = &self.run_store.root;
        let stream_path = self.netflow_path(run_id);
        if let Some(prev) =
            read_landing_record(root, run_id).filter(|r| r.status == LandingStatus::Complete)
        {
            // A copy that can't be measured can't be landed either, and a
            // missing one would be refused as a downgrade: skip both.
            let grew = std::fs::metadata(&stream_path).is_ok_and(|m| m.len() != prev.bytes);
            if !grew {
                return;
            }
        }
        let stream = std::fs::read(&stream_path).ok();
        let partial = std::fs::read_to_string(self.netflow_partial_marker(run_id)).ok();
        let complete = partial.is_none();
        let reason = partial.map(|r| {
            if r.trim().is_empty() {
                "the copy was not confirmed whole".to_string()
            } else {
                r
            }
        });
        if let Err(e) = land_and_record(root, run_id, node_id, stream.as_deref(), complete, reason)
        {
            tracing::warn!(
                run_id,
                node_id,
                error = %e,
                "netflow landing failed; the run's flows will read as not collected"
            );
        }
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
    pub async fn finish(
        &self,
        run_id: &str,
        node_id: &str,
        status: &str,
    ) -> Result<(), MirrorError> {
        self.finish_with_reason(run_id, node_id, status, None).await
    }

    /// [`Self::finish`], also recording why the run failed
    /// (`RunRecord.error_message`) when the host said so.
    pub async fn finish_with_reason(
        &self,
        run_id: &str,
        node_id: &str,
        status: &str,
        reason: Option<String>,
    ) -> Result<(), MirrorError> {
        let (run_id, node_id, status) = (run_id.to_owned(), node_id.to_owned(), status.to_owned());
        self.off_runtime(move |m| m.finish_blocking(&run_id, &node_id, &status, reason))
            .await
    }

    fn finish_blocking(
        &self,
        run_id: &str,
        node_id: &str,
        status: &str,
        reason: Option<String>,
    ) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let mut record = self.run_store.load(run_id)?;
        if record.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        record.status = parse_status(status);
        if reason.is_some() && record.status == RunStatus::Failed && record.error_message.is_none()
        {
            record.error_message = reason;
        }
        record.finished_at = Some(Utc::now());
        record.active_step_id = None;
        record.active_step_transcript_path = None;
        self.land_netflow_if_terminal(run_id, node_id);
        self.run_store.update(&record)?;
        self.synthesize_transcript_step_result(&record);
        Ok(())
    }

    /// The store this mirror writes into. Readers that need the run's own
    /// artifacts (the tail pump's terminal pull) go through here; the shared
    /// handle lets them take the read to the blocking pool.
    pub fn run_store(&self) -> &Arc<RunStore> {
        &self.run_store
    }

    /// The CP global dir (`<global>/runs` is the store root).
    pub fn global_dir(&self) -> PathBuf {
        crate::host::transcript_paths::global_dir_of(&self.run_store)
    }

    /// Spec §8: the pump saw the agent transcript's first line. Point the
    /// local record's active step at the mirrored copy so the frontends'
    /// existing active-step fallback opens it live. No-op once terminal.
    pub async fn note_transcript_started(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<(), MirrorError> {
        let (run_id, node_id) = (run_id.to_owned(), node_id.to_owned());
        self.off_runtime(move |m| m.note_transcript_started_blocking(&run_id, &node_id))
            .await
    }

    fn note_transcript_started_blocking(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<(), MirrorError> {
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
            host: None,
            codename: None,
            cause: None,
            error: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_spec() -> crate::node::protocol::RunSpec {
        crate::node::protocol::RunSpec {
            skill_bundle: None,
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "a".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: None,
            engagement_profiles: Vec::new(),
            skills: Vec::new(),
            preload_skills: Vec::new(),
            codename: None,
        }
    }

    /// A mirror over `<tmp>/runs` with one run owned by `node_id`.
    async fn mirror_with_run(
        tmp: &std::path::Path,
        run_id: &str,
        node_id: &str,
    ) -> (
        NodeMirror,
        std::sync::Arc<rupu_orchestrator::runs::RunStore>,
    ) {
        let store = std::sync::Arc::new(rupu_orchestrator::runs::RunStore::new(tmp.join("runs")));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        mirror
            .create_run(run_id, node_id, &agent_spec())
            .await
            .unwrap();
        (mirror, store)
    }

    #[tokio::test]
    async fn netflow_frames_append_and_land_before_the_run_turns_terminal() {
        use rupu_netflow::land::{read_landing_record, LandingStatus};
        let tmp = tempfile::tempdir().unwrap();
        let (mirror, run_store) = mirror_with_run(tmp.path(), "run_N1", "node_a").await;
        let begin = r#"{"kind":"begin","v":1,"run_id":"run_N1"}"#;
        let line = r#"{"kind":"line","ledger":"run_N1","line":{"type":"dropped","count":1,"ts":"2026-10-14T00:00:00Z"}}"#;
        let sub = r#"{"kind":"line","ledger":"sub_N1a","line":{"type":"dropped","count":2,"ts":"2026-10-14T00:00:01Z"}}"#;
        mirror
            .append("run_N1", "node_a", ArtifactFile::Netflow, begin)
            .await
            .unwrap();
        mirror
            .append("run_N1", "node_a", ArtifactFile::Netflow, line)
            .await
            .unwrap();
        mirror
            .append("run_N1", "node_a", ArtifactFile::Netflow, sub)
            .await
            .unwrap();
        assert!(mirror.netflow_path("run_N1").exists());
        assert_eq!(
            std::fs::read_to_string(mirror.netflow_path("run_N1")).unwrap(),
            format!("{begin}\n{line}\n{sub}\n")
        );
        assert!(matches!(
            mirror
                .append("run_N1", "node_b", ArtifactFile::Netflow, line)
                .await,
            Err(MirrorError::WrongNode(_))
        ));
        // No landing yet: the run is still running.
        assert!(read_landing_record(&run_store.root, "run_N1").is_none());
        let mut rec = run_store.load("run_N1").unwrap();
        rec.status = rupu_orchestrator::runs::RunStatus::Running;
        mirror
            .append(
                "run_N1",
                "node_a",
                ArtifactFile::RunJson,
                &serde_json::to_string(&rec).unwrap(),
            )
            .await
            .unwrap();
        assert!(
            read_landing_record(&run_store.root, "run_N1").is_none(),
            "a non-terminal run.json lands nothing"
        );
        // The terminal run.json lands first.
        rec.status = rupu_orchestrator::runs::RunStatus::Completed;
        mirror
            .append(
                "run_N1",
                "node_a",
                ArtifactFile::RunJson,
                &serde_json::to_string(&rec).unwrap(),
            )
            .await
            .unwrap();
        let landed = read_landing_record(&run_store.root, "run_N1").unwrap();
        assert_eq!(landed.status, LandingStatus::Complete);
        assert_eq!(landed.host, "node_a");
        assert_eq!(
            landed.ledgers,
            vec!["run_N1".to_string(), "sub_N1a".to_string()]
        );
        let global = run_store.root.parent().unwrap();
        assert!(global.join("netflow/run_N1.jsonl").exists());
        assert!(global.join("netflow/sub_N1a.jsonl").exists());
        assert_eq!(
            run_store.load("run_N1").unwrap().status,
            rupu_orchestrator::runs::RunStatus::Completed
        );
    }

    #[tokio::test]
    async fn finish_without_a_stream_records_unavailable() {
        use rupu_netflow::land::{read_landing_record, LandingStatus, NOT_STREAMED_REASON};
        let tmp = tempfile::tempdir().unwrap();
        let (mirror, run_store) = mirror_with_run(tmp.path(), "run_N2", "node_a").await;
        mirror
            .finish("run_N2", "node_a", "completed")
            .await
            .unwrap();
        let r = read_landing_record(&run_store.root, "run_N2").unwrap();
        assert_eq!(r.status, LandingStatus::Unavailable);
        assert_eq!(r.reason.as_deref(), Some(NOT_STREAMED_REASON));
        assert_eq!(r.host, "node_a");
        assert!(r.ledgers.is_empty());
        assert!(!run_store.root.parent().unwrap().join("netflow").exists());
        assert_eq!(
            run_store.load("run_N2").unwrap().status,
            rupu_orchestrator::runs::RunStatus::Completed
        );
    }

    #[tokio::test]
    async fn finish_with_a_begun_stream_lands_complete() {
        use rupu_netflow::land::{read_landing_record, LandingStatus};
        let tmp = tempfile::tempdir().unwrap();
        let (mirror, run_store) = mirror_with_run(tmp.path(), "run_N5", "node_a").await;
        mirror
            .append(
                "run_N5",
                "node_a",
                ArtifactFile::Netflow,
                r#"{"kind":"begin","v":1,"run_id":"run_N5"}"#,
            )
            .await
            .unwrap();
        mirror.finish("run_N5", "node_a", "failed").await.unwrap();
        let r = read_landing_record(&run_store.root, "run_N5").unwrap();
        assert_eq!((r.status, r.reason), (LandingStatus::Complete, None));
        assert!(r.ledgers.is_empty(), "a run with no flows lands no ledger");
    }

    #[tokio::test]
    async fn an_ssh_partial_copy_lands_partial() {
        use rupu_netflow::land::{read_landing_record, LandingStatus};
        let tmp = tempfile::tempdir().unwrap();
        let (mirror, run_store) = mirror_with_run(tmp.path(), "run_N3", "host_ssh").await;
        let body = "{\"kind\":\"begin\",\"v\":1,\"run_id\":\"run_N3\"}\n";
        mirror
            .replace_netflow(
                "run_N3",
                "host_ssh",
                body,
                false,
                Some("the host's run.json was not terminal".into()),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(mirror.netflow_path("run_N3")).unwrap(),
            body
        );
        assert!(matches!(
            mirror
                .replace_netflow("run_N3", "node_x", body, true, None)
                .await,
            Err(MirrorError::WrongNode(_))
        ));
        assert!(matches!(
            mirror
                .replace_netflow("../escape", "host_ssh", body, true, None)
                .await,
            Err(MirrorError::InvalidRunId(_))
        ));
        mirror
            .finish("run_N3", "host_ssh", "completed")
            .await
            .unwrap();
        let r = read_landing_record(&run_store.root, "run_N3").unwrap();
        assert_eq!(r.status, LandingStatus::Partial);
        assert!(r.reason.unwrap().contains("not terminal"));
        let run_dir = mirror
            .netflow_path("run_N3")
            .parent()
            .unwrap()
            .to_path_buf();
        let leftovers: Vec<_> = std::fs::read_dir(&run_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[tokio::test]
    async fn a_whole_ssh_copy_clears_the_partial_marker_and_lands_complete() {
        use rupu_netflow::land::{read_landing_record, LandingStatus};
        let tmp = tempfile::tempdir().unwrap();
        let (mirror, run_store) = mirror_with_run(tmp.path(), "run_N4", "host_ssh").await;
        let body = "{\"kind\":\"begin\",\"v\":1,\"run_id\":\"run_N4\"}\n";
        mirror
            .replace_netflow("run_N4", "host_ssh", body, false, Some("short".into()))
            .await
            .unwrap();
        assert!(mirror.netflow_partial_marker("run_N4").exists());
        mirror
            .replace_netflow("run_N4", "host_ssh", body, true, None)
            .await
            .unwrap();
        assert!(!mirror.netflow_partial_marker("run_N4").exists());
        mirror
            .finish("run_N4", "host_ssh", "completed")
            .await
            .unwrap();
        let r = read_landing_record(&run_store.root, "run_N4").unwrap();
        assert_eq!((r.status, r.reason), (LandingStatus::Complete, None));
    }

    #[tokio::test]
    async fn re_mirroring_terminal_run_json_does_not_re_land_a_complete_copy() {
        use rupu_netflow::land::{read_landing_record, LandingStatus};
        let tmp = tempfile::tempdir().unwrap();
        let (mirror, run_store) = mirror_with_run(tmp.path(), "run_N6", "node_a").await;
        for l in [
            r#"{"kind":"begin","v":1,"run_id":"run_N6"}"#,
            r#"{"kind":"line","ledger":"run_N6","line":{"type":"dropped","count":1,"ts":"2026-10-14T00:00:00Z"}}"#,
        ] {
            mirror
                .append("run_N6", "node_a", ArtifactFile::Netflow, l)
                .await
                .unwrap();
        }
        let mut rec = run_store.load("run_N6").unwrap();
        rec.status = rupu_orchestrator::runs::RunStatus::Completed;
        let terminal = serde_json::to_string(&rec).unwrap();
        mirror
            .append("run_N6", "node_a", ArtifactFile::RunJson, &terminal)
            .await
            .unwrap();
        let first = read_landing_record(&run_store.root, "run_N6").unwrap();
        assert_eq!(first.status, LandingStatus::Complete);
        let ledger = run_store
            .root
            .parent()
            .unwrap()
            .join("netflow/run_N6.jsonl");
        let mtime = std::fs::metadata(&ledger).unwrap().modified().unwrap();
        // The copy did not grow: the poller's next tick and `finish` leave
        // the complete landing alone.
        mirror
            .append("run_N6", "node_a", ArtifactFile::RunJson, &terminal)
            .await
            .unwrap();
        mirror
            .finish("run_N6", "node_a", "completed")
            .await
            .unwrap();
        let again = read_landing_record(&run_store.root, "run_N6").unwrap();
        assert_eq!(again, first, "the record is untouched (`at` equal)");
        assert_eq!(
            std::fs::metadata(&ledger).unwrap().modified().unwrap(),
            mtime,
            "the landed ledger was not rewritten"
        );
        assert_eq!(std::fs::read_to_string(&ledger).unwrap().lines().count(), 1);
    }

    /// Review finding 5: a mirrored copy that grew after a complete landing
    /// (a host-side `workflow resume` appended to the stream, and the host
    /// turned terminal again) is landed again — complete replacing
    /// complete is allowed by the never-downgrade rule — so the coordinator
    /// never keeps a stale complete ledger beside a longer copy.
    #[tokio::test]
    async fn a_copy_that_grew_after_a_complete_landing_is_re_landed() {
        use rupu_netflow::land::{read_landing_record, LandingStatus};
        let tmp = tempfile::tempdir().unwrap();
        let (mirror, run_store) = mirror_with_run(tmp.path(), "run_N7", "node_a").await;
        for l in [
            r#"{"kind":"begin","v":1,"run_id":"run_N7"}"#,
            r#"{"kind":"line","ledger":"run_N7","line":{"type":"dropped","count":1,"ts":"2026-10-14T00:00:00Z"}}"#,
        ] {
            mirror
                .append("run_N7", "node_a", ArtifactFile::Netflow, l)
                .await
                .unwrap();
        }
        mirror
            .finish("run_N7", "node_a", "completed")
            .await
            .unwrap();
        let first = read_landing_record(&run_store.root, "run_N7").unwrap();
        assert_eq!(first.status, LandingStatus::Complete);
        assert_eq!(
            first.bytes,
            std::fs::metadata(mirror.netflow_path("run_N7"))
                .unwrap()
                .len(),
            "the record holds the landed copy's length"
        );
        let ledger = run_store
            .root
            .parent()
            .unwrap()
            .join("netflow/run_N7.jsonl");
        assert_eq!(std::fs::read_to_string(&ledger).unwrap().lines().count(), 1);

        let later = r#"{"kind":"line","ledger":"run_N7","line":{"type":"dropped","count":9,"ts":"2026-10-14T00:00:02Z"}}"#;
        mirror
            .append("run_N7", "node_a", ArtifactFile::Netflow, later)
            .await
            .unwrap();
        mirror
            .finish("run_N7", "node_a", "completed")
            .await
            .unwrap();
        let again = read_landing_record(&run_store.root, "run_N7").unwrap();
        assert_eq!(again.status, LandingStatus::Complete);
        assert!(
            again.bytes > first.bytes,
            "re-landed: {} > {}",
            again.bytes,
            first.bytes
        );
        assert_eq!(
            again.bytes,
            std::fs::metadata(mirror.netflow_path("run_N7"))
                .unwrap()
                .len()
        );
        let landed = std::fs::read_to_string(&ledger).unwrap();
        assert_eq!(landed.lines().count(), 2, "{landed}");
        assert!(
            landed.contains(r#""count":9"#),
            "the new line landed: {landed}"
        );

        // Mirrored again with no growth: left alone.
        mirror
            .finish("run_N7", "node_a", "completed")
            .await
            .unwrap();
        assert_eq!(
            read_landing_record(&run_store.root, "run_N7").unwrap(),
            again
        );
    }

    #[tokio::test]
    async fn coverage_lines_append_and_replace_is_authoritative() {
        let tmp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::runs::RunStore::new(
            tmp.path().join("runs"),
        ));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        let spec = crate::node::protocol::RunSpec {
            skill_bundle: None,
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "a".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: None,
            engagement_profiles: Vec::new(),
            skills: Vec::new(),
            preload_skills: Vec::new(),
            codename: None,
        };
        mirror.create_run("run_C1", "node-1", &spec).await.unwrap();
        mirror
            .append("run_C1", "node-1", ArtifactFile::Coverage, "line-1")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(mirror.coverage_path("run_C1")).unwrap(),
            "line-1\n"
        );
        mirror
            .replace_coverage("run_C1", "node-1", "a\nb\n", false)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(mirror.coverage_path("run_C1")).unwrap(),
            "a\nb\n"
        );
        assert!(matches!(
            mirror
                .replace_coverage("run_C1", "node-2", "x", false)
                .await,
            Err(MirrorError::WrongNode(_))
        ));
    }

    /// Only a replace with the host's file read once the run was terminal
    /// marks the copy complete. A snapshot replace (the run may still be
    /// writing) clears the mark rather than sit next to it, so a reader never
    /// sees "complete" beside bytes that are not the authoritative copy.
    #[tokio::test]
    async fn replace_coverage_marks_only_an_authoritative_copy_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::runs::RunStore::new(
            tmp.path().join("runs"),
        ));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        let spec = crate::node::protocol::RunSpec {
            skill_bundle: None,
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "a".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: None,
            engagement_profiles: Vec::new(),
            skills: Vec::new(),
            preload_skills: Vec::new(),
            codename: None,
        };
        mirror.create_run("run_C3", "node-1", &spec).await.unwrap();
        assert!(
            !mirror.coverage_complete("run_C3").await,
            "nothing pulled yet"
        );

        mirror
            .replace_coverage("run_C3", "node-1", "snap\n", false)
            .await
            .unwrap();
        assert!(
            !mirror.coverage_complete("run_C3").await,
            "a snapshot is not complete"
        );

        mirror
            .replace_coverage("run_C3", "node-1", "whole\n", true)
            .await
            .unwrap();
        assert!(mirror.coverage_complete("run_C3").await);
        assert_eq!(
            std::fs::read_to_string(mirror.coverage_path("run_C3")).unwrap(),
            "whole\n"
        );

        mirror
            .replace_coverage("run_C3", "node-1", "later-snap\n", false)
            .await
            .unwrap();
        assert!(
            !mirror.coverage_complete("run_C3").await,
            "a later snapshot must clear the mark"
        );
        assert!(!mirror.coverage_complete("../escape").await);
    }

    #[tokio::test]
    async fn replace_coverage_leaves_no_temp_file_and_rejects_an_invalid_run_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::runs::RunStore::new(
            tmp.path().join("runs"),
        ));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        let spec = crate::node::protocol::RunSpec {
            skill_bundle: None,
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "a".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: None,
            engagement_profiles: Vec::new(),
            skills: Vec::new(),
            preload_skills: Vec::new(),
            codename: None,
        };
        mirror.create_run("run_C2", "node-1", &spec).await.unwrap();
        mirror
            .replace_coverage("run_C2", "node-1", "a\n", false)
            .await
            .unwrap();
        mirror
            .replace_coverage("run_C2", "node-1", "b\n", false)
            .await
            .unwrap();
        let run_dir = mirror
            .coverage_path("run_C2")
            .parent()
            .unwrap()
            .to_path_buf();
        let leftovers: Vec<_> = std::fs::read_dir(&run_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );

        assert!(matches!(
            mirror
                .replace_coverage("../escape", "node-1", "x", false)
                .await,
            Err(MirrorError::InvalidRunId(_))
        ));
        assert!(matches!(
            mirror
                .replace_coverage("not_a_run", "node-1", "x", false)
                .await,
            Err(MirrorError::InvalidRunId(_))
        ));
    }

    /// A failed rename must not leave its temp file behind.
    #[tokio::test]
    async fn replace_coverage_removes_its_temp_file_when_the_rename_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::runs::RunStore::new(
            tmp.path().join("runs"),
        ));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        let spec = crate::node::protocol::RunSpec {
            skill_bundle: None,
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "a".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: None,
            engagement_profiles: Vec::new(),
            skills: Vec::new(),
            preload_skills: Vec::new(),
            codename: None,
        };
        mirror.create_run("run_C3", "node-1", &spec).await.unwrap();
        // A directory where the stream file belongs: the write of the temp
        // file succeeds, the rename over a non-empty directory fails.
        let dest = mirror.coverage_path("run_C3");
        std::fs::create_dir_all(dest.join("occupied")).unwrap();
        assert!(mirror
            .replace_coverage("run_C3", "node-1", "a\n", true)
            .await
            .is_err());
        assert!(
            !mirror.coverage_complete("run_C3").await,
            "a replace that did not land must not be marked complete"
        );
        let run_dir = dest.parent().unwrap().to_path_buf();
        let leftovers: Vec<_> = std::fs::read_dir(&run_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    /// Appenders racing on one file (frames from a reconnected node beside an
    /// old connection's, two pumps for one run) each land whole lines: a line
    /// and its newline go out in one write, so no other line lands between.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_appends_never_tear_a_line() {
        const TASKS: u8 = 8;
        const LINES: usize = 50;
        const WIDTH: usize = 4096;
        let tmp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::runs::RunStore::new(
            tmp.path().join("runs"),
        ));
        let mirror = std::sync::Arc::new(NodeMirror::new(std::sync::Arc::clone(&store)));
        let spec = crate::node::protocol::RunSpec {
            skill_bundle: None,
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "a".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: None,
            engagement_profiles: Vec::new(),
            skills: Vec::new(),
            preload_skills: Vec::new(),
            codename: None,
        };
        mirror.create_run("run_A1", "node-1", &spec).await.unwrap();

        let tasks: Vec<_> = (0..TASKS)
            .map(|t| {
                let mirror = std::sync::Arc::clone(&mirror);
                tokio::spawn(async move {
                    let line = char::from(b'a' + t).to_string().repeat(WIDTH);
                    for _ in 0..LINES {
                        mirror
                            .append("run_A1", "node-1", ArtifactFile::Events, &line)
                            .await
                            .unwrap();
                    }
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }

        let body = std::fs::read_to_string(store.events_path("run_A1")).unwrap();
        let lines: Vec<&str> = body.split_terminator('\n').collect();
        assert_eq!(lines.len(), usize::from(TASKS) * LINES, "a line was torn");
        for line in lines {
            assert!(
                line.len() == WIDTH && line.bytes().all(|b| b == line.as_bytes()[0]),
                "a line was torn: {} bytes, starting {:?}",
                line.len(),
                &line[..line.len().min(16)]
            );
        }
    }
}
