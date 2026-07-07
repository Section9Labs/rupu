//! Cross-store/host run resolution.
//!
//! `AppState.run_store` (state.rs:65) only ever reads `global_dir/runs`. Most
//! runs land there — but an **autoflow** run's artifacts can also live in a
//! **project-local** `.rupu/runs/` store or on a **remote host**, and a run
//! that failed before it ever wrote `run.json` (e.g. an invalid provider key
//! at dispatch) has *no* artifacts anywhere except the autoflow history +
//! claim record.
//!
//! [`resolve_run_location`] is the single place that answers "where does this
//! run's data live" on demand — it never mirrors/copies artifacts. Order:
//!
//! 1. The global `RunStore` (unchanged, fast path — a normal run resolves
//!    here exactly as it always has).
//! 2. Every registered project's local `.rupu/runs/` store (one representative
//!    workspace per distinct repo, via [`distinct_repo_workspaces`]).
//! 3. The autoflow-history reader ([`autoflow_run_context`]): if the history
//!    knows about this run_id, it either says "ran on host X" (`Host`) or, if
//!    no host is on record and no artifacts turned up in (1)/(2), the run is
//!    `Unpersisted` — synthesized from the cycle/claim's own status + failure.
//! 4. A bounded host-probe fallback for non-autoflow remote runs: fire
//!    `GET /api/runs/:id` at every *registered* host (never unbounded — the
//!    host set is whatever `rupu host add` produced) and take the first hit.
//! 5. `NotFound`.
//!
//! Read-only + fail-closed throughout: an unreadable directory, a malformed
//! JSON/TOML file, or an unreachable host is skipped (optionally logged),
//! never a panic.
//!
//! ## The `host_id` signal (forward-looking)
//!
//! Nothing in today's autoflow-history writer (`rupu-cli`'s
//! `autoflow_runtime.rs`) records *which host* ran a cycle — `AutoflowCycleRecord`
//! only carries a `worker_id`/`worker_name` (see
//! `rupu_runtime::autoflow_history`). Distributed/host-dispatched autoflow
//! doesn't exist yet. Rather than invent a fake correlation (e.g. guessing
//! from a worker hostname), this reader looks for an **optional** `host_id`
//! string directly on each history event's raw JSON — a field no current
//! writer emits, so on real `~/.rupu` data this is always `None` and every
//! run continues to resolve exactly as before. It exists so that a future
//! writer change (autoflow cycles dispatched to a registered rupu-cp host)
//! lights up the `Host` branch with zero reader changes. Until then it is
//! exercised only by this module's tests — mirrored deliberately on the
//! design spec's own "ProjectLocal is future-proofing" reasoning
//! (`docs/superpowers/specs/2026-07-07-rupu-autoflow-runs-firstclass-design.md`,
//! Open Questions).

use crate::{api::repo_scope::distinct_repo_workspaces, state::AppState};
use rupu_orchestrator::{runs::RunStore, RunStatus};
use rupu_runtime::{
    AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleRecord, AutoflowHistoryEventRecord,
};
use rupu_workspace::{AutoflowClaimRecord, AutoflowClaimStore, RepoRegistryStore, WorkspaceStore};
use std::path::{Path, PathBuf};

// ── Types ──────────────────────────────────────────────────────────────────

/// Where a run's artifacts live, resolved on demand. Never mirrors/copies
/// data — callers read/proxy from wherever this points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLocation {
    /// In `AppState.run_store` (`<global_dir>/runs/<id>`) — the fast, common
    /// path, unchanged from before this resolver existed.
    Global,
    /// In a registered project's own `.rupu/runs/<id>` store.
    ProjectLocal { path: PathBuf },
    /// On a registered remote rupu-cp host; proxy `GET`s to it.
    Host { host_id: String },
    /// The autoflow history knows about this run_id, but no artifacts exist
    /// anywhere (global, every project-local store, or any registered host):
    /// the run failed before/without ever persisting `run.json`. The caller
    /// synthesizes a run record from these fields instead of 404ing.
    Unpersisted {
        cycle_id: String,
        status: RunStatus,
        failure: String,
        workflow_name: String,
        entity: Option<String>,
    },
    /// Global + every project-local store + autoflow history + every
    /// registered host all missed.
    NotFound,
}

