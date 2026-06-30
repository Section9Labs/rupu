//! Persistent run state.
//!
//! Each `run_workflow` invocation writes a per-run directory under
//! `<global>/runs/<run-id>/` so completed and in-flight runs can be
//! inspected after the fact (and, in a follow-up PR, paused mid-run
//! at an approval gate and resumed later).
//!
//! Layout:
//! ```text
//! <global>/runs/<run-id>/
//!   ├── run.json           # RunRecord — status, inputs, event, timestamps, awaiting_*
//!   ├── run_envelope.json  # RunEnvelope — portable execution request snapshot
//!   ├── artifact_manifest.json # ArtifactManifest — portable output inventory
//!   ├── workflow.yaml      # snapshot of the workflow body at run start
//!   └── step_results.jsonl # one StepResultRecord per completed step (append-only)
//! ```
//!
//! The directory is created when the run starts; `run.json` and the
//! step-results log are updated atomically via a tmp-file rename so a
//! crash in the middle of a write leaves the previous coherent state
//! on disk rather than a half-written record.
//!
//! `step_results.jsonl` is append-only — each completed step is one
//! line. Skipped steps and fan-out steps both produce one line; the
//! per-item rows for fan-out are nested inside the line's JSON.

use crate::runner::{ItemResult, StepResult};
use chrono::{DateTime, Utc};
use rupu_runtime::{ArtifactManifest, RunEnvelope};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Top-level status of a workflow run, persisted in `run.json`.
///
/// State machine (PR 1 coverage in **bold**):
/// - **`Pending`** → **`Running`** when the first step starts.
/// - **`Running`** → **`Completed`** when the last step finishes
///   without error.
/// - **`Running`** → **`Failed`** when a step errors and
///   `continue_on_error` is not set.
/// - `Running` → `AwaitingApproval` (PR 2) when the runner reaches
///   an `approval: required` step.
/// - `AwaitingApproval` → `Running` (PR 2) on `rupu workflow approve`.
/// - `AwaitingApproval` → `Rejected` (PR 2) on `rupu workflow reject`.
/// - `Pending`/`Running` → `Cancelled` on `rupu workflow cancel` (or
///   the web/CP cancel control): a deliberate operator stop, distinct
///   from `Failed` (a run that errored on its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
    AwaitingApproval,
    Rejected,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }

    /// True when no further state transitions are expected. Used by
    /// `rupu workflow runs` to bucket terminal vs in-flight rows.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Rejected | Self::Cancelled
        )
    }
}

/// Identity + bookkeeping for one run. Persisted as `run.json`.
///
/// Forward-compatibility note: PR 2 will populate `awaiting_step_id`
/// and `approval_prompt` when the run pauses; PR 1 leaves them empty.
/// Adding new optional fields is non-breaking thanks to serde
/// defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// `run_<ULID>` — same shape as the per-step `run_id`s the agent
    /// runtime emits, but distinct (this one is the *workflow* run).
    pub id: String,
    /// Workflow name (filename stem).
    pub workflow_name: String,
    /// Human-readable status. Drives the CLI summary view.
    pub status: RunStatus,
    /// Resolved inputs (post `--input` parsing + defaults).
    pub inputs: std::collections::BTreeMap<String, String>,
    /// Verbatim vendor JSON for event-triggered runs; `None` for
    /// manual / cron runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<serde_json::Value>,
    /// Workspace this run was bound to. Used by the CLI to surface
    /// "this run was for project X".
    pub workspace_id: String,
    /// Filesystem path of the workspace at run-start.
    pub workspace_path: PathBuf,
    /// Directory where each step's transcript JSONL is written.
    pub transcript_dir: PathBuf,
    pub started_at: DateTime<Utc>,
    /// Set when the run reaches a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    /// Set in `Failed` status; the runner's error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// id of the step the run is paused at, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_step_id: Option<String>,
    /// Rendered approval prompt the operator sees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_prompt: Option<String>,
    /// When the run paused for approval. Set alongside
    /// `awaiting_step_id`. Used as the anchor for `expires_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_since: Option<DateTime<Utc>>,
    /// When the pending approval expires. `None` when the awaited
    /// step has no `timeout_seconds:` set. After this instant, an
    /// approve/reject attempt will fail and `rupu workflow runs`
    /// surfaces the run as expired (status flipped to `Failed`
    /// with `error_message` set on first observation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Stable text reference to the issue this run targets, when
    /// applicable. Format: `<tracker>:<project>/issues/<number>`
    /// (e.g. `github:Section9Labs/rupu/issues/42`). Stored as a
    /// string rather than a typed `IssueRef` so `runs.rs` stays
    /// independent of `rupu-scm`. The CLI's `rupu workflow runs
    /// --issue <ref>` filter compares against this verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_ref: Option<String>,
    /// Full pre-fetched issue JSON, when applicable. Persisted so
    /// the resume path (`rupu workflow approve <run-id>`) can
    /// rebind `{{issue.*}}` without making another network call to
    /// the issue tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<serde_json::Value>,
    /// When this run is a sub-agent dispatch, the parent run's id.
    /// `None` for top-level workflow runs. Sub-runs persist under
    /// the parent's directory at
    /// `<runs>/<parent_run_id>/sub/<sub_run_id>/` and don't appear
    /// in `rupu workflow runs` output by default.
    /// See `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    /// Concrete execution backend used for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_id: Option<String>,
    /// Worker identity that executed this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// Path to the persisted artifact manifest for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_manifest_path: Option<PathBuf>,
    /// PID of the local process currently executing this run, when the
    /// run is actively owned by a foreground CLI / serve worker on this
    /// machine. Cleared once the run reaches a terminal or paused state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_pid: Option<u32>,
    /// Source wake id when this run came from the durable wake queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_wake_id: Option<String>,
    /// Id of the step currently executing, when the run is in-flight.
    /// Used by foreground attach/render paths so they can begin
    /// tailing the step transcript before `step_results.jsonl` gets a
    /// completed record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_step_id: Option<String>,
    /// Workflow-step shape for the currently executing step. Optional
    /// for back-compat with older `run.json` files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_step_kind: Option<StepKind>,
    /// Agent assigned to the currently executing step, when the step
    /// uses a single named agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_step_agent: Option<String>,
    /// Transcript path for the currently executing step. Present for
    /// linear steps and any other step kinds that expose a single live
    /// transcript stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_step_transcript_path: Option<PathBuf>,
    /// When a web/delegated approval flipped the run back to `Running`
    /// and asked a background worker (not the approving request) to
    /// resume it. Acts as the "pending resume" marker that
    /// [`RunStore::list_pending_resume`] scans for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_requested_at: Option<DateTime<Utc>>,
    /// When a worker most recently claimed this run for resume. Paired
    /// with `resume_claimed_by` to form a time-bounded lease
    /// ([`RunStore::RESUME_LEASE`]); a stale lease is reclaimable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_claimed_at: Option<DateTime<Utc>>,
    /// Identity of the worker holding the current resume lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_claimed_by: Option<String>,
    /// Permission mode the operator chose when approving a delegated
    /// resume (`ask` / `bypass` / `readonly`). Set alongside
    /// `resume_requested_at` by [`RunStore::request_resume_approval`];
    /// the worker that picks the run up reads this to re-enter
    /// `run_workflow` in the requested mode. `None` when no mode was
    /// specified (or an invalid one was supplied). Cleared by
    /// [`RunStore::clear_resume`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_mode: Option<String>,
    /// Final assistant text for an agent run (set by `rupu run`); `None` for
    /// workflow runs and older records. Carried by the mirror so a remotely
    /// dispatched unit's output is retrievable centrally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_output: Option<String>,
}

/// Workflow-step shape, persisted alongside the result so the
/// printer can dispatch on it without re-inferring from items+findings.
/// Older `step_results.jsonl` records lack this field; serde defaults
/// missing values to [`StepKind::Linear`] (the inference matches what
/// the line-stream printer used pre-PR-B for any record without `items`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    #[default]
    Linear,
    ForEach,
    Parallel,
    Panel,
}

