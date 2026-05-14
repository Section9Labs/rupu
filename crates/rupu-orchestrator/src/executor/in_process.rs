//! InProcessExecutor — runs workflows in a tokio task and fans
//! events through every attached sink. The rupu.app singleton holds
//! one of these; the CLI builds a short-lived one per command.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

use crate::executor::errors::ExecutorError;
use crate::executor::sink::{EventSink, FanOutSink};
use crate::executor::{Event, InMemorySink, JsonlSink};
use crate::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use crate::runs::{ApprovalError, RunRecord, RunStatus, RunStore};

pub type EventStream = Pin<Box<dyn Stream<Item = Event> + Send>>;

/// Options for a fresh workflow run.
pub struct WorkflowRunOpts {
    pub workflow_path: PathBuf,
    pub vars: BTreeMap<String, String>,
}

/// Lightweight handle returned by [`WorkflowExecutor::start`].
pub struct RunHandle {
    pub run_id: String,
    pub workflow_path: PathBuf,
}

/// Filter used by [`WorkflowExecutor::list_runs`].
pub enum RunFilter {
    All,
    ByWorkflowPath(PathBuf),
    ByStatus(RunStatus),
    Active,
}

/// Surface for starting, inspecting, and controlling workflow runs.
#[async_trait]
pub trait WorkflowExecutor: Send + Sync {
    async fn start(
        &self,
        opts: WorkflowRunOpts,
        factory: Arc<dyn StepFactory>,
    ) -> Result<RunHandle, ExecutorError>;
    fn list_runs(&self, filter: RunFilter) -> Vec<RunRecord>;
    fn tail(&self, run_id: &str) -> Result<EventStream, ExecutorError>;
    async fn approve(&self, run_id: &str, approver: &str) -> Result<(), ExecutorError>;
    async fn reject(&self, run_id: &str, reason: &str) -> Result<(), ExecutorError>;
    async fn cancel(&self, run_id: &str) -> Result<(), ExecutorError>;
}

struct RunState {
    in_memory: Arc<InMemorySink>,
    #[allow(dead_code)]
    jsonl: Arc<JsonlSink>,
    #[allow(dead_code)]
    join: Mutex<Option<JoinHandle<()>>>,
    cancel: CancellationToken,
    workflow_path: PathBuf,
}

pub struct InProcessExecutor {
    run_store: Arc<RunStore>,
    runs: Mutex<HashMap<String, Arc<RunState>>>,
    extra_sinks: Vec<Arc<dyn EventSink>>,
    workspace_id: String,
    workspace_path: PathBuf,
    transcript_dir: PathBuf,
}

impl InProcessExecutor {
    pub fn new(
        run_store: Arc<RunStore>,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_dir: PathBuf,
    ) -> Self {
        Self {
            run_store,
            runs: Mutex::new(HashMap::new()),
            extra_sinks: Vec::new(),
            workspace_id,
            workspace_path,
            transcript_dir,
        }
    }

    /// Add a sink that receives events from every run started by this executor.
    pub fn add_sink(&mut self, sink: Arc<dyn EventSink>) {
        self.extra_sinks.push(sink);
    }
}

#[async_trait]
impl WorkflowExecutor for InProcessExecutor {
    async fn start(
        &self,
        opts: WorkflowRunOpts,
        factory: Arc<dyn StepFactory>,
    ) -> Result<RunHandle, ExecutorError> {
        // 1. Read + parse the workflow YAML.
        let yaml = std::fs::read_to_string(&opts.workflow_path)?;
        let workflow = crate::workflow::Workflow::parse(&yaml)?;

        // 2. Generate a run id (same scheme as the runner itself uses).
        let run_id = format!("run_{}", ulid::Ulid::new());

        // 3. Build sinks: in-memory broadcast + on-disk events.jsonl.
        let in_memory = Arc::new(InMemorySink::with_capacity(1024));
        let events_path = self.run_store.events_path(&run_id);
        let jsonl = Arc::new(
            JsonlSink::create(&events_path)
                .map_err(|e| ExecutorError::Internal(format!("JsonlSink::create: {e}")))?,
        );

        // 4. Fan-out sink: in_memory + jsonl + any extra sinks.
        let mut fan_sinks: Vec<Arc<dyn EventSink>> = vec![
            in_memory.clone() as Arc<dyn EventSink>,
            jsonl.clone() as Arc<dyn EventSink>,
        ];
        for s in &self.extra_sinks {
            fan_sinks.push(s.clone());
        }
        let fan_out = Arc::new(FanOutSink::new(fan_sinks));

        // 5. Build OrchestratorRunOpts.
        let orchestrator_opts = OrchestratorRunOpts {
            workflow,
            inputs: opts.vars.clone(),
            workspace_id: self.workspace_id.clone(),
            workspace_path: self.workspace_path.clone(),
            transcript_dir: self.transcript_dir.clone(),
            factory,
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(self.run_store.clone()),
            workflow_yaml: Some(yaml),
            resume_from: None,
            run_id_override: Some(run_id.clone()),
            strict_templates: false,
            event_sink: Some(fan_out as Arc<dyn EventSink>),
        };

        // 6. Stash state before spawning (so tail() works immediately).
        let cancel = CancellationToken::new();
        let state = Arc::new(RunState {
            in_memory,
            jsonl,
            join: Mutex::new(None),
            cancel: cancel.clone(),
            workflow_path: opts.workflow_path.clone(),
        });

        {
            let mut runs = self.runs.lock().unwrap();
            runs.insert(run_id.clone(), state.clone());
        }

        // 7. Spawn the runner task.
        let join: JoinHandle<()> = tokio::spawn(async move {
            if let Err(e) = run_workflow(orchestrator_opts).await {
                tracing::error!(error = %e, "InProcessExecutor: run_workflow failed");
            }
        });
        *state.join.lock().unwrap() = Some(join);

        Ok(RunHandle {
            run_id,
            workflow_path: opts.workflow_path,
        })
    }