/// Read-only autoflow-history context for a run_id: which entity/cycle/claim
/// produced it, where it ran, and (for a failed dispatch) the failure cause.
///
/// Built from `<global_dir>/autoflows/history/{cycles,events}/**/*.json` +
/// `<global_dir>/autoflows/claims/<repo--issue>/claim.toml`. Used both by
/// [`resolve_run_location`] and (Task 2) the `GET /api/runs/:id/autoflow`
/// endpoint feeding the web Autoflow panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoflowRunContext {
    pub repo_ref: String,
    pub workspace_path: Option<PathBuf>,
    /// See the module doc's "`host_id` signal" section — `None` on all
    /// current real data; forward-compatible field.
    pub host_id: Option<String>,
    pub status: RunStatus,
    pub cycle_id: String,
    pub failure: Option<String>,
    pub workflow_name: String,
    pub entity: Option<String>,
}

// ── Resolver ───────────────────────────────────────────────────────────────

/// Resolve where `run_id`'s artifacts live. See the module doc for the full
/// order + rationale. Async because the bounded host-probe fallback makes
/// real network calls (`HostConnector::proxy_get_json`); every other step is
/// pure filesystem I/O.
pub async fn resolve_run_location(s: &AppState, run_id: &str) -> RunLocation {
    if s.run_store.load(run_id).is_ok() {
        return RunLocation::Global;
    }

    if let Some(path) = find_project_local(s, run_id) {
        return RunLocation::ProjectLocal { path };
    }

    if let Some(ctx) = autoflow_run_context(&s.global_dir, run_id) {
        if let Some(host_id) = ctx.host_id {
            return RunLocation::Host { host_id };
        }
        return RunLocation::Unpersisted {
            cycle_id: ctx.cycle_id,
            status: ctx.status,
            failure: ctx
                .failure
                .unwrap_or_else(|| "autoflow run failed; no failure detail recorded".to_string()),
            workflow_name: ctx.workflow_name,
            entity: ctx.entity,
        };
    }

    if let Some(host_id) = probe_hosts(s, run_id).await {
        return RunLocation::Host { host_id };
    }

    RunLocation::NotFound
}

fn workspace_store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

fn repo_store(s: &AppState) -> RepoRegistryStore {
    RepoRegistryStore {
        root: s.global_dir.join("repos"),
    }
}

/// Check every registered project (one representative workspace per distinct
/// repo — see [`distinct_repo_workspaces`]) for a local `.rupu/runs/<run_id>`.
/// Fail-closed: an unreadable workspace/repo store just yields no candidates.
fn find_project_local(s: &AppState, run_id: &str) -> Option<PathBuf> {
    let workspaces = workspace_store(s).list().unwrap_or_default();
    let repos = distinct_repo_workspaces(workspaces, &repo_store(s));
    for r in repos {
        let path = PathBuf::from(&r.workspace.path);
        let store = RunStore::new(path.join(".rupu").join("runs"));
        if store.load(run_id).is_ok() {
            return Some(path);
        }
    }
    None
}

/// Bounded fallback for a non-autoflow run whose artifacts live on a remote
/// host with no autoflow-history trail: fire `GET /api/runs/:id` at every
/// *registered* host (never unbounded — bounded by whatever `rupu host add`
/// produced) and take the first hit. `"local"` is skipped: the global +
/// project-local checks above already cover this machine.
async fn probe_hosts(s: &AppState, run_id: &str) -> Option<String> {
    for host in s.hosts.list_hosts() {
        if host.id == "local" {
            continue;
        }
        let Ok(conn) = s.hosts.resolve(&host.id) else {
            continue;
        };
        if conn
            .proxy_get_json(&format!("/api/runs/{run_id}"))
            .await
            .is_ok()
        {
            return Some(host.id);
        }
    }
    None
}

// ── Autoflow-history reader ────────────────────────────────────────────────