/// One entry in `step_results.jsonl`. Mirrors [`StepResult`] but with
/// types that round-trip through serde cleanly. We keep the runtime
/// `StepResult` and this on-disk record separate so internal types
/// (e.g. `serde_json::Value` for `item`) can change shape without
/// breaking the persisted format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResultRecord {
    pub step_id: String,
    pub run_id: String,
    pub transcript_path: PathBuf,
    /// Final assistant text (or JSON aggregate for fan-out).
    pub output: String,
    pub success: bool,
    pub skipped: bool,
    pub rendered_prompt: String,
    /// Workflow-step shape. Drives line-stream printer dispatch
    /// (linear / for_each / parallel / panel). Defaults to
    /// `Linear` on read for back-compat with pre-PR-B records.
    #[serde(default)]
    pub kind: StepKind,
    /// Per-unit records for fan-out steps. Empty for linear steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ItemResultRecord>,
    /// Aggregated panel findings. Empty for non-panel steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<FindingRecord>,
    /// Iteration count for panel `gate:` loops. `0` for non-panel
    /// steps and panel steps without a gate.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub iterations: u32,
    /// `true` when the panel step's gate cleared (or no gate). True
    /// for non-panel steps.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub resolved: bool,
    pub finished_at: DateTime<Utc>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}
fn is_true(b: &bool) -> bool {
    *b
}
fn default_true() -> bool {
    true
}

/// Persisted form of a [`crate::runner::Finding`]. Severity is
/// stringified so the on-disk format is stable across enum
/// variant additions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingRecord {
    pub source: String,
    pub severity: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemResultRecord {
    pub index: usize,
    pub item: serde_json::Value,
    pub sub_id: String,
    pub rendered_prompt: String,
    pub run_id: String,
    pub transcript_path: PathBuf,
    pub output: String,
    pub success: bool,
}

/// One durable per-unit checkpoint for a fan-out (`for_each`) step,
/// appended to `unit_checkpoints.jsonl` the moment a unit's agent run
/// finishes (success or failure). On `rupu workflow resume` the
/// successful checkpoints are replayed from disk instead of being
/// re-dispatched, so a partially-completed fan-out step only re-runs
/// the units that didn't already succeed.
///
/// `index` is the unit's 0-based position in the rendered `for_each`
/// list. The list is deterministic on resume (it reads the same file
/// / inputs), so the index is a stable key across runs. If the
/// rendered list length differs from what was checkpointed the runner
/// falls back to re-running every unit (it logs a warning rather than
/// trusting a stale index).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitCheckpoint {
    pub step_id: String,
    pub index: usize,
    pub item: serde_json::Value,
    pub run_id: String,
    pub transcript_path: PathBuf,
    pub output: String,
    pub success: bool,
    pub finished_at: DateTime<Utc>,
}

impl From<&StepResult> for StepResultRecord {
    fn from(sr: &StepResult) -> Self {
        Self {
            step_id: sr.step_id.clone(),
            run_id: sr.run_id.clone(),
            transcript_path: sr.transcript_path.clone(),
            output: sr.output.clone(),
            success: sr.success,
            skipped: sr.skipped,
            rendered_prompt: sr.rendered_prompt.clone(),
            kind: sr.kind,
            items: sr.items.iter().map(ItemResultRecord::from).collect(),
            findings: sr
                .findings
                .iter()
                .map(|f| FindingRecord {
                    source: f.source.clone(),
                    severity: f.severity.as_str().to_string(),
                    title: f.title.clone(),
                    body: f.body.clone(),
                })
                .collect(),
            iterations: sr.iterations,
            resolved: sr.resolved,
            finished_at: Utc::now(),
        }
    }
}

impl From<&ItemResult> for ItemResultRecord {
    fn from(i: &ItemResult) -> Self {
        Self {
            index: i.index,
            item: i.item.clone(),
            sub_id: i.sub_id.clone(),
            rendered_prompt: i.rendered_prompt.clone(),
            run_id: i.run_id.clone(),
            transcript_path: i.transcript_path.clone(),
            output: i.output.clone(),
            success: i.success,
        }
    }
}

impl From<&StepResultRecord> for StepResult {
    fn from(rec: &StepResultRecord) -> Self {
        Self {
            step_id: rec.step_id.clone(),
            rendered_prompt: rec.rendered_prompt.clone(),
            run_id: rec.run_id.clone(),
            transcript_path: rec.transcript_path.clone(),
            output: rec.output.clone(),
            success: rec.success,
            skipped: rec.skipped,
            kind: rec.kind,
            items: rec.items.iter().map(ItemResult::from).collect(),
            findings: rec
                .findings
                .iter()
                .map(|f| crate::runner::Finding {
                    source: f.source.clone(),
                    severity: crate::workflow::Severity::parse_lossy(&f.severity),
                    title: f.title.clone(),
                    body: f.body.clone(),
                })
                .collect(),
            iterations: rec.iterations,
            resolved: rec.resolved,
        }
    }
}

impl From<&ItemResultRecord> for ItemResult {
    fn from(rec: &ItemResultRecord) -> Self {
        Self {
            index: rec.index,
            item: rec.item.clone(),
            sub_id: rec.sub_id.clone(),
            rendered_prompt: rec.rendered_prompt.clone(),
            run_id: rec.run_id.clone(),
            transcript_path: rec.transcript_path.clone(),
            output: rec.output.clone(),
            success: rec.success,
        }
    }
}

#[derive(Debug, Error)]
pub enum RunStoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("run `{0}` not found")]
    NotFound(String),
    /// A run with the supplied id already exists. Surfaced when
    /// triggered runs (cron, polled events, webhooks) use deterministic
    /// run-ids: the duplicate dispatch is the *expected* behavior on
    /// re-delivery; the caller should log + skip, not panic.
    #[error("run `{0}` already exists")]
    AlreadyExists(String),
    #[error("run `{0}` is not in a terminal state (cancel it first)")]
    NotTerminal(String),
}

/// Filesystem-backed run store. One root directory; one
/// sub-directory per run. The store is stateless — every method
/// reads/writes from disk, so concurrent CLIs sharing the same
/// `<global>/runs/` see each other's updates.
pub struct RunStore {
    pub root: PathBuf,
}

