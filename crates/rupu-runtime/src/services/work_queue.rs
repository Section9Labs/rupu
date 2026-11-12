//! Exact execution-bound Queue access. The lifecycle lease is always outermost:
//! retention -> message space -> SQLite. No model input can select an owner.
use crate::{
    assembly::{LaunchSpec, Origin},
    retention_lease::RetentionLease,
};
use rupu_agent::{Cadence, Injection, InjectionKind, TurnCollector, TurnContext};
use rupu_fleet::{work_queue::*, Bus, FleetError};
use rupu_tools::work_queue::{WorkOwnerBinding, WorkOwnerKind, WorkQueuePort};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

fn refused(message: impl Into<String>) -> FleetError {
    FleetError::Refused(message.into())
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn under_configured_root(path: &Path, configured: &Path, canonical: &Path) -> PathBuf {
    path.strip_prefix(configured)
        .map(|relative| canonical.join(relative))
        .unwrap_or_else(|_| path.to_path_buf())
}
fn relative(path: &Path, root: &Path) -> Option<String> {
    let path = path.strip_prefix(root).ok()?;
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    path.to_str().map(str::to_owned)
}
/// Reject every existing linked component; a not-yet-created recording tail is
/// permitted because assembly precedes its writer. Owner directories must exist.
fn regular_path(path: &Path, allow_missing: bool) -> Result<(), FleetError> {
    if !path.is_absolute() {
        return Err(refused("Queue path must be absolute"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        if !matches!(part, Component::RootDir | Component::Normal(_)) {
            return Err(refused("Queue path has non-normal components"));
        }
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(m) if m.file_type().is_symlink() || (!m.is_dir() && !m.is_file()) => {
                return Err(refused("Queue path contains a link or special file"))
            }
            Ok(_) => (),
            Err(e) if allow_missing && e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(refused(format!("Queue owner unavailable: {e}"))),
        }
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}
#[cfg(unix)]
fn directory_identity(path: &Path) -> Result<DirectoryIdentity, FleetError> {
    use std::os::unix::fs::MetadataExt;
    regular_path(path, false)?;
    let m = fs::symlink_metadata(path).map_err(|e| refused(e.to_string()))?;
    if !m.is_dir() {
        return Err(refused("Queue owner is not a directory"));
    }
    Ok(DirectoryIdentity {
        device: m.dev(),
        inode: m.ino(),
    })
}
#[cfg(not(unix))]
fn directory_identity(_: &Path) -> Result<DirectoryIdentity, FleetError> {
    Err(refused(
        "execution-bound Queue requires Unix lifecycle guards",
    ))
}

// Match the real owner record schemas. Mutable status/history do not identify
// the owner: atomic session saves must not invalidate a still-current execution.
#[derive(serde::Deserialize)]
struct SessionOwnerRecord {
    session_id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    workspace_id: String,
    workspace_path: PathBuf,
    transcripts_dir: PathBuf,
    agent_name: String,
    status: crate::sessions::SessionStatus,
}
#[derive(serde::Deserialize)]
struct RunOwnerRecord {
    id: String,
    started_at: chrono::DateTime<chrono::Utc>,
    status: String,
    name: Option<String>,
    workflow_name: Option<String>,
    workspace_id: Option<String>,
    workspace_path: Option<PathBuf>,
}
struct OwnerRecord {
    identity: serde_json::Value,
    status: String,
    workspace_id: Option<String>,
}
fn owner_record(owner: &WorkOwnerBinding) -> Result<Option<OwnerRecord>, FleetError> {
    let file = match owner.kind {
        WorkOwnerKind::Session => "session.json",
        WorkOwnerKind::Agentiflow => "agentiflow.json",
        _ => "run.json",
    };
    let path = owner.directory.join(file);
    match fs::symlink_metadata(&path) {
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && owner.kind == WorkOwnerKind::Standalone =>
        {
            return Ok(None)
        }
        Err(e) => return Err(refused(format!("Queue owner record unavailable: {e}"))),
        Ok(_) => (),
    }
    regular_path(&path, false)?;
    let f = fs::File::open(path).map_err(|e| refused(e.to_string()))?;
    // Stream projection parsing: message_history is skipped, not materialized.
    if f.metadata().map_err(|e| refused(e.to_string()))?.len() > 64 * 1024 * 1024 {
        return Err(refused("Queue owner record exceeds 64 MiB bound"));
    }
    let limited = f.take(64 * 1024 * 1024 + 1);
    let record = if owner.kind == WorkOwnerKind::Session {
        let r: SessionOwnerRecord = serde_json::from_reader(limited)
            .map_err(|_| refused("session Queue owner record is malformed or exceeds bound"))?;
        if r.session_id != owner.id {
            return Err(refused("session Queue owner identity differs"));
        }
        OwnerRecord {
            identity: serde_json::json!([
                r.session_id,
                r.created_at,
                r.workspace_id,
                r.workspace_path,
                r.transcripts_dir,
                r.agent_name
            ]),
            status: r.status.as_str().into(),
            workspace_id: Some(r.workspace_id),
        }
    } else {
        let r: RunOwnerRecord = serde_json::from_reader(limited)
            .map_err(|_| refused("run Queue owner record is malformed or exceeds bound"))?;
        if r.id != owner.id {
            return Err(refused("run Queue owner identity differs"));
        }
        OwnerRecord {
            identity: serde_json::json!([
                r.id,
                r.started_at,
                r.name,
                r.workflow_name,
                r.workspace_id,
                r.workspace_path
            ]),
            status: r.status,
            workspace_id: r.workspace_id,
        }
    };
    Ok(Some(record))
}

struct RuntimeQueue {
    owner: WorkOwnerBinding,
    configured_global: PathBuf,
    owner_identity: DirectoryIdentity,
    record_identity: Option<serde_json::Value>,
    global_identity: DirectoryIdentity,
    bus: Arc<Bus>,
    actor: WorkActor,
    active: AtomicBool,
}
impl RuntimeQueue {
    fn owner_current(&self) -> Result<(), FleetError> {
        if self
            .configured_global
            .canonicalize()
            .map_err(|e| refused(e.to_string()))?
            != self.owner.global
        {
            return Err(refused("configured Queue root changed"));
        }
        if directory_identity(&self.owner.global)? != self.global_identity
            || directory_identity(&self.owner.directory)? != self.owner_identity
        {
            return Err(refused("Queue owner directory changed"));
        }
        regular_path(&self.owner.message_space, true)?;
        if under_configured_root(
            self.bus.space(),
            &self.configured_global,
            &self.owner.global,
        ) != self.owner.message_space
        {
            return Err(refused("Queue message-space owner changed"));
        }
        let current = owner_record(&self.owner)?;
        if let Some(expected) = &self.record_identity {
            if current.as_ref().map(|r| &r.identity) != Some(expected) {
                return Err(refused("Queue owner record identity changed"));
            }
        }
        Ok(())
    }
    fn live(&self) -> Result<(), FleetError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(refused("Queue execution has ended"));
        }
        let record = owner_record(&self.owner)?;
        let Some(record) = record else {
            return Ok(());
        };
        let live = match self.owner.kind {
            WorkOwnerKind::Session => {
                matches!(record.status.as_str(), "idle" | "running" | "failed")
            }
            WorkOwnerKind::Workflow | WorkOwnerKind::Standalone => {
                matches!(record.status.as_str(), "running" | "pending")
            }
            WorkOwnerKind::Agentiflow => record.status == "running",
        };
        if !live {
            return Err(refused("Queue owner is no longer active"));
        }
        Ok(())
    }
}
impl WorkQueuePort for RuntimeQueue {
    fn binding(&self) -> WorkOwnerBinding {
        self.owner.clone()
    }
    fn actor(&self) -> WorkActor {
        self.actor.clone()
    }
    fn finish_execution(&self) {
        self.active.store(false, Ordering::Release);
    }
    fn list(&self, query: WorkPageQuery) -> Result<WorkPage, WorkError> {
        self.owner_current()?;
        self.bus.board.work_list(query)
    }
    fn get(&self, id: &str) -> Result<WorkItem, WorkError> {
        self.owner_current()?;
        self.bus.board.work_get(id)
    }
    fn timeline(&self, query: WorkTimelineQuery) -> Result<WorkTimelinePage, WorkError> {
        self.owner_current()?;
        self.bus.board.work_timeline(query)
    }
    fn apply(&self, command: WorkCommand, source: WorkSource) -> Result<WorkReply, WorkError> {
        let _lifecycle = RetentionLease::try_acquire(&self.owner.global).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                WorkError::Busy
            } else {
                WorkError::Persistence(format!("Queue lifecycle guard: {e}"))
            }
        })?;
        self.owner_current()?;
        self.bus.work_apply(
            command,
            WorkContext {
                actor: self.actor.clone(),
                source: Some(source),
            },
            || self.owner_current(),
            || self.live(),
        )
    }
}