/// Look up `run_id` in the autoflow history + claim records. Returns `None`
/// when the history has no record of this run (a plain non-autoflow run, or
/// history genuinely missing/unreadable) — never panics.
pub fn autoflow_run_context(global_dir: &Path, run_id: &str) -> Option<AutoflowRunContext> {
    let history_root = global_dir.join("autoflows").join("history");
    let claims = AutoflowClaimStore {
        root: global_dir.join("autoflows").join("claims"),
    }
    .list()
    .unwrap_or_default();

    // 1. Saved cycle records (the common case — a cycle that ran to
    //    completion, successfully or not).
    for path in scan_json_files(&history_root.join("cycles")) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(cycle) = serde_json::from_slice::<AutoflowCycleRecord>(&bytes) else {
            tracing::warn!(path = %path.display(), "autoflow history: skipping unparseable cycle file");
            continue;
        };
        let raw: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        if let Some((idx, event)) = cycle
            .events
            .iter()
            .enumerate()
            .find(|(_, e)| e.run_id.as_deref() == Some(run_id))
        {
            let host_id = event_host_id(&raw, idx);
            return Some(build_context(&cycle, event, host_id, &claims, run_id));
        }
    }

    // 2. Loose event files not (yet) consolidated into a saved cycle record
    //    (e.g. a live cycle recorder that appended events before a crash).
    for path in scan_json_files(&history_root.join("events")) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<AutoflowHistoryEventRecord>(&bytes) else {
            tracing::warn!(path = %path.display(), "autoflow history: skipping unparseable event file");
            continue;
        };
        if record.event.run_id.as_deref() != Some(run_id) {
            continue;
        }
        let raw: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        let host_id = raw
            .get("host_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let synthetic_cycle = synthetic_cycle_from_event(&record);
        return Some(build_context(
            &synthetic_cycle,
            &record.event,
            host_id,
            &claims,
            run_id,
        ));
    }

    None
}

/// All prior cycles that touched `issue_ref`, newest first. Read-only,
/// fail-closed (an unreadable history dir yields an empty list). Feeds the
/// (Task 2) `/api/runs/:id/autoflow` endpoint's "prior cycles" list.
pub fn entity_cycles(global_dir: &Path, issue_ref: &str) -> Vec<AutoflowCycleRecord> {
    let cycles_dir = global_dir.join("autoflows").join("history").join("cycles");
    let mut out: Vec<AutoflowCycleRecord> = scan_json_files(&cycles_dir)
        .into_iter()
        .filter_map(|path| std::fs::read(&path).ok())
        .filter_map(|bytes| serde_json::from_slice::<AutoflowCycleRecord>(&bytes).ok())
        .filter(|cycle| {
            cycle
                .events
                .iter()
                .any(|e| e.issue_ref.as_deref() == Some(issue_ref))
        })
        .collect();
    out.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| b.cycle_id.cmp(&a.cycle_id))
    });
    out
}

/// List every `*.json` file under `<root>/<day>/` (the cycles/events history
/// layout is always one date-named subdirectory per day). Fail-closed: a
/// missing/unreadable root or day dir contributes nothing rather than erroring.
fn scan_json_files(root: &Path) -> Vec<PathBuf> {
    let Ok(days) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for day in days.flatten() {
        if !day.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(day.path()) else {
            continue;
        };
        for file in files.flatten() {
            if file.path().extension().and_then(|e| e.to_str()) == Some("json") {
                out.push(file.path());
            }
        }
    }
    out
}

/// Peek `events[index].host_id` on the raw (untyped) JSON — see the module
/// doc's "`host_id` signal" section for why this isn't a typed field on
/// [`AutoflowCycleEvent`].
fn event_host_id(raw: &serde_json::Value, index: usize) -> Option<String> {
    raw.get("events")?
        .get(index)?
        .get("host_id")?
        .as_str()
        .map(str::to_string)
}

/// Wrap a loose [`AutoflowHistoryEventRecord`] in a single-event
/// [`AutoflowCycleRecord`] shell so [`build_context`] has one code path
/// regardless of whether the match came from a saved cycle or a loose event.
fn synthetic_cycle_from_event(record: &AutoflowHistoryEventRecord) -> AutoflowCycleRecord {
    AutoflowCycleRecord {
        version: AutoflowCycleRecord::VERSION,
        cycle_id: record.cycle_id.clone(),
        mode: record.mode,
        worker_id: record.worker_id.clone(),
        worker_name: record.worker_name.clone(),
        repo_filter: record.repo_filter.clone(),
        started_at: record.at.clone(),
        finished_at: record.at.clone(),
        workflow_count: 0,
        polled_event_count: 0,
        webhook_event_count: 0,
        ran_cycles: 0,
        skipped_cycles: 0,
        failed_cycles: 0,
        cleaned_claims: 0,
        events: vec![record.event.clone()],
    }
}