impl RunStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join(run_id)
    }

    fn run_json(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("run.json")
    }

    fn step_results_log(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("step_results.jsonl")
    }

    fn unit_checkpoints_log(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("unit_checkpoints.jsonl")
    }

    /// Sub-run directory: lives under the parent's run dir so cleanup
    /// follows parent lifecycle. See spec § 5.1.
    fn sub_run_dir(&self, parent_run_id: &str, sub_run_id: &str) -> PathBuf {
        self.run_dir(parent_run_id).join("sub").join(sub_run_id)
    }

    fn sub_run_transcript(&self, parent_run_id: &str, sub_run_id: &str) -> PathBuf {
        self.sub_run_dir(parent_run_id, sub_run_id)
            .join("transcript.jsonl")
    }

    fn workflow_snapshot(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("workflow.yaml")
    }

    fn run_envelope(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("run_envelope.json")
    }

    fn artifact_manifest(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("artifact_manifest.json")
    }

    /// Create the run directory and persist initial `run.json` and
    /// the workflow YAML snapshot. Returns the created `RunRecord`.
    ///
    /// Returns [`RunStoreError::AlreadyExists`] if `run.json` is
    /// present at the resolved path — used by the cron-tick polled-
    /// events tier to skip re-delivery of the same logical event
    /// without re-firing the workflow. Manual runs use `run_<ULID>`
    /// ids which never collide so this branch is invisible to them.
    pub fn create(
        &self,
        record: RunRecord,
        workflow_yaml: &str,
    ) -> Result<RunRecord, RunStoreError> {
        let dir = self.run_dir(&record.id);
        if self.run_json(&record.id).is_file() {
            return Err(RunStoreError::AlreadyExists(record.id));
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(self.workflow_snapshot(&record.id), workflow_yaml)?;
        // Touch the step-results + unit-checkpoint logs so subsequent
        // appends don't need to create+open.
        File::create(self.step_results_log(&record.id))?;
        File::create(self.unit_checkpoints_log(&record.id))?;
        write_atomic(
            &self.run_json(&record.id),
            &serde_json::to_vec_pretty(&record)?,
        )?;
        Ok(record)
    }

    pub fn write_run_envelope(
        &self,
        run_id: &str,
        envelope: &RunEnvelope,
    ) -> Result<(), RunStoreError> {
        if self.run_json(run_id).is_file() {
            return Err(RunStoreError::AlreadyExists(run_id.to_string()));
        }
        let dir = self.run_dir(run_id);
        std::fs::create_dir_all(&dir)?;
        write_atomic(
            &self.run_envelope(run_id),
            &serde_json::to_vec_pretty(envelope)?,
        )?;
        Ok(())
    }

    /// Allocate a sub-run directory under an existing parent run and
    /// return `(sub_run_id, transcript_path)`. The `sub_run_id` is
    /// `sub_<ULID>`. Caller is the [`crate::runner`] when it spawns
    /// a child agent run via the `dispatch_agent` tool — it uses the
    /// returned id for the child agent's `run_id` and the path for
    /// `transcript_path`. See
    /// `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`
    /// § 5.1 for the directory layout.
    pub fn create_sub_run(
        &self,
        parent_run_id: &str,
        agent: &str,
    ) -> Result<(String, PathBuf), RunStoreError> {
        let _ = agent; // currently unused at the storage layer; carried
                       // for future telemetry / sub-run listing.
        let sub_run_id = format!("sub_{}", ulid::Ulid::new());
        let dir = self.sub_run_dir(parent_run_id, &sub_run_id);
        std::fs::create_dir_all(&dir)?;
        let transcript = self.sub_run_transcript(parent_run_id, &sub_run_id);
        // Touch the transcript file so the printer's tailer can attach
        // immediately (it tolerates empty/missing files but a present
        // empty file removes a class of "did the runner start yet?"
        // races during testing).
        File::create(&transcript)?;
        Ok((sub_run_id, transcript))
    }

    /// Load a run by id.
    pub fn load(&self, run_id: &str) -> Result<RunRecord, RunStoreError> {
        let path = self.run_json(run_id);
        if !path.is_file() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        let body = std::fs::read(&path)?;
        Ok(serde_json::from_slice(&body)?)
    }

    /// Update the top-level `run.json`. Used when status flips
    /// (Pending → Running → Completed/Failed/etc.) or when
    /// approval-related fields are populated.
    pub fn update(&self, record: &RunRecord) -> Result<(), RunStoreError> {
        let path = self.run_json(&record.id);
        if !path.parent().map(|p| p.exists()).unwrap_or(false) {
            return Err(RunStoreError::NotFound(record.id.clone()));
        }
        write_atomic(&path, &serde_json::to_vec_pretty(record)?)?;
        Ok(())
    }

    /// Append one completed step's record to `step_results.jsonl`.
    /// We use append-mode + a single `write_all` so the entry is
    /// either fully present or absent — no partial JSON lines.
    pub fn append_step_result(
        &self,
        run_id: &str,
        record: &StepResultRecord,
    ) -> Result<(), RunStoreError> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.step_results_log(run_id))?;
        f.write_all(&line)?;
        Ok(())
    }

    /// Read every step-result row for a run, in append order.
    pub fn read_step_results(&self, run_id: &str) -> Result<Vec<StepResultRecord>, RunStoreError> {
        let path = self.step_results_log(run_id);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let f = File::open(path)?;
        let reader = BufReader::new(f);
        let mut out = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            // Skip malformed rows rather than failing the read —
            // matches how the transcript reader treats partial
            // writes. The CLI 'show-run' view would rather render N-1
            // valid rows than fail entirely.
            if let Ok(rec) = serde_json::from_str::<StepResultRecord>(&line) {
                out.push(rec);
            }
        }
        Ok(out)
    }

    /// Append one fan-out unit's checkpoint to `unit_checkpoints.jsonl`.
    /// Append-mode + a single `write_all`, so a crash mid-write leaves
    /// the line either fully present or absent. Called by the runner as
    /// each `for_each` unit finishes (success or failure) so resume can
    /// replay the finished units.
    pub fn append_unit_checkpoint(
        &self,
        run_id: &str,
        checkpoint: &UnitCheckpoint,
    ) -> Result<(), RunStoreError> {
        let mut line = serde_json::to_vec(checkpoint)?;
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.unit_checkpoints_log(run_id))?;
        f.write_all(&line)?;
        Ok(())
    }

    /// Read every unit checkpoint for a run, in append order. A missing
    /// file yields an empty vec (a run that never reached a fan-out
    /// step, or a pre-resume-feature run). Malformed lines are skipped,
    /// mirroring `read_step_results`.
    pub fn read_unit_checkpoints(
        &self,
        run_id: &str,
    ) -> Result<Vec<UnitCheckpoint>, RunStoreError> {
        let path = self.unit_checkpoints_log(run_id);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let f = File::open(path)?;
        let reader = BufReader::new(f);
        let mut out = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(rec) = serde_json::from_str::<UnitCheckpoint>(&line) {
                out.push(rec);
            }
        }
        Ok(out)
    }

    /// Return the workflow YAML body persisted at run start.
    pub fn read_workflow_snapshot(&self, run_id: &str) -> Result<String, RunStoreError> {
        let path = self.workflow_snapshot(run_id);
        if !path.is_file() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        Ok(std::fs::read_to_string(path)?)
    }

    pub fn read_run_envelope(&self, run_id: &str) -> Result<RunEnvelope, RunStoreError> {
        let path = self.run_envelope(run_id);
        if !path.is_file() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        let body = std::fs::read(&path)?;
        Ok(serde_json::from_slice(&body)?)
    }

    pub fn run_json_path(&self, run_id: &str) -> PathBuf {
        self.run_json(run_id)
    }

    pub fn workflow_snapshot_path(&self, run_id: &str) -> PathBuf {
        self.workflow_snapshot(run_id)
    }

    pub fn run_envelope_path(&self, run_id: &str) -> PathBuf {
        self.run_envelope(run_id)
    }

    pub fn write_artifact_manifest(
        &self,
        run_id: &str,
        manifest: &ArtifactManifest,
    ) -> Result<PathBuf, RunStoreError> {
        let dir = self.run_dir(run_id);
        if !dir.exists() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        let path = self.artifact_manifest(run_id);
        write_atomic(&path, &serde_json::to_vec_pretty(manifest)?)?;
        Ok(path)
    }

    pub fn read_artifact_manifest(&self, run_id: &str) -> Result<ArtifactManifest, RunStoreError> {
        let path = self.artifact_manifest(run_id);
        if !path.is_file() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        let body = std::fs::read(&path)?;
        Ok(serde_json::from_slice(&body)?)
    }

    pub fn artifact_manifest_path(&self, run_id: &str) -> PathBuf {
        self.artifact_manifest(run_id)
    }

    /// Path to the executor's event stream log for a run.
    pub fn events_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("events.jsonl")
    }

    /// Read run records from an arbitrary runs root (active or archive),
    /// newest first. Shared by `list` / `list_archived`.
    fn list_in(root: &std::path::Path) -> Result<Vec<RunRecord>, RunStoreError> {
        let mut out: Vec<RunRecord> = Vec::new();
        let rd = match std::fs::read_dir(root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in rd.flatten() {
            let p = entry.path().join("run.json");
            if !p.is_file() {
                continue;
            }
            if let Ok(body) = std::fs::read(&p) {
                if let Ok(rec) = serde_json::from_slice::<RunRecord>(&body) {
                    out.push(rec);
                }
            }
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        Ok(out)
    }

    /// List every run currently on disk, newest-first by
    /// `started_at`. Malformed `run.json` files are skipped.
    pub fn list(&self) -> Result<Vec<RunRecord>, RunStoreError> {
        Self::list_in(&self.root)
    }

    /// Directory holding archived runs — sibling of the active runs dir
    /// (`<global>/runs` → `<global>/runs-archive`).
    fn archive_root(&self) -> PathBuf {
        self.root.with_file_name("runs-archive")
    }

    /// List archived runs (reads `<root>/../runs-archive`), newest first.
    pub fn list_archived(&self) -> Result<Vec<RunRecord>, RunStoreError> {
        Self::list_in(&self.archive_root())
    }

    /// Move `runs/<id>` → `runs-archive/<id>` (reversible). Requires the run
    /// to exist and be in a terminal state. The run dir carries its own
    /// transcript artifacts, so the rename takes them with it.
    pub fn archive(&self, run_id: &str) -> Result<(), RunStoreError> {
        let src = self.run_dir(run_id);
        if !src.join("run.json").is_file() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        let rec: RunRecord = serde_json::from_slice(&std::fs::read(src.join("run.json"))?)?;
        if !rec.status.is_terminal() {
            return Err(RunStoreError::NotTerminal(run_id.to_string()));
        }
        let dst = self.archive_root().join(run_id);
        if dst.exists() {
            return Err(RunStoreError::AlreadyExists(run_id.to_string()));
        }
        std::fs::create_dir_all(self.archive_root())?;
        std::fs::rename(&src, &dst)?;
        Ok(())
    }

    /// Move `runs-archive/<id>` → `runs/<id>`.
    pub fn restore(&self, run_id: &str) -> Result<(), RunStoreError> {
        let src = self.archive_root().join(run_id);
        if !src.join("run.json").is_file() {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        }
        let dst = self.run_dir(run_id);
        if dst.exists() {
            return Err(RunStoreError::AlreadyExists(run_id.to_string()));
        }
        std::fs::create_dir_all(&self.root)?;
        std::fs::rename(&src, &dst)?;
        Ok(())
    }

    /// Permanently remove the run directory from whichever scope holds it.
    /// No terminal-state guard here — the CP/CLI layer enforces that.
    pub fn delete(&self, run_id: &str) -> Result<(), RunStoreError> {
        let active = self.run_dir(run_id);
        let archived = self.archive_root().join(run_id);
        let target = if active.is_dir() {
            active
        } else if archived.is_dir() {
            archived
        } else {
            return Err(RunStoreError::NotFound(run_id.to_string()));
        };
        std::fs::remove_dir_all(&target)?;
        Ok(())
    }

    /// If `record` is in `AwaitingApproval` and its `expires_at`
    /// (when set) is in the past relative to `now`, transition the
    /// record to `Failed` with an "expired" error message and
    /// persist. Returns `Ok(true)` when a transition happened,
    /// `Ok(false)` otherwise. Used by the CLI's `approve` /
    /// `reject` / `runs` paths to enforce the timeout lazily — no
    /// daemon needed.
    pub fn expire_if_overdue(
        &self,
        record: &mut RunRecord,
        now: DateTime<Utc>,
    ) -> Result<bool, RunStoreError> {
        if record.status != RunStatus::AwaitingApproval {
            return Ok(false);
        }
        let Some(expires_at) = record.expires_at else {
            return Ok(false);
        };
        if now <= expires_at {
            return Ok(false);
        }
        let waited = expires_at - record.awaiting_since.unwrap_or(record.started_at);
        record.status = RunStatus::Failed;
        record.finished_at = Some(now);
        record.error_message = Some(format!(
            "approval expired: paused at step `{}` waited longer than {}s without approval",
            record.awaiting_step_id.as_deref().unwrap_or("?"),
            waited.num_seconds()
        ));
        // Keep awaiting_step_id / approval_prompt around so
        // post-mortem inspection can see what was missed; clear
        // expires_at so subsequent reads don't re-expire.
        record.expires_at = None;
        self.update(record)?;
        Ok(true)
    }
}

/// Outcome of an approve/reject library call. Returned to callers so
/// they decide how to display it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved {
        run_id: String,
        step_id: String,
    },
    Rejected {
        run_id: String,
        step_id: String,
        reason: String,
    },
    Expired {
        run_id: String,
        step_id: String,
        error: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("run not found: {0}")]
    NotFound(String),
    #[error("run is `{0}`, not `awaiting_approval`")]
    NotAwaiting(String),
    #[error("approval expired: {0}")]
    Expired(String),
    #[error("missing awaiting_step_id in record")]
    NoAwaitingStep,
    #[error("store: {0}")]
    Store(#[from] RunStoreError),
}