    fn list_runs(&self, filter: RunFilter) -> Vec<RunRecord> {
        // Snapshot in-memory state under a single lock.
        let (in_memory_ids, in_memory_paths): (Vec<String>, HashMap<String, PathBuf>) = {
            let runs = self.runs.lock().unwrap();
            let ids: Vec<_> = runs.keys().cloned().collect();
            let paths: HashMap<_, _> = runs
                .iter()
                .map(|(id, state)| (id.clone(), state.workflow_path.clone()))
                .collect();
            (ids, paths)
        };

        let mut records: HashMap<String, RunRecord> = HashMap::new();

        // Load all disk records first.
        if let Ok(disk_records) = self.run_store.list() {
            for rec in disk_records {
                records.insert(rec.id.clone(), rec);
            }
        }

        // For in-memory runs that aren't on disk yet, try a targeted load.
        // If still missing (task hasn't called run_store.create yet),
        // synthesize a minimal Pending record so callers can see the run.
        for id in &in_memory_ids {
            if !records.contains_key(id) {
                if let Ok(rec) = self.run_store.load(id) {
                    records.insert(rec.id.clone(), rec);
                } else {
                    // Pre-disk stub: the spawned task hasn't persisted
                    // run.json yet. Synthesize a minimal record so the
                    // run is visible in list_runs immediately after start().
                    let wf_path = in_memory_paths.get(id).cloned().unwrap_or_default();
                    let wf_name = wf_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    records.insert(
                        id.clone(),
                        RunRecord {
                            id: id.clone(),
                            workflow_name: wf_name,
                            status: RunStatus::Running,
                            inputs: BTreeMap::new(),
                            event: None,
                            workspace_id: self.workspace_id.clone(),
                            workspace_path: self.workspace_path.clone(),
                            transcript_dir: self.transcript_dir.clone(),
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
                        },
                    );
                }
            }
        }

        // Snapshot active ids for the Active filter.
        let active_ids: std::collections::HashSet<String> = in_memory_ids.into_iter().collect();

        records
            .into_values()
            .filter(|rec| match &filter {
                RunFilter::All => true,
                RunFilter::ByWorkflowPath(p) => {
                    // RunRecord doesn't carry the workflow_path directly.
                    // We check in-memory state for in-flight runs; for
                    // completed runs this filter cannot match on disk.
                    if let Some(state_path) = in_memory_paths.get(&rec.id) {
                        state_path == p
                    } else {
                        // Fall back to comparing workspace_path + workflow_name
                        // (best-effort for disk-only records).
                        rec.workspace_path.join(&rec.workflow_name) == *p
                    }
                }
                RunFilter::ByStatus(s) => &rec.status == s,
                RunFilter::Active => active_ids.contains(&rec.id) && !rec.status.is_terminal(),
            })
            .collect()
    }

    fn tail(&self, run_id: &str) -> Result<EventStream, ExecutorError> {
        let runs = self.runs.lock().unwrap();
        let state = runs
            .get(run_id)
            .ok_or_else(|| ExecutorError::RunNotFound(run_id.to_string()))?
            .clone();
        drop(runs);

        let rx = state.in_memory.subscribe();
        let stream = BroadcastStream::new(rx).filter_map(|res| async move { res.ok() });
        Ok(Box::pin(stream))
    }

    async fn approve(&self, run_id: &str, approver: &str) -> Result<(), ExecutorError> {
        self.run_store
            .approve(run_id, approver, chrono::Utc::now())
            .map(|_| ())
            .map_err(map_approval_err)
    }

    async fn reject(&self, run_id: &str, reason: &str) -> Result<(), ExecutorError> {
        self.run_store
            .reject(run_id, "executor", reason, chrono::Utc::now())
            .map(|_| ())
            .map_err(map_approval_err)
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutorError> {
        let runs = self.runs.lock().unwrap();
        let state = runs
            .get(run_id)
            .ok_or_else(|| ExecutorError::RunNotFound(run_id.to_string()))?
            .clone();
        drop(runs);
        state.cancel.cancel();
        Ok(())
    }
}

fn map_approval_err(e: ApprovalError) -> ExecutorError {
    match e {
        ApprovalError::NotFound(id) => ExecutorError::RunNotFound(id),
        other => ExecutorError::Internal(other.to_string()),
    }
}
