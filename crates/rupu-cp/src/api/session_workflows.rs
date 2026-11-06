//! Explicit operator workflow launches associated with a session. These records
//! are neither agent turns nor dispatch ancestry: the workflow owns its actual
//! events, messages and transcripts, and this session owns its durable link.
use crate::{
    error::{ApiError, ApiResult},
    launcher::{LaunchError, LaunchRequest, RunLauncher},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path as FsPath, PathBuf},
};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/sessions/:id/workflows", get(list))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub scope_kind: String,
    pub scope_id: Option<String>,
    pub revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Association {
    pub session_id: String,
    pub run_id: String,
    pub workspace_id: String,
    pub host_id: String,
    pub workflow_name: String,
    pub source: Source,
    pub launched_at: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAssociation {
    association: Association,
    monitor_pid: u32,
    monitor_start: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartupOutcome {
    run_id: String,
    status: String,
    error: String,
}
fn outcome_path(s: &AppState, run_id: &str) -> PathBuf {
    s.global_dir
        .join("runs")
        .join(run_id)
        .join("session-launch-result.json")
}

#[derive(Deserialize)]
struct SessionIdentity {
    session_id: String,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    workspace_path: PathBuf,
}

fn identity(dir: &FsPath, id: &str) -> ApiResult<SessionIdentity> {
    let record: SessionIdentity = serde_json::from_slice(
        &fs::read(dir.join("session.json")).map_err(|e| ApiError::internal(e.to_string()))?,
    )
    .map_err(|e| ApiError::internal(e.to_string()))?;
    if record.session_id != id {
        return Err(ApiError::conflict("session record identity mismatch"));
    }
    Ok(record)
}

/// Resolve an active session and compare both its registered project identity
/// and canonical execution path. Called before snapshot creation and again
/// before reserving the launch, so invalid attachments cannot start work.
pub(crate) fn validate_attachment(
    s: &AppState,
    id: &str,
    workspace_id: &str,
    working_dir: &FsPath,
) -> ApiResult<PathBuf> {
    super::runs::validate_id(id)?;
    super::sessions::active_session(s, id, "launch workflow")?;
    let dir = s.global_dir.join("sessions").join(id);
    let record = identity(&dir, id)?;
    let session_path = record
        .workspace_path
        .canonicalize()
        .map_err(|e| ApiError::conflict(format!("session workspace is unavailable: {e}")))?;
    let execution_path = working_dir
        .canonicalize()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if record.workspace_id != workspace_id || session_path != execution_path {
        return Err(ApiError::conflict(
            "workflow execution workspace does not match the session",
        ));
    }
    Ok(dir)
}

/// The association is durable before the adapter may spawn. A crash at this
/// boundary leaves a visible starting record; it never authorizes a retry.
/// Unknown spawn outcomes are reported to the caller, with the same ID retained.
pub(crate) async fn launch(
    s: &AppState,
    launcher: &dyn RunLauncher,
    session_id: &str,
    workspace_id: &str,
    source: Source,
    request: LaunchRequest,
) -> ApiResult<Association> {
    let dir = validate_attachment(
        s,
        session_id,
        workspace_id,
        FsPath::new(request.working_dir.as_deref().ok_or_else(|| {
            ApiError::bad_request("session workflow requires an execution workspace")
        })?),
    )?;
    let links = create_association_dir(&dir)?;
    let record = Association {
        session_id: session_id.into(),
        run_id: format!("run_{}", ulid::Ulid::new()),
        workspace_id: workspace_id.into(),
        host_id: "local".into(),
        workflow_name: request.workflow.clone(),
        source,
        launched_at: chrono::Utc::now().to_rfc3339(),
        status: "starting".into(),
        error: None,
    };
    let path = links.join(format!("{}.json", record.run_id));
    let mut stored = StoredAssociation {
        association: record,
        monitor_pid: std::process::id(),
        monitor_start: rupu_launch::proc::own_start_stamp(),
    };
    let run_dir = s.global_dir.join("runs").join(&stored.association.run_id);
    // Refuse a known tracking-storage failure before publishing a launch.
    fs::create_dir_all(&run_dir).map_err(|e| ApiError::internal(e.to_string()))?;
    save_record(&path, &stored, false)
        .and_then(|_| fs::File::open(&dir)?.sync_all())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let stderr = run_dir.join("session-launch.log");
    let (state, id, run_id, log) = (
        s.clone(),
        session_id.to_string(),
        stored.association.run_id.clone(),
        stderr.clone(),
    );
    let watch = crate::launcher::LaunchWatch {
        stderr,
        on_exit: std::sync::Arc::new(move |exit| {
            if let Err(error) = observe_exit(&state, &id, &run_id, exit, &log) {
                tracing::error!(%run_id, error = %error.1, "could not record session workflow process exit");
            }
        }),
    };
    match launcher
        .launch_identified(request, &stored.association.run_id, watch)
        .await
    {
        Ok(()) => Ok(stored.association),
        Err(error) => {
            stored.association.status = "launch_failed".into();
            stored.association.error = Some(error.to_string());
            let outcome = StartupOutcome {
                run_id: stored.association.run_id.clone(),
                status: "launch_failed".into(),
                error: error.to_string(),
            };
            if let Err(e) = save_record(&outcome_path(s, &outcome.run_id), &outcome, true) {
                tracing::error!(run_id = %outcome.run_id, error = %e, "could not record workflow spawn failure");
            }
            // A concurrent archive moves the link, so resolve its current
            // location. Never recreate an active session directory.
            let result = link_path(s, session_id, &stored.association.run_id).and_then(|path| {
                save_record(&path, &stored, true).map_err(|e| ApiError::internal(e.to_string()))
            });
            if let Err(e) = result {
                tracing::error!(run_id = %stored.association.run_id, error = %e.1, "could not persist workflow launch failure");
            }
            Err(match error {
                LaunchError::Invalid(message) => ApiError::bad_request(message),
                LaunchError::Spawn(message) => ApiError::internal(message),
            })
        }
    }
}

fn link_path(s: &AppState, id: &str, run_id: &str) -> ApiResult<PathBuf> {
    for scope in ["sessions", "sessions-archive"] {
        let dir = s.global_dir.join(scope).join(id);
        let path = dir.join("workflow-runs").join(format!("{run_id}.json"));
        if path.is_file() {
            identity(&dir, id)?;
            super::fs_safety::validate_within(&path, &dir)?;
            return Ok(path);
        }
    }
    Err(ApiError::not_found(
        "session workflow association is no longer present",
    ))
}

/// Exact process exit evidence, not a timeout. A canonical run record always
/// wins; this only diagnoses the startup gap before run.json exists.
pub(crate) fn observe_exit(
    s: &AppState,
    _id: &str,
    run_id: &str,
    exit: Result<std::process::ExitStatus, String>,
    stderr: &FsPath,
) -> ApiResult<()> {
    let run = s.run_store.load(run_id).or_else(|e| match e {
        rupu_orchestrator::RunStoreError::NotFound(_) => {
            rupu_orchestrator::RunStore::new(s.run_store.archive_root()).load(run_id)
        }
        other => Err(other),
    });
    match run {
        Ok(_) => return Ok(()),
        Err(rupu_orchestrator::RunStoreError::NotFound(_)) => {}
        Err(e) => return Err(ApiError::internal(e.to_string())),
    }
    let (status, detail) = match exit {
        Ok(status) => (
            "launch_failed",
            format!("workflow process exited ({status}) before recording its run"),
        ),
        Err(error) => (
            "unknown",
            format!("workflow process outcome could not be observed: {error}"),
        ),
    };
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(file) = fs::File::open(stderr) {
        let _ = file.take(4096).read_to_end(&mut bytes);
    }
    let stderr = String::from_utf8_lossy(&bytes);
    let outcome = StartupOutcome {
        run_id: run_id.into(),
        status: status.into(),
        error: if stderr.trim().is_empty() {
            detail
        } else {
            format!("{detail}: {}", stderr.trim())
        },
    };
    save_record(&outcome_path(s, run_id), &outcome, true)
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// Create only the child directory: an archive/delete racing the earlier
/// identity check must never recreate an empty active-session parent.
pub(crate) fn create_association_dir(dir: &FsPath) -> ApiResult<PathBuf> {
    let links = dir.join("workflow-runs");
    match fs::create_dir(&links) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(ApiError::internal(e.to_string())),
    }
    super::fs_safety::validate_within(&links, dir)?;
    Ok(links)
}

/// Publish only complete JSON. Initial hard-link publication refuses an ID
/// collision instead of overwriting another launch; replacements are atomic.
fn save_record(path: &FsPath, record: &impl Serialize, replace: bool) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", ulid::Ulid::new()));
    let result = (|| -> std::io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(record)?)?;
        file.sync_all()?;
        if replace {
            fs::rename(&temporary, path)?;
        } else {
            fs::hard_link(&temporary, path)?;
            fs::remove_file(&temporary)?;
        }
        fs::File::open(path.parent().expect("association has a session directory"))?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[derive(Default, Deserialize)]
struct HostQuery {
    host: Option<String>,
}

async fn list(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    super::fleet::require_local(q.host.as_deref(), "session workflow links")?;
    super::runs::blocking(move || list_records(&s, &id).map(Json)).await
}

pub(crate) fn list_records(s: &AppState, id: &str) -> ApiResult<serde_json::Value> {
    super::runs::validate_id(id)?;
    let active = s.global_dir.join("sessions").join(id);
    let archive = s.global_dir.join("sessions-archive").join(id);
    let dir = if active.join("session.json").is_file() {
        active
    } else if archive.join("session.json").is_file() {
        archive
    } else {
        return Err(ApiError::not_found("session not found"));
    };
    let owner = identity(&dir, id)?;
    let links = dir.join("workflow-runs");
    let mut records = Vec::new();
    if links.exists() {
        super::fs_safety::validate_within(&links, &dir)?;
        for entry in fs::read_dir(&links).map_err(|e| ApiError::internal(e.to_string()))? {
            let path = entry.map_err(|e| ApiError::internal(e.to_string()))?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            super::fs_safety::validate_within(&path, &links)?;
            let stored: StoredAssociation = serde_json::from_slice(
                &fs::read(&path).map_err(|e| ApiError::internal(e.to_string()))?,
            )
            .map_err(|e| ApiError::internal(e.to_string()))?;
            let mut record = stored.association;
            super::runs::validate_id(&record.run_id)?;
            if record.session_id != id
                || record.workspace_id != owner.workspace_id
                || record.host_id != "local"
                || path.file_stem().and_then(|v| v.to_str()) != Some(record.run_id.as_str())
            {
                return Err(ApiError::conflict("workflow association identity mismatch"));
            }
            let run = s.run_store.load(&record.run_id).or_else(|e| match e {
                rupu_orchestrator::RunStoreError::NotFound(_) => {
                    rupu_orchestrator::RunStore::new(s.run_store.archive_root())
                        .load(&record.run_id)
                }
                other => Err(other),
            });
            match run {
                Ok(run) => {
                    if run.id != record.run_id
                        || run.workspace_id != record.workspace_id
                        || run.workflow_name != record.workflow_name
                    {
                        return Err(ApiError::conflict(
                            "workflow run does not match its session association",
                        ));
                    }
                    record.status = run.status.as_str().to_string();
                    record.error = run.error_message;
                }
                Err(rupu_orchestrator::RunStoreError::NotFound(_)) => {
                    match fs::read(outcome_path(s, &record.run_id)) {
                        Ok(bytes) => {
                            let outcome: StartupOutcome = serde_json::from_slice(&bytes)
                                .map_err(|e| ApiError::internal(e.to_string()))?;
                            if outcome.run_id != record.run_id {
                                return Err(ApiError::conflict(
                                    "workflow startup result identity mismatch",
                                ));
                            }
                            record.status = outcome.status;
                            record.error = Some(outcome.error);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            if record.status == "starting"
                                && !rupu_launch::proc::process_alive(
                                    stored.monitor_pid,
                                    stored.monitor_start,
                                )
                            {
                                record.status = "unknown".into();
                                record.error = Some("launch monitor exited before workflow startup was recorded; inspect this run before launching again".into());
                            }
                        }
                        Err(e) => return Err(ApiError::internal(e.to_string())),
                    }
                }
                Err(e) => return Err(ApiError::internal(e.to_string())),
            }
            records.push(record);
        }
    }
    records.sort_by(|a, b| {
        a.launched_at
            .cmp(&b.launched_at)
            .then(a.run_id.cmp(&b.run_id))
    });
    Ok(serde_json::json!({"session_id":id,"host_id":"local","workflows":records}))
}