/// Outcome of [`RunStore::cancel`]. Returned so callers (CLI / CP web)
/// can phrase the right message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The run was paused at an approval gate; cancellation was
    /// implemented as a reject (status flips to `Rejected`).
    RejectedAwaitingApproval,
    /// The run was `Pending`/`Running`; status flipped to `Cancelled`.
    /// `pid` is the recorded runner pid (if any) and `was_running`
    /// indicates whether that pid was live (and got a TERM signal).
    MarkedCancelled { pid: Option<u32>, was_running: bool },
}

#[derive(Debug, thiserror::Error)]
pub enum CancelError {
    /// The run is already in a terminal state and cannot be cancelled.
    #[error("run is already terminal (`{}`)", .0.as_str())]
    AlreadyTerminal(RunStatus),
    #[error("run not found: {0}")]
    NotFound(String),
    #[error("store: {0}")]
    Store(String),
}

impl RunStore {
    /// Library-level approve flow: load → expire-check → mutate
    /// status → persist. Caller is responsible for re-entering
    /// `run_workflow` (CLI does this via the existing path).
    pub fn approve(
        &self,
        run_id: &str,
        approver: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ApprovalDecision, ApprovalError> {
        let mut record = self.load(run_id).map_err(|e| match e {
            RunStoreError::NotFound(s) => ApprovalError::NotFound(s),
            other => ApprovalError::Store(other),
        })?;
        if self.expire_if_overdue(&mut record, now)? {
            return Err(ApprovalError::Expired(
                record
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "paused run timed out".into()),
            ));
        }
        if record.status != RunStatus::AwaitingApproval {
            return Err(ApprovalError::NotAwaiting(
                record.status.as_str().to_string(),
            ));
        }
        let step_id = record
            .awaiting_step_id
            .clone()
            .ok_or(ApprovalError::NoAwaitingStep)?;
        let _ = approver; // identity recorded in transcript via runner re-entry
        record.status = RunStatus::Running;
        record.awaiting_step_id = None;
        record.approval_prompt = None;
        record.awaiting_since = None;
        record.expires_at = None;
        record.error_message = None;
        self.update(&record)?;
        Ok(ApprovalDecision::Approved {
            run_id: run_id.to_string(),
            step_id,
        })
    }

    pub fn reject(
        &self,
        run_id: &str,
        approver: &str,
        reason: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ApprovalDecision, ApprovalError> {
        let mut record = self.load(run_id).map_err(|e| match e {
            RunStoreError::NotFound(s) => ApprovalError::NotFound(s),
            other => ApprovalError::Store(other),
        })?;
        if self.expire_if_overdue(&mut record, now)? {
            return Err(ApprovalError::Expired(
                record
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "paused run timed out".into()),
            ));
        }
        if record.status != RunStatus::AwaitingApproval {
            return Err(ApprovalError::NotAwaiting(
                record.status.as_str().to_string(),
            ));
        }
        let step_id = record
            .awaiting_step_id
            .clone()
            .ok_or(ApprovalError::NoAwaitingStep)?;
        let _ = approver;
        record.status = RunStatus::Rejected;
        record.error_message = Some(format!("rejected: {reason}"));
        record.finished_at = Some(now);
        record.awaiting_step_id = None;
        record.approval_prompt = None;
        record.awaiting_since = None;
        record.expires_at = None;
        self.update(&record)?;
        Ok(ApprovalDecision::Rejected {
            run_id: run_id.to_string(),
            step_id,
            reason: reason.to_string(),
        })
    }

    /// How long a resume claim stays valid before it can be reclaimed
    /// by another worker. A worker that picks up a run via
    /// [`claim_resume`](Self::claim_resume) is expected to finish
    /// (re-enter `run_workflow`) and call
    /// [`clear_resume`](Self::clear_resume) within this window;
    /// otherwise the run becomes eligible for re-claim so a crashed
    /// worker doesn't strand the resume.
    pub const RESUME_LEASE: chrono::Duration = chrono::Duration::minutes(5);