/// The launch site is the sole source of owner identity. A Bus pathname alone
/// never establishes a parent, and old process handoffs cannot opt themselves in.
pub fn bind(
    global: &Path,
    project: Option<&Path>,
    spec: &LaunchSpec,
    bus: Arc<Bus>,
    execution_id: String,
) -> Result<Arc<dyn WorkQueuePort>, String> {
    let configured_global = global.to_path_buf();
    let global = global
        .canonicalize()
        .map_err(|e| format!("Queue configured root unavailable: {e}"))?;
    let normalize = |path: &Path| under_configured_root(path, &configured_global, &global);
    let canonical_project = project
        .map(|path| path.canonicalize())
        .transpose()
        .map_err(|e| format!("Queue project root unavailable: {e}"))?;
    let transcript_path = match (project, canonical_project.as_deref()) {
        (Some(configured), Some(canonical)) if spec.transcript_path.starts_with(configured) => {
            under_configured_root(&spec.transcript_path, configured, canonical)
        }
        _ => normalize(&spec.transcript_path),
    };
    let (owner, round) = match &spec.origin {
        Origin::SubAgent { .. } => (
            spec.services
                .work_queue_owner
                .clone()
                .ok_or("parent has no typed Queue owner binding")?,
            None,
        ),
        Origin::SessionTurn {
            session_id,
            session_dir,
        } => (
            WorkOwnerBinding {
                global: global.clone(),
                kind: WorkOwnerKind::Session,
                id: session_id.clone(),
                directory: normalize(session_dir),
                message_space: normalize(session_dir).join("messages"),
            },
            None,
        ),
        Origin::WorkflowStep {
            workflow_run_id, ..
        } => {
            let directory = global.join("runs").join(workflow_run_id);
            (
                WorkOwnerBinding {
                    global: global.clone(),
                    kind: WorkOwnerKind::Workflow,
                    id: workflow_run_id.clone(),
                    message_space: directory.join("messages"),
                    directory,
                },
                None,
            )
        }
        Origin::FlowLead {
            flow_id, flow_dir, ..
        } => (
            WorkOwnerBinding {
                global: global.clone(),
                kind: WorkOwnerKind::Agentiflow,
                id: flow_id.clone(),
                directory: normalize(flow_dir),
                message_space: normalize(flow_dir),
            },
            Some(
                spec.services
                    .work_queue_round
                    .ok_or("lead round has no exact Queue recording identity")?,
            ),
        ),
        Origin::FlowUnit {
            flow_id, flow_dir, ..
        } => (
            WorkOwnerBinding {
                global: global.clone(),
                kind: WorkOwnerKind::Agentiflow,
                id: flow_id.clone(),
                directory: normalize(flow_dir),
                message_space: normalize(flow_dir),
            },
            None,
        ),
        Origin::Standalone if spec.link.is_none() => {
            let directory = global.join("runs").join(&spec.run_id);
            (
                WorkOwnerBinding {
                    global: global.clone(),
                    kind: WorkOwnerKind::Standalone,
                    id: spec.run_id.clone(),
                    message_space: directory.join("messages"),
                    directory,
                },
                None,
            )
        }
        _ => return Err("legacy process-child handoff has no typed Queue owner".into()),
    };
    if owner.global != global
        || !valid_id(&owner.id)
        || !valid_id(&spec.run_id)
        || !valid_id(&execution_id)
    {
        return Err("invalid Queue execution or owner identity".into());
    }
    let collection = match owner.kind {
        WorkOwnerKind::Session => "sessions",
        WorkOwnerKind::Agentiflow => "agentiflows",
        _ => "runs",
    };
    if owner.directory != global.join(collection).join(&owner.id)
        || normalize(bus.space()) != owner.message_space
    {
        return Err("handed message space does not match explicit Queue owner".into());
    }
    let owner_identity = directory_identity(&owner.directory).map_err(|e| e.to_string())?;
    let global_identity = directory_identity(&owner.global).map_err(|e| e.to_string())?;
    let record = owner_record(&owner).map_err(|e| e.to_string())?;
    if record
        .as_ref()
        .and_then(|r| r.workspace_id.as_deref())
        .is_some_and(|id| id != spec.workspace.id)
    {
        return Err("Queue owner belongs to a different workspace".into());
    }
    let record_identity = record.map(|r| r.identity);
    regular_path(&owner.message_space, true).map_err(|e| e.to_string())?;
    regular_path(&transcript_path, true).map_err(|e| e.to_string())?;
    if let Origin::SubAgent { parent } = &spec.origin {
        let expected = global
            .join("runs")
            .join(&parent.identity.run_id)
            .join("sub")
            .join(&spec.run_id)
            .join("transcript.jsonl");
        if !valid_id(&parent.identity.run_id) || transcript_path != expected {
            return Err("child recording does not match its trusted parent execution".into());
        }
    }
    let (anchor, relative_path, workspace_id) =
        if let Some(p) = relative(&transcript_path, &owner.directory) {
            (WorkRecordingAnchor::Owner, p, None)
        } else if let Some(p) = relative(&transcript_path, &global.join("transcripts")) {
            (WorkRecordingAnchor::GlobalTranscripts, p, None)
        } else if let Some(p) = canonical_project
            .as_deref()
            .and_then(|root| relative(&transcript_path, &root.join(".rupu/transcripts")))
        {
            if spec.workspace.id.is_empty() {
                return Err("project recording has no workspace identity".into());
            }
            (
                WorkRecordingAnchor::ProjectTranscripts,
                p,
                Some(spec.workspace.id.clone()),
            )
        } else if let Origin::SubAgent { parent } = &spec.origin {
            let exact = global
                .join("runs")
                .join(&parent.identity.run_id)
                .join("sub")
                .join(&spec.run_id)
                .join("transcript.jsonl");
            if !valid_id(&parent.identity.run_id) || transcript_path != exact {
                return Err("child recording does not match its trusted parent execution".into());
            }
            (
                WorkRecordingAnchor::GlobalRuns,
                relative(&exact, &global.join("runs")).ok_or("invalid child recording")?,
                None,
            )
        } else {
            return Err("recording is outside supported Queue transcript anchors".into());
        };
    let actor = WorkActor::Agent {
        address: bus.participant.clone(),
        run_id: spec.run_id.clone(),
        execution_id,
        agent: spec.agent.name.clone(),
        recording: WorkRecording {
            anchor,
            relative_path,
            workspace_id,
            round,
        },
    };
    Ok(Arc::new(RuntimeQueue {
        owner,
        configured_global,
        owner_identity,
        record_identity,
        global_identity,
        bus,
        actor,
        active: AtomicBool::new(true),
    }))
}