/// Build the context from a matched cycle + its `run_id`-bearing event.
///
/// Failure precedence: the matched event's own detail if it's itself a
/// `CycleFailed` event; else a `CycleFailed` sibling in the same cycle for
/// the same run/issue; else the claim's `last_error` (matched by
/// `last_run_id`). `workspace_path` comes from the claim's `worktree_path`.
fn build_context(
    cycle: &AutoflowCycleRecord,
    matched: &AutoflowCycleEvent,
    host_id: Option<String>,
    claims: &[AutoflowClaimRecord],
    run_id: &str,
) -> AutoflowRunContext {
    let claim = claims
        .iter()
        .find(|c| c.last_run_id.as_deref() == Some(run_id));

    let failure = if matched.kind == AutoflowCycleEventKind::CycleFailed {
        matched.detail.clone()
    } else {
        cycle
            .events
            .iter()
            .find(|e| {
                e.kind == AutoflowCycleEventKind::CycleFailed
                    && (e.run_id.as_deref() == Some(run_id) || e.issue_ref == matched.issue_ref)
            })
            .and_then(|e| e.detail.clone())
    }
    .or_else(|| claim.and_then(|c| c.last_error.clone()));

    let status = matched
        .status
        .as_deref()
        .map(map_claim_status_str)
        .unwrap_or(RunStatus::Failed);

    AutoflowRunContext {
        repo_ref: matched.repo_ref.clone().unwrap_or_default(),
        workspace_path: claim
            .and_then(|c| c.worktree_path.clone())
            .map(PathBuf::from),
        host_id,
        status,
        cycle_id: cycle.cycle_id.clone(),
        failure,
        workflow_name: matched.workflow.clone().unwrap_or_default(),
        entity: matched
            .issue_display_ref
            .clone()
            .or_else(|| matched.issue_ref.clone()),
    }
}