    /// Web/delegated approve flow — **marker-only**. Validates the run
    /// is still `AwaitingApproval` (same expire-check + `NotAwaiting`
    /// error as [`approve`](Self::approve)), then records the
    /// `resume_requested_at` marker and persists. It does **not** flip
    /// the status or clear any pause fields: the run stays
    /// `AwaitingApproval` so a background worker — not the approving
    /// HTTP request — picks it up via
    /// [`list_pending_resume`](Self::list_pending_resume), calls
    /// [`approve`](Self::approve) (which yields the awaited step id),
    /// and re-enters `run_workflow`. Leaving the run in
    /// `AwaitingApproval` with `awaiting_step_id` intact is what lets
    /// the worker recover which gate to resume.
    ///
    /// Returns `Approved { step_id }` with the still-present
    /// `awaiting_step_id` so the web endpoint can report which gate the
    /// operator approved.
    ///
    /// `mode` is the permission mode the operator chose for the resumed
    /// run (`ask` / `bypass` / `readonly`). It is validated and stored on
    /// `resume_mode`; anything outside the three known modes (or `None`)
    /// stores `None`, leaving the worker to fall back to its default.
    pub fn request_resume_approval(
        &self,
        run_id: &str,
        approver: &str,
        mode: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ApprovalDecision, ApprovalError> {
        let mut record = self.load(run_id).map_err(|e| match e {
            RunStoreError::NotFound(s) => ApprovalError::NotFound(s),
            other => ApprovalError::Store(other),
        })?;
        if self.expire_if_overdue(&mut record, now)? {
            return Err(ApprovalError::Expired(
                record
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "paused run timed out".into()),
            ));
        }
        if record.status != RunStatus::AwaitingApproval {
            return Err(ApprovalError::NotAwaiting(
                record.status.as_str().to_string(),
            ));
        }
        let step_id = record
            .awaiting_step_id
            .clone()
            .ok_or(ApprovalError::NoAwaitingStep)?;
        let _ = approver; // identity recorded in transcript via runner re-entry
                          // Marker-only: leave status AwaitingApproval and keep
                          // awaiting_step_id / approval_prompt / awaiting_since /
                          // expires_at intact for the worker to approve+resume.
        record.resume_requested_at = Some(now);
        record.resume_mode = mode
            .filter(|m| matches!(*m, "ask" | "bypass" | "readonly"))
            .map(str::to_string);
        self.update(&record)?;
        Ok(ApprovalDecision::Approved {
            run_id: run_id.to_string(),
            step_id,
        })
    }

