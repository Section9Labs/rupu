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
    /// PR 2: id of the step the run is paused at, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_step_id: Option<String>,
    /// PR 2: rendered approval prompt the operator sees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_prompt: Option<String>,
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
    /// Per-unit records for fan-out steps. Empty for linear steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ItemResultRecord>,
    pub finished_at: DateTime<Utc>,
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
            items: sr.items.iter().map(ItemResultRecord::from).collect(),
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

#[derive(Debug, Error)]
pub enum RunStoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("run `{0}` not found")]
    NotFound(String),
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

    fn workflow_snapshot(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("workflow.yaml")
    }

    /// Create the run directory and persist initial `run.json` and
    /// the workflow YAML snapshot. Returns the created `RunRecord`.
    pub fn create(
        &self,
        record: RunRecord,
        workflow_yaml: &str,
    ) -> Result<RunRecord, RunStoreError> {
        let dir = self.run_dir(&record.id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(self.workflow_snapshot(&record.id), workflow_yaml)?;
        // Touch the step-results log so subsequent appends don't
        // need to create+open.
        File::create(self.step_results_log(&record.id))?;
        write_atomic(&self.run_json(&record.id), &serde_json::to_vec_pretty(&record)?)?;
        Ok(record)
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
    pub fn read_step_results(
        &self,
        run_id: &str,
    ) -> Result<Vec<StepResultRecord>, RunStoreError> {
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
            items: Vec::new(),
            finished_at: Utc::now(),
        }
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
    fn update_flips_status_and_persists() {
        let tmp = TempDir::new().unwrap();
        let store = RunStore::new(tmp.path().to_path_buf());
        let rec = sample_record("run_02");
        store.create(rec.clone(), "name: x\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n").unwrap();

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
        store.create(rec.clone(), "name: x\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n").unwrap();

        store.append_step_result(&rec.id, &sample_step_result("a")).unwrap();
        store.append_step_result(&rec.id, &sample_step_result("b")).unwrap();

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
}