/// Map a claim-status string (as written by `claim_status_name` in
/// `rupu-cli`'s `autoflow_runtime.rs` — `"eligible"`, `"claimed"`,
/// `"running"`, `"await_human"`, `"await_external"`, `"retry_backoff"`,
/// `"blocked"`, `"complete"`, `"released"`) onto the run-level [`RunStatus`]
/// the CP already renders. Used only in the [`RunLocation::Unpersisted`] /
/// [`AutoflowRunContext`] synthesis path — there's no live run to ask.
fn map_claim_status_str(status: &str) -> RunStatus {
    match status {
        "complete" => RunStatus::Completed,
        "await_human" | "await_external" => RunStatus::AwaitingApproval,
        "eligible" | "claimed" | "running" | "retry_backoff" => RunStatus::Running,
        // "blocked", "released", and any unrecognized value: this branch only
        // runs when no artifacts exist anywhere for the run, which in
        // practice means dispatch failed — render it as failed rather than
        // silently defaulting to pending/running.
        _ => RunStatus::Failed,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::RunRecord;
    use rupu_runtime::AutoflowCycleMode;
    use rupu_workspace::ClaimStatus;
    use std::collections::BTreeMap;

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf())
    }

    fn run_record(id: &str, workflow_name: &str, status: RunStatus) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: workflow_name.into(),
            status,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: PathBuf::from("/tmp/proj"),
            transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
            started_at: chrono::Utc::now(),
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

    /// Register a workspace record `<global_dir>/workspaces/<id>.toml` whose
    /// `path` points at `project_root` (mirrors `api::autoflows`'s test helper).
    fn register_workspace(tmp: &tempfile::TempDir, id: &str, project_root: &Path) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                project_root.display()
            ),
        )
        .unwrap();
    }

    fn write_cycle(tmp: &tempfile::TempDir, day: &str, cycle: &AutoflowCycleRecord) {
        let dir = tmp
            .path()
            .join("autoflows")
            .join("history")
            .join("cycles")
            .join(day);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.json", cycle.cycle_id)),
            serde_json::to_vec_pretty(cycle).unwrap(),
        )
        .unwrap();
    }

    /// Same as `write_cycle`, but lets the test inject a raw `host_id` field
    /// onto one event that the typed `AutoflowCycleEvent` doesn't carry (see
    /// the module doc's "`host_id` signal" section).
    fn write_cycle_with_host_id(
        tmp: &tempfile::TempDir,
        day: &str,
        cycle: &AutoflowCycleRecord,
        event_index: usize,
        host_id: &str,
    ) {
        let dir = tmp
            .path()
            .join("autoflows")
            .join("history")
            .join("cycles")
            .join(day);
        std::fs::create_dir_all(&dir).unwrap();
        let mut value = serde_json::to_value(cycle).unwrap();
        value["events"][event_index]["host_id"] = serde_json::Value::String(host_id.to_string());
        std::fs::write(
            dir.join(format!("{}.json", cycle.cycle_id)),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    fn base_cycle(cycle_id: &str) -> AutoflowCycleRecord {
        AutoflowCycleRecord {
            version: AutoflowCycleRecord::VERSION,
            cycle_id: cycle_id.into(),
            mode: AutoflowCycleMode::Tick,
            worker_id: Some("worker_local".into()),
            worker_name: Some("local".into()),
            repo_filter: None,
            started_at: "2026-07-01T10:00:00Z".into(),
            finished_at: "2026-07-01T10:00:05Z".into(),
            workflow_count: 1,
            polled_event_count: 0,
            webhook_event_count: 0,
            ran_cycles: 1,
            skipped_cycles: 0,
            failed_cycles: 0,
            cleaned_claims: 0,
            events: Vec::new(),
        }
    }

    fn run_launched_event(run_id: &str, status: &str) -> AutoflowCycleEvent {
        AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::RunLaunched,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            issue_display_ref: Some("42".into()),
            repo_ref: Some("github:Section9Labs/rupu".into()),
            source_ref: None,
            workflow: Some("issue-supervisor-dispatch".into()),
            run_id: Some(run_id.into()),
            wake_id: None,
            wake_event_id: None,
            status: Some(status.into()),
            detail: None,
        }
    }

    // ── resolve_run_location ────────────────────────────────────────────

    #[tokio::test]
    async fn resolves_global_when_run_json_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(
                run_record("run_global_1", "wf", RunStatus::Completed),
                "name: wf\nsteps: []\n",
            )
            .unwrap();

        let loc = resolve_run_location(&s, "run_global_1").await;
        assert_eq!(loc, RunLocation::Global);
    }

    #[tokio::test]
    async fn resolves_unpersisted_from_history_when_no_run_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let mut cycle = base_cycle("afc_001");
        cycle
            .events
            .push(run_launched_event("run_01KWYZ2QY4", "blocked"));
        cycle.events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::CycleFailed,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            repo_ref: Some("github:Section9Labs/rupu".into()),
            workflow: Some("issue-supervisor-dispatch".into()),
            detail: Some("401 invalid x-api-key".into()),
            ..AutoflowCycleEvent::default()
        });
        write_cycle(&tmp, "2026-07-01", &cycle);

        let loc = resolve_run_location(&s, "run_01KWYZ2QY4").await;
        match loc {
            RunLocation::Unpersisted {
                cycle_id,
                status,
                failure,
                workflow_name,
                entity,
            } => {
                assert_eq!(cycle_id, "afc_001");
                assert_eq!(status, RunStatus::Failed);
                assert_eq!(failure, "401 invalid x-api-key");
                assert_eq!(workflow_name, "issue-supervisor-dispatch");
                assert_eq!(entity.as_deref(), Some("42"));
            }
            other => panic!("expected Unpersisted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolves_unpersisted_failure_from_claim_last_error_when_no_cycle_failed_event() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let mut cycle = base_cycle("afc_002");
        cycle
            .events
            .push(run_launched_event("run_claim_err", "blocked"));
        write_cycle(&tmp, "2026-07-02", &cycle);

        let claims = AutoflowClaimStore {
            root: tmp.path().join("autoflows").join("claims"),
        };
        claims
            .save(&AutoflowClaimRecord {
                issue_ref: "github:Section9Labs/rupu/issues/42".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: Some("42".into()),
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::Blocked,
                worktree_path: Some("/home/matt/.rupu/autoflows/worktrees/rupu/42".into()),
                branch: None,
                last_run_id: Some("run_claim_err".into()),
                last_error: Some("401 invalid x-api-key (from claim)".into()),
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: None,
                pending_dispatch: None,
                contenders: vec![],
                updated_at: "2026-07-02T10:00:05Z".into(),
            })
            .unwrap();

        let loc = resolve_run_location(&s, "run_claim_err").await;
        match loc {
            RunLocation::Unpersisted { failure, .. } => {
                assert_eq!(failure, "401 invalid x-api-key (from claim)");
            }
            other => panic!("expected Unpersisted, got {other:?}"),
        }

        let ctx = autoflow_run_context(tmp.path(), "run_claim_err").unwrap();
        assert_eq!(
            ctx.workspace_path,
            Some(PathBuf::from(
                "/home/matt/.rupu/autoflows/worktrees/rupu/42"
            ))
        );
    }

    #[tokio::test]
    async fn resolves_project_local_when_run_in_project_store() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let proj = tempfile::TempDir::new().unwrap();
        let proj_run_store = RunStore::new(proj.path().join(".rupu").join("runs"));
        proj_run_store
            .create(
                run_record("run_proj_1", "wf", RunStatus::Completed),
                "name: wf\nsteps: []\n",
            )
            .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let loc = resolve_run_location(&s, "run_proj_1").await;
        assert_eq!(
            loc,
            RunLocation::ProjectLocal {
                path: proj.path().to_path_buf()
            }
        );
    }

    #[tokio::test]
    async fn resolves_host_when_history_records_remote_host() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let mut cycle = base_cycle("afc_003");
        cycle
            .events
            .push(run_launched_event("run_on_remote", "running"));
        write_cycle_with_host_id(&tmp, "2026-07-03", &cycle, 0, "host_worker_1");

        let loc = resolve_run_location(&s, "run_on_remote").await;
        assert_eq!(
            loc,
            RunLocation::Host {
                host_id: "host_worker_1".into()
            }
        );
    }

    #[tokio::test]
    async fn not_found_when_nowhere() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let loc = resolve_run_location(&s, "run_does_not_exist").await;
        assert_eq!(loc, RunLocation::NotFound);
    }

    // ── autoflow_run_context / entity_cycles ───────────────────────────

    #[test]
    fn autoflow_run_context_none_when_history_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(autoflow_run_context(tmp.path(), "run_x").is_none());
    }

    #[test]
    fn autoflow_run_context_finds_loose_event_not_yet_in_a_saved_cycle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cycle = base_cycle("afc_004");
        let event = run_launched_event("run_loose", "running");
        let record = AutoflowHistoryEventRecord::from_cycle_event(
            &cycle,
            event,
            chrono::DateTime::parse_from_rfc3339("2026-07-04T10:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        let dir = tmp
            .path()
            .join("autoflows")
            .join("history")
            .join("events")
            .join("2026-07-04");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.json", record.event_id)),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();

        let ctx = autoflow_run_context(tmp.path(), "run_loose").unwrap();
        assert_eq!(ctx.cycle_id, "afc_004");
        assert_eq!(ctx.workflow_name, "issue-supervisor-dispatch");
        assert_eq!(ctx.entity.as_deref(), Some("42"));
    }

    #[test]
    fn entity_cycles_filters_by_issue_ref_newest_first() {
        let tmp = tempfile::TempDir::new().unwrap();

        let mut older = base_cycle("afc_older");
        older.started_at = "2026-07-01T10:00:00Z".into();
        older.events.push(run_launched_event("run_a", "running"));
        write_cycle(&tmp, "2026-07-01", &older);

        let mut newer = base_cycle("afc_newer");
        newer.started_at = "2026-07-05T10:00:00Z".into();
        newer.events.push(run_launched_event("run_b", "complete"));
        write_cycle(&tmp, "2026-07-05", &newer);

        let mut unrelated = base_cycle("afc_unrelated");
        unrelated.events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::RunLaunched,
            issue_ref: Some("github:Section9Labs/rupu/issues/99".into()),
            run_id: Some("run_c".into()),
            ..AutoflowCycleEvent::default()
        });
        write_cycle(&tmp, "2026-07-06", &unrelated);

        let cycles = entity_cycles(tmp.path(), "github:Section9Labs/rupu/issues/42");
        assert_eq!(cycles.len(), 2);
        assert_eq!(cycles[0].cycle_id, "afc_newer");
        assert_eq!(cycles[1].cycle_id, "afc_older");
    }

    #[test]
    fn map_claim_status_str_covers_known_states() {
        assert_eq!(map_claim_status_str("complete"), RunStatus::Completed);
        assert_eq!(
            map_claim_status_str("await_human"),
            RunStatus::AwaitingApproval
        );
        assert_eq!(map_claim_status_str("running"), RunStatus::Running);
        assert_eq!(map_claim_status_str("blocked"), RunStatus::Failed);
        assert_eq!(map_claim_status_str("unknown-state"), RunStatus::Failed);
    }
}