    /// Runs that a web/delegated approval marked for resume and that no
    /// worker currently holds a live lease on. A run is pending when it
    /// is still `AwaitingApproval`, has a `resume_requested_at` marker,
    /// AND either has no claim or a claim older than
    /// [`RESUME_LEASE`](Self::RESUME_LEASE).
    ///
    /// The `AwaitingApproval` requirement guards against a
    /// reject-after-approve race: if the run was rejected (or otherwise
    /// finished) after the marker was set, its status is no longer
    /// `AwaitingApproval` and the stale marker must not cause the worker
    /// to resume a terminal run.
    pub fn list_pending_resume(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RunRecord>, RunStoreError> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|r| r.status == RunStatus::AwaitingApproval)
            .filter(|r| r.resume_requested_at.is_some())
            .filter(|r| match r.resume_claimed_at {
                None => true,
                Some(claimed) => now - claimed > Self::RESUME_LEASE,
            })
            .collect())
    }

    /// Try to claim a pending-resume run for `worker_id`. Returns
    /// `Ok(true)` when the claim was taken (lease was free or stale),
    /// `Ok(false)` when another worker holds a live lease.
    pub fn claim_resume(
        &self,
        run_id: &str,
        worker_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, RunStoreError> {
        let mut record = self.load(run_id)?;
        if let Some(claimed) = record.resume_claimed_at {
            if now - claimed <= Self::RESUME_LEASE {
                return Ok(false);
            }
        }
        record.resume_claimed_at = Some(now);
        record.resume_claimed_by = Some(worker_id.to_string());
        self.update(&record)?;
        Ok(true)
    }

    /// Clear the pending-resume marker + claim after a worker finishes
    /// (or abandons) a resume. `now` is accepted for signature symmetry
    /// with the other resume methods even though it isn't used.
    pub fn clear_resume(
        &self,
        run_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), RunStoreError> {
        let _ = now;
        let mut record = self.load(run_id)?;
        record.resume_requested_at = None;
        record.resume_claimed_at = None;
        record.resume_claimed_by = None;
        record.resume_mode = None;
        self.update(&record)?;
        Ok(())
    }

    /// Cancel a run. This is the shared backend for `rupu workflow
    /// cancel` and the CP web cancel control.
    ///
    /// - Terminal runs (`Completed`/`Failed`/`Rejected`/`Cancelled`)
    ///   yield [`CancelError::AlreadyTerminal`].
    /// - A run paused at an approval gate (`AwaitingApproval`) is
    ///   cancelled by rejecting it with `reason` — status flips to
    ///   `Rejected`; returns [`CancelOutcome::RejectedAwaitingApproval`].
    /// - A `Pending`/`Running` run is marked `Cancelled`: if its
    ///   recorded `runner_pid` is live AND is not our own process it is
    ///   sent SIGTERM, the pause and active-step fields are cleared,
    ///   `finished_at`/`error_message` are set, and
    ///   [`CancelOutcome::MarkedCancelled`] is returned.
    ///
    /// # Limitation
    ///
    /// Cancelling a run that is being resumed in-process by `cp serve`
    /// marks it `Cancelled` but cannot interrupt the in-flight resume
    /// task (no cooperative cancellation yet); the resume may run to
    /// completion. Cancelling a run owned by a *separate* process (e.g.
    /// `rupu run`) sends SIGTERM and stops it.
    pub fn cancel(
        &self,
        run_id: &str,
        approver: &str,
        reason: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<CancelOutcome, CancelError> {
        let mut record = self.load(run_id).map_err(|e| match e {
            RunStoreError::NotFound(s) => CancelError::NotFound(s),
            other => CancelError::Store(other.to_string()),
        })?;
        match record.status {
            RunStatus::Completed
            | RunStatus::Failed
            | RunStatus::Rejected
            | RunStatus::Cancelled => Err(CancelError::AlreadyTerminal(record.status)),
            RunStatus::AwaitingApproval => {
                self.reject(run_id, approver, reason, now)
                    .map_err(|e| match e {
                        ApprovalError::NotFound(s) => CancelError::NotFound(s),
                        other => CancelError::Store(other.to_string()),
                    })?;
                Ok(CancelOutcome::RejectedAwaitingApproval)
            }
            RunStatus::Pending | RunStatus::Running => {
                let pid = record.runner_pid;
                let was_running = pid.is_some_and(pid_is_running);
                // Only signal a pid that is live AND is NOT our own
                // process. A web-approved gate is resumed in-process
                // inside `cp serve`, so the run's `runner_pid` can be the
                // cp-serve PID itself — SIGTERMing it would kill the whole
                // control plane (web server + resume worker + every
                // in-flight resume). The run is still marked `Cancelled`
                // below; we just cannot interrupt an in-process resume via
                // signal (see the limitation note on `cancel`).
                if let Some(pid) =
                    pid.filter(|pid| pid_is_running(*pid) && *pid != std::process::id())
                {
                    let _ = terminate_pid(pid);
                }
                record.status = RunStatus::Cancelled;
                record.finished_at = Some(now);
                record.error_message = Some(reason.to_string());
                record.awaiting_step_id = None;
                record.approval_prompt = None;
                record.awaiting_since = None;
                record.expires_at = None;
                record.runner_pid = None;
                record.active_step_id = None;
                record.active_step_kind = None;
                record.active_step_agent = None;
                record.active_step_transcript_path = None;
                self.update(&record)
                    .map_err(|e| CancelError::Store(e.to_string()))?;
                Ok(CancelOutcome::MarkedCancelled { pid, was_running })
            }
        }
    }
}

/// True when `pid` names a live process on this machine. Shells out to
/// `/bin/kill -0` (the no-op signal): exit 0 means the process exists.
fn pid_is_running(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Send SIGTERM to `pid` via `/bin/kill -TERM`. Returns whether the
/// signal was delivered successfully.
fn terminate_pid(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Atomic write: write to `path.tmp`, then rename. POSIX rename is
/// atomic within a directory, so a crash mid-write leaves either the
/// previous coherent file or no `.tmp` (which a future write
/// overwrites) — never a half-written `run.json`.
fn write_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_runtime::{
        ArtifactKind, ArtifactManifest, ArtifactRef, ExecutionRequest, RepoBinding, RunContext,
        RunEnvelope, RunKind, RunTrigger, RunTriggerSource, WorkflowBinding,
    };
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn sample_record(id: &str) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: "investigate-then-fix".into(),
            status: RunStatus::Pending,
            inputs: BTreeMap::from([("prompt".into(), "fix x".into())]),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: PathBuf::from("/tmp/proj"),
            transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
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
            worker_id: None,
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
            final_output: None,
        }
    }

    fn sample_step_result(step_id: &str) -> StepResultRecord {
        StepResultRecord {
            step_id: step_id.into(),
            run_id: format!("run_step_{step_id}"),
            transcript_path: PathBuf::from(format!("/tmp/{step_id}.jsonl")),
            output: format!("output of {step_id}"),
            success: true,
            skipped: false,
            rendered_prompt: format!("prompt for {step_id}"),
            kind: StepKind::Linear,
            items: Vec::new(),
            findings: Vec::new(),
            iterations: 0,
            resolved: true,
            finished_at: Utc::now(),
        }
    }

    fn sample_envelope(run_id: &str) -> RunEnvelope {
        RunEnvelope {
            version: RunEnvelope::VERSION,
            run_id: run_id.into(),
            kind: RunKind::WorkflowRun,
            workflow: WorkflowBinding {
                name: "investigate-then-fix".into(),
                source_path: PathBuf::from(".rupu/workflows/investigate-then-fix.yaml"),
                fingerprint: "sha256:abc123".into(),
            },
            repo: Some(RepoBinding {
                repo_ref: Some("github:Section9Labs/rupu".into()),
                project_root: Some(PathBuf::from("/tmp/proj")),
                workspace_id: "ws_1".into(),
                workspace_path: PathBuf::from("/tmp/proj"),
            }),
            trigger: RunTrigger {
                source: RunTriggerSource::WorkflowCli,
                wake_id: None,
                event_id: None,
            },
            inputs: BTreeMap::from([("prompt".into(), "fix x".into())]),
            context: Some(RunContext {
                issue_ref: None,
                target: None,
                event_present: false,
                issue_present: false,
            }),
            execution: ExecutionRequest {
                backend: Some("local_checkout".into()),
                permission_mode: "bypass".into(),
                workspace_strategy: Some("in_place_checkout".into()),
                strict_templates: false,
                attach_ui: false,
            },
            autoflow: None,
            correlation: None,
            worker: None,
        }
    }

    #[test]
    fn create_with_existing_id_returns_already_exists() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = sample_record("evt-triage-github-12345");
        let yaml = "name: x\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n";

        // First create succeeds.
        store.create(rec.clone(), yaml).unwrap();
        // Second create with the same id returns AlreadyExists.
        let err = store.create(rec.clone(), yaml).unwrap_err();
        assert!(matches!(err, RunStoreError::AlreadyExists(id) if id == "evt-triage-github-12345"));
    }

    #[test]
    fn create_persists_run_json_and_workflow_snapshot() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = sample_record("run_01HX");
        let yaml = "name: x\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n";
        store.create(rec.clone(), yaml).unwrap();

        // run.json round-trips
        let loaded = store.load(&rec.id).unwrap();
        assert_eq!(loaded.id, rec.id);
        assert_eq!(loaded.status, RunStatus::Pending);
        assert_eq!(loaded.workflow_name, "investigate-then-fix");

        // workflow snapshot round-trips
        let snap = store.read_workflow_snapshot(&rec.id).unwrap();
        assert_eq!(snap, yaml);
    }

    #[test]
    fn write_and_read_run_envelope_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let envelope = sample_envelope("run_env_01");

        store
            .write_run_envelope(&envelope.run_id, &envelope)
            .unwrap();
        let loaded = store.read_run_envelope(&envelope.run_id).unwrap();
        assert_eq!(loaded, envelope);
    }

    #[test]
    fn write_run_envelope_rejects_existing_run_id() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = sample_record("evt-triage-github-12345");
        let envelope = sample_envelope(&rec.id);
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let err = store.write_run_envelope(&rec.id, &envelope).unwrap_err();
        assert!(matches!(err, RunStoreError::AlreadyExists(id) if id == rec.id));
    }

    #[test]
    fn write_and_read_artifact_manifest_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = sample_record("run_artifacts_01");
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let manifest = ArtifactManifest {
            version: ArtifactManifest::VERSION,
            run_id: rec.id.clone(),
            backend_id: "local_worktree".into(),
            worker_id: Some("worker_local_cli".into()),
            generated_at: Utc::now().to_rfc3339(),
            artifacts: vec![ArtifactRef {
                id: "art_run".into(),
                kind: ArtifactKind::RunRecord,
                name: "run-record".into(),
                producer: "run".into(),
                local_path: Some(PathBuf::from("/tmp/run.json")),
                uri: None,
                inline_json: None,
            }],
        };

        let written = store.write_artifact_manifest(&rec.id, &manifest).unwrap();
        assert_eq!(written, store.artifact_manifest_path(&rec.id));

        let loaded = store.read_artifact_manifest(&rec.id).unwrap();
        assert_eq!(loaded, manifest);
    }

    #[test]
    fn update_flips_status_and_persists() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = sample_record("run_02");
        store
            .create(
                rec.clone(),
                "name: x\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n",
            )
            .unwrap();

        let mut updated = rec.clone();
        updated.status = RunStatus::Running;
        store.update(&updated).unwrap();
        assert_eq!(store.load(&rec.id).unwrap().status, RunStatus::Running);

        updated.status = RunStatus::Completed;
        updated.finished_at = Some(Utc::now());
        store.update(&updated).unwrap();
        let loaded = store.load(&rec.id).unwrap();
        assert_eq!(loaded.status, RunStatus::Completed);
        assert!(loaded.finished_at.is_some());
        assert!(loaded.status.is_terminal());
    }

    #[test]
    fn append_and_read_step_results_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = sample_record("run_03");
        store
            .create(
                rec.clone(),
                "name: x\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n",
            )
            .unwrap();

        store
            .append_step_result(&rec.id, &sample_step_result("a"))
            .unwrap();
        store
            .append_step_result(&rec.id, &sample_step_result("b"))
            .unwrap();

        let rows = store.read_step_results(&rec.id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].step_id, "a");
        assert_eq!(rows[1].step_id, "b");
    }

    #[test]
    fn append_and_read_unit_checkpoints_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = sample_record("run_units");
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let cp0 = UnitCheckpoint {
            step_id: "review_each".into(),
            index: 0,
            item: serde_json::json!("a.rs"),
            run_id: "run_unit_a".into(),
            transcript_path: PathBuf::from("/tmp/a.jsonl"),
            output: "reviewed a".into(),
            success: true,
            finished_at: Utc::now(),
        };
        let cp1 = UnitCheckpoint {
            step_id: "review_each".into(),
            index: 1,
            item: serde_json::json!("b.rs"),
            run_id: "run_unit_b".into(),
            transcript_path: PathBuf::from("/tmp/b.jsonl"),
            output: String::new(),
            success: false,
            finished_at: Utc::now(),
        };
        store.append_unit_checkpoint(&rec.id, &cp0).unwrap();
        store.append_unit_checkpoint(&rec.id, &cp1).unwrap();

        let rows = store.read_unit_checkpoints(&rec.id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].index, 0);
        assert!(rows[0].success);
        assert_eq!(rows[0].item, serde_json::json!("a.rs"));
        assert_eq!(rows[1].index, 1);
        assert!(!rows[1].success);
    }

    #[test]
    fn read_unit_checkpoints_missing_file_is_empty() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        // No run created, no file on disk.
        let rows = store.read_unit_checkpoints("never_ran").unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn list_returns_runs_newest_first() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        // Create two runs with explicit started_at gaps.
        let mut r1 = sample_record("run_a");
        r1.started_at = Utc::now() - chrono::Duration::seconds(10);
        let mut r2 = sample_record("run_b");
        r2.started_at = Utc::now();
        store.create(r1.clone(), "x").unwrap();
        store.create(r2.clone(), "x").unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 2);
        // Newest first.
        assert_eq!(listed[0].id, r2.id);
        assert_eq!(listed[1].id, r1.id);
    }

    #[test]
    fn list_skips_malformed_run_json() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        // Valid run.
        let r1 = sample_record("run_ok");
        store.create(r1.clone(), "x").unwrap();
        // Hand-crafted broken run dir.
        let bad_dir = tmp.path().join("run_broken");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("run.json"), "not json").unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1, "broken run dir should be skipped");
        assert_eq!(listed[0].id, "run_ok");
    }

    #[test]
    fn load_unknown_run_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let err = store.load("does_not_exist").unwrap_err();
        assert!(matches!(err, RunStoreError::NotFound(_)));
    }

    #[test]
    fn expire_if_overdue_does_nothing_when_not_paused() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let mut rec = sample_record("run_a");
        rec.status = RunStatus::Running;
        store.create(rec.clone(), "x").unwrap();
        let mut loaded = rec;
        let flipped = store.expire_if_overdue(&mut loaded, Utc::now()).unwrap();
        assert!(!flipped);
        assert_eq!(loaded.status, RunStatus::Running);
    }

    #[test]
    fn expire_if_overdue_does_nothing_when_no_expires_at() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let mut rec = sample_record("run_a");
        rec.status = RunStatus::AwaitingApproval;
        rec.awaiting_step_id = Some("deploy".into());
        rec.awaiting_since = Some(Utc::now() - chrono::Duration::days(30));
        // expires_at intentionally None: timeout not configured.
        store.create(rec.clone(), "x").unwrap();
        let mut loaded = rec;
        let flipped = store.expire_if_overdue(&mut loaded, Utc::now()).unwrap();
        assert!(!flipped, "no timeout configured → never expires");
        assert_eq!(loaded.status, RunStatus::AwaitingApproval);
    }

    #[test]
    fn expire_if_overdue_does_nothing_when_within_window() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();
        let mut rec = sample_record("run_a");
        rec.status = RunStatus::AwaitingApproval;
        rec.awaiting_step_id = Some("deploy".into());
        rec.awaiting_since = Some(now);
        rec.expires_at = Some(now + chrono::Duration::seconds(60));
        store.create(rec.clone(), "x").unwrap();
        let mut loaded = rec;
        let flipped = store.expire_if_overdue(&mut loaded, now).unwrap();
        assert!(!flipped);
        assert_eq!(loaded.status, RunStatus::AwaitingApproval);
    }

    #[test]
    fn expire_if_overdue_flips_status_and_persists_when_past_deadline() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();
        let mut rec = sample_record("run_a");
        rec.status = RunStatus::AwaitingApproval;
        rec.awaiting_step_id = Some("deploy".into());
        rec.awaiting_since = Some(now - chrono::Duration::seconds(120));
        rec.expires_at = Some(now - chrono::Duration::seconds(30));
        store.create(rec.clone(), "x").unwrap();

        let mut loaded = rec;
        let flipped = store.expire_if_overdue(&mut loaded, now).unwrap();
        assert!(flipped);
        assert_eq!(loaded.status, RunStatus::Failed);
        assert!(loaded.finished_at.is_some());
        let msg = loaded.error_message.as_deref().unwrap();
        assert!(msg.contains("approval expired"));
        assert!(msg.contains("deploy"));
        assert!(loaded.expires_at.is_none(), "cleared after flip");
        // Persisted to disk too.
        let reloaded = store.load("run_a").unwrap();
        assert_eq!(reloaded.status, RunStatus::Failed);
    }

    #[test]
    fn expire_if_overdue_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();
        let mut rec = sample_record("run_a");
        rec.status = RunStatus::AwaitingApproval;
        rec.awaiting_step_id = Some("deploy".into());
        rec.expires_at = Some(now - chrono::Duration::seconds(30));
        store.create(rec.clone(), "x").unwrap();

        let mut loaded = rec;
        assert!(store.expire_if_overdue(&mut loaded, now).unwrap());
        // Second call should be a no-op since status is no longer
        // AwaitingApproval.
        assert!(!store.expire_if_overdue(&mut loaded, now).unwrap());
    }

    const SAMPLE_YAML: &str =
        "name: x\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n";

    #[test]
    fn approve_flips_running_and_clears_pause_fields() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let mut rec = sample_record("run_appr_test");
        rec.status = RunStatus::AwaitingApproval;
        rec.awaiting_step_id = Some("deploy".into());
        rec.approval_prompt = Some("ok?".into());
        rec.awaiting_since = Some(Utc::now());
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let decision = store.approve(&rec.id, "matt", Utc::now()).unwrap();
        assert!(matches!(decision, ApprovalDecision::Approved { .. }));
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::Running);
        assert!(reloaded.awaiting_step_id.is_none());
        assert!(reloaded.approval_prompt.is_none());
    }

    #[test]
    fn step_kind_round_trips_through_jsonl() {
        // Each variant must round-trip cleanly through serde so the
        // line-stream printer can dispatch on the persisted value.
        for kind in [
            StepKind::Linear,
            StepKind::ForEach,
            StepKind::Parallel,
            StepKind::Panel,
        ] {
            let mut rec = sample_step_result("k");
            rec.kind = kind;
            let json = serde_json::to_string(&rec).unwrap();
            let parsed: StepResultRecord = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.kind, kind);
        }
    }

    #[test]
    fn step_result_record_without_kind_field_defaults_to_linear() {
        // Back-compat: pre-PR-B step_results.jsonl files have records
        // without `kind`. They must deserialize as Linear so older
        // runs still render correctly when the user re-attaches.
        let json = serde_json::json!({
            "step_id": "ancient",
            "run_id": "run_old",
            "transcript_path": "/tmp/old.jsonl",
            "output": "x",
            "success": true,
            "skipped": false,
            "rendered_prompt": "p",
            "finished_at": Utc::now().to_rfc3339(),
        });
        let parsed: StepResultRecord = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.kind, StepKind::Linear);
    }

    #[test]
    fn reject_flips_to_rejected_and_records_reason() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let mut rec = sample_record("run_rej_test");
        rec.status = RunStatus::AwaitingApproval;
        rec.awaiting_step_id = Some("deploy".into());
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let decision = store
            .reject(&rec.id, "matt", "looks risky", Utc::now())
            .unwrap();
        assert!(matches!(decision, ApprovalDecision::Rejected { .. }));
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::Rejected);
        assert!(reloaded.error_message.unwrap().contains("looks risky"));
    }

    fn awaiting_record(id: &str) -> RunRecord {
        let mut rec = sample_record(id);
        rec.status = RunStatus::AwaitingApproval;
        rec.awaiting_step_id = Some("deploy".into());
        rec.approval_prompt = Some("ok?".into());
        rec.awaiting_since = Some(Utc::now());
        rec
    }

    #[test]
    fn request_resume_approval_is_marker_only_and_stays_awaiting() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = awaiting_record("run_resume_appr");
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let now = Utc::now();
        let decision = store
            .request_resume_approval(&rec.id, "matt", None, now)
            .unwrap();
        // The decision reports the still-present awaited step id.
        assert_eq!(
            decision,
            ApprovalDecision::Approved {
                run_id: rec.id.clone(),
                step_id: "deploy".into(),
            }
        );

        let reloaded = store.load(&rec.id).unwrap();
        // Marker-only: the run stays AwaitingApproval with all pause
        // fields intact; only resume_requested_at is added.
        assert_eq!(reloaded.status, RunStatus::AwaitingApproval);
        assert_eq!(reloaded.awaiting_step_id.as_deref(), Some("deploy"));
        assert!(reloaded.approval_prompt.is_some());
        assert!(reloaded.awaiting_since.is_some());
        assert_eq!(reloaded.resume_requested_at, Some(now));
    }

    #[test]
    fn request_resume_approval_on_non_awaiting_run_errors() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let mut rec = sample_record("run_resume_running");
        rec.status = RunStatus::Running;
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let err = store
            .request_resume_approval(&rec.id, "matt", None, Utc::now())
            .unwrap_err();
        assert!(matches!(err, ApprovalError::NotAwaiting(_)));
    }

    #[test]
    fn list_pending_resume_filters_on_marker_and_lease() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();

        // No marker, awaiting → never listed (marker absent).
        let mut plain = sample_record("run_plain");
        plain.status = RunStatus::AwaitingApproval;
        store.create(plain, SAMPLE_YAML).unwrap();

        // Awaiting + marked, never claimed → listed.
        let mut marked = sample_record("run_marked");
        marked.status = RunStatus::AwaitingApproval;
        marked.resume_requested_at = Some(now);
        store.create(marked, SAMPLE_YAML).unwrap();

        // Awaiting + marked + freshly claimed (within TTL) → excluded.
        let mut fresh = sample_record("run_fresh_claim");
        fresh.status = RunStatus::AwaitingApproval;
        fresh.resume_requested_at = Some(now);
        fresh.resume_claimed_at = Some(now);
        fresh.resume_claimed_by = Some("w1".into());
        store.create(fresh, SAMPLE_YAML).unwrap();

        // Marked but already Rejected (reject-after-approve race): the
        // stale marker must NOT cause it to be picked up for resume.
        let mut rejected = sample_record("run_rejected_marker");
        rejected.status = RunStatus::Rejected;
        rejected.resume_requested_at = Some(now);
        store.create(rejected, SAMPLE_YAML).unwrap();

        let pending = store.list_pending_resume(now).unwrap();
        let ids: Vec<&str> = pending.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["run_marked"]);

        // Advance well past the lease → the stale claim is re-included,
        // but the Rejected run is still excluded (status gates it out).
        let later = now + RunStore::RESUME_LEASE + chrono::Duration::seconds(1);
        let pending_later = store.list_pending_resume(later).unwrap();
        let mut ids_later: Vec<&str> = pending_later.iter().map(|r| r.id.as_str()).collect();
        ids_later.sort();
        assert_eq!(ids_later, vec!["run_fresh_claim", "run_marked"]);
    }

    #[test]
    fn claim_resume_respects_live_lease_then_reclaims_after_ttl() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();
        let mut rec = sample_record("run_claim");
        rec.resume_requested_at = Some(now);
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        // First claim succeeds.
        assert!(store.claim_resume(&rec.id, "w1", now).unwrap());
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.resume_claimed_by.as_deref(), Some("w1"));

        // Second claim within the lease window is refused.
        let within = now + chrono::Duration::seconds(10);
        assert!(!store.claim_resume(&rec.id, "w2", within).unwrap());

        // After the lease elapses, the run is reclaimable.
        let after = now + RunStore::RESUME_LEASE + chrono::Duration::seconds(1);
        assert!(store.claim_resume(&rec.id, "w2", after).unwrap());
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.resume_claimed_by.as_deref(), Some("w2"));
    }

    #[test]
    fn clear_resume_drops_marker_and_claim() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();
        let mut rec = sample_record("run_clear");
        rec.resume_requested_at = Some(now);
        rec.resume_claimed_at = Some(now);
        rec.resume_claimed_by = Some("w1".into());
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        store.clear_resume(&rec.id, now).unwrap();
        let reloaded = store.load(&rec.id).unwrap();
        assert!(reloaded.resume_requested_at.is_none());
        assert!(reloaded.resume_claimed_at.is_none());
        assert!(reloaded.resume_claimed_by.is_none());
    }

    #[test]
    fn clear_resume_nulls_resume_mode() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();
        let mut rec = sample_record("run_clear_mode");
        rec.resume_requested_at = Some(now);
        rec.resume_claimed_at = Some(now);
        rec.resume_claimed_by = Some("w1".into());
        rec.resume_mode = Some("bypass".into());
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        store.clear_resume(&rec.id, now).unwrap();
        let reloaded = store.load(&rec.id).unwrap();
        assert!(reloaded.resume_mode.is_none());
        assert!(reloaded.resume_requested_at.is_none());
        assert!(reloaded.resume_claimed_at.is_none());
        assert!(reloaded.resume_claimed_by.is_none());
    }

    #[test]
    fn request_resume_approval_records_valid_mode() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = awaiting_record("run_resume_mode");
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        store
            .request_resume_approval(&rec.id, "matt", Some("bypass"), Utc::now())
            .unwrap();

        let reloaded = store.load(&rec.id).unwrap();
        // Marker set, mode stored, run stays paused.
        assert_eq!(reloaded.resume_mode.as_deref(), Some("bypass"));
        assert!(reloaded.resume_requested_at.is_some());
        assert_eq!(reloaded.status, RunStatus::AwaitingApproval);
    }

    #[test]
    fn request_resume_approval_drops_invalid_mode() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = awaiting_record("run_resume_bad_mode");
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        store
            .request_resume_approval(&rec.id, "matt", Some("turbo"), Utc::now())
            .unwrap();

        let reloaded = store.load(&rec.id).unwrap();
        assert!(reloaded.resume_mode.is_none());
        assert!(reloaded.resume_requested_at.is_some());
    }

    #[test]
    fn cancel_running_marks_cancelled_and_clears_fields() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();
        let mut rec = sample_record("run_cancel_running");
        rec.status = RunStatus::Running;
        // No live pid: non-live path → was_running false, status Cancelled.
        rec.runner_pid = None;
        rec.active_step_id = Some("step_a".into());
        rec.active_step_agent = Some("agent_a".into());
        rec.awaiting_step_id = Some("deploy".into());
        rec.approval_prompt = Some("ok?".into());
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let outcome = store.cancel(&rec.id, "matt", "stop it", now).unwrap();
        assert_eq!(
            outcome,
            CancelOutcome::MarkedCancelled {
                pid: None,
                was_running: false,
            }
        );

        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::Cancelled);
        assert!(reloaded.status.is_terminal());
        assert_eq!(reloaded.finished_at, Some(now));
        assert_eq!(reloaded.error_message.as_deref(), Some("stop it"));
        assert!(reloaded.runner_pid.is_none());
        assert!(reloaded.active_step_id.is_none());
        assert!(reloaded.active_step_agent.is_none());
        assert!(reloaded.awaiting_step_id.is_none());
        assert!(reloaded.approval_prompt.is_none());
    }

    #[test]
    fn cancel_running_does_not_signal_self() {
        // Regression: a web-approved gate is resumed in-process inside
        // `cp serve`, so the run's `runner_pid` can be the cp-serve PID
        // (== this process). Cancel must mark it `Cancelled` WITHOUT
        // SIGTERMing itself — otherwise it would kill the control plane.
        // Proof: if the guard were missing, SIGTERM to this test process
        // would terminate the test run; instead the assertions below run.
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let now = Utc::now();
        let mut rec = sample_record("run_cancel_self_pid");
        rec.status = RunStatus::Running;
        rec.runner_pid = Some(std::process::id());
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let outcome = store.cancel(&rec.id, "matt", "stop it", now).unwrap();
        // The self pid is live, so `was_running` reflects that; the
        // `pid` echoed back is the recorded runner_pid (pre-clear).
        assert_eq!(
            outcome,
            CancelOutcome::MarkedCancelled {
                pid: Some(std::process::id()),
                was_running: true,
            }
        );

        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::Cancelled);
        assert!(reloaded.runner_pid.is_none());

        // Critically: we reached this line, which means the test process
        // was NOT terminated by a self-directed SIGTERM.
    }

    #[test]
    fn cancel_awaiting_approval_rejects() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = awaiting_record("run_cancel_awaiting");
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let outcome = store
            .cancel(&rec.id, "matt", "not now", Utc::now())
            .unwrap();
        assert_eq!(outcome, CancelOutcome::RejectedAwaitingApproval);

        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::Rejected);
        assert!(reloaded.error_message.unwrap().contains("not now"));
    }

    #[test]
    fn cancel_terminal_run_errors() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let mut rec = sample_record("run_cancel_terminal");
        rec.status = RunStatus::Completed;
        rec.finished_at = Some(Utc::now());
        store.create(rec.clone(), SAMPLE_YAML).unwrap();

        let err = store
            .cancel(&rec.id, "matt", "too late", Utc::now())
            .unwrap_err();
        assert!(matches!(
            err,
            CancelError::AlreadyTerminal(RunStatus::Completed)
        ));
    }

    #[test]
    fn archive_moves_run_out_of_list_and_restore_brings_it_back() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut rec = sample_record("run_01ARCHIVE");
        rec.status = RunStatus::Completed;
        store.create(rec.clone(), "x").unwrap();
        assert_eq!(store.list().unwrap().len(), 1);

        store.archive(&rec.id).unwrap();
        assert_eq!(
            store.list().unwrap().len(),
            0,
            "archived run leaves active list"
        );
        let archived = store.list_archived().unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, rec.id);
        // The archive dir is a sibling of the runs dir.
        assert!(tmp
            .path()
            .join("runs-archive")
            .join(&rec.id)
            .join("run.json")
            .is_file());

        store.restore(&rec.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.list_archived().unwrap().len(), 0);
    }

    #[test]
    fn archive_requires_terminal_status() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut rec = sample_record("run_01RUNNING");
        rec.status = RunStatus::Running;
        store.create(rec.clone(), "x").unwrap();
        match store.archive(&rec.id) {
            Err(RunStoreError::NotTerminal(_)) => {}
            other => panic!("expected NotTerminal, got {other:?}"),
        }
        // Still listed, untouched.
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn delete_removes_from_either_scope_and_then_404s() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut rec = sample_record("run_01DELETE");
        rec.status = RunStatus::Failed;
        store.create(rec.clone(), "x").unwrap();

        store.delete(&rec.id).unwrap();
        assert!(!tmp.path().join("runs").join(&rec.id).exists());
        match store.delete(&rec.id) {
            Err(RunStoreError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn archive_missing_run_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        match store.archive("run_NOPE") {
            Err(RunStoreError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