/// Small observations only. This does not wake idle sessions, choose work,
/// claim it, dispatch children, or upgrade an execution's permissions.
pub struct QueueCollector {
    pub queue: Arc<dyn WorkQueuePort>,
}
impl TurnCollector for QueueCollector {
    fn name(&self) -> &str {
        "work_queue"
    }
    fn collect(&self, _ctx: &TurnContext) -> Vec<Injection> {
        let actor = self.queue.actor();
        let WorkActor::Agent { ref run_id, .. } = actor else {
            return Vec::new();
        };
        let queued = self.queue.list(WorkPageQuery {
            filters: WorkFilters {
                state: Some(WorkState::Queued),
                claimant_run_id: None,
            },
            limit: 8,
            cursor: None,
        });
        let Ok(queued) = queued else {
            return Vec::new();
        };
        let mut own = Vec::new();
        let mut own_has_more = false;
        let mut run_counts = Vec::new();
        for state in [WorkState::Claimed, WorkState::Working, WorkState::Blocked] {
            let Ok(page) = self.queue.list(WorkPageQuery {
                filters: WorkFilters {
                    state: Some(state),
                    claimant_run_id: Some(run_id.clone()),
                },
                limit: 4,
                cursor: None,
            }) else {
                return Vec::new();
            };
            own_has_more |= page.has_more;
            run_counts.push(serde_json::json!({"state":state,"matching_total":page.counts.matching_total,"scope":"claimant_run_at_source_revision","source_revision":page.snapshot.revision}));
            own.extend(
                page.items
                    .into_iter()
                    .filter(|i| i.claimant.as_ref() == Some(&actor)),
            );
        }
        if queued.items.is_empty() && own.is_empty() {
            return Vec::new();
        }
        let brief = |item: &WorkItemSummary| serde_json::json!({"work_id":item.work_id,"revision":item.revision,"title":item.title,"state":item.state,"work_attempt_id":item.work_attempt_id});
        let value = serde_json::json!({"queue_id":queued.snapshot.queue_id,"queue_revision":queued.snapshot.revision,
            "queued":queued.items.iter().map(brief).collect::<Vec<_>>(),"queued_matching_total":queued.counts.matching_total,"queued_has_more":queued.has_more,
            "this_execution_active_attempts":own.iter().map(brief).collect::<Vec<_>>(),"run_scoped_counts":run_counts,
            "run_active_pages_have_more":own_has_more,"queued_limit":8,"active_limit_per_state":4});
        vec![Injection {source:"board:work_queue".into(),kind:InjectionKind::Observation,cadence:Cadence::EveryTurn,priority:100,
            content:format!("Durable Board work observation (bounded; may change). Select only work within the user's task. Use board.work_get/list for current revisions before claiming or transitioning. Posting does not launch or wake agents.\n{value}"),messages:Vec::new()}]
    }
}

#[cfg(all(test, unix))]
#[path = "work_queue/tests.rs"]
mod tests;
