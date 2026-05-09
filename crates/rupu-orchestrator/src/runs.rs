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
use rupu_runtime::RunEnvelope;
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
        }
    }

    /// True when no further state transitions are expected. Used by
    /// `rupu workflow runs` to bucket terminal vs in-flight rows.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Rejected)
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
        // Touch the step-results log so subsequent appends don't
        // need to create+open.
        File::create(self.step_results_log(&record.id))?;
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

    /// List every run currently on disk, newest-first by
    /// `started_at`. Malformed `run.json` files are skipped.
    pub fn list(&self) -> Result<Vec<RunRecord>, RunStoreError> {
        let mut out: Vec<RunRecord> = Vec::new();
        let rd = match std::fs::read_dir(&self.root) {
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

/// Outcome of an approve/reject library call. Returned to callers
/// (CLI text wrapper or TUI toast) so they decide how to display it.
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

impl RunStore {
    /// Library-level approve flow: load → expire-check → mutate
    /// status → persist. Caller is responsible for re-entering
    /// `run_workflow` (CLI does this via the existing path; TUI
    /// optimistically updates the local model and waits for the next
    /// RunUpdate from disk).
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
        ExecutionRequest, RepoBinding, RunContext, RunEnvelope, RunKind, RunTrigger,
        RunTriggerSource, WorkflowBinding,
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
                use_canvas: false,
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
}
