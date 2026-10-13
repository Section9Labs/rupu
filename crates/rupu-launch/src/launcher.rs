//! [`RunLauncher`]: the one [`Launcher`] (spec W7 §3.3). Every child kind
//! goes through the same policy (allowlist, depth, ceiling, concurrency),
//! codename scheme, children ledger, events and usage rollup; only how the
//! child runs differs — an in-process task over the run assembler for a
//! `subagent`, a [`ProcessBackend`] child for an `agent` or `workflow`.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rupu_agent::{StreamOpts, UserTurn};
use rupu_codename::SharedNamer;
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::{RunStore, Workflow};
use rupu_runtime::argv::{
    AgentRun, FlowAttach, LaunchLink, MessageSpace, RunArgv, WorkflowControl, WorkflowRef,
    WorkflowRun,
};
use rupu_runtime::assembly::{
    LaunchServices, LaunchSpec, Origin, Overrides, ParentRun, RunAssembler, WorkspaceBinding,
};
use rupu_runtime::usage_ledger::UsageLedger;
use rupu_tools::{
    ChildHandle, ChildKind, ChildRequest, ChildState, ChildStatus, ChildTarget, ChildTokens,
    GeneratedWorkflow, JoinSet, LaunchError, Launcher, ParentEnd, PermissionMode, Prompter,
    RunIdentity, SpawnPermission, ToolContext, Until, WorkflowGenerator, MAX_LAUNCH_DEPTH,
};
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use chrono::{DateTime, Utc};

use crate::generate::Generation;
use crate::identity::{own_end, plus, tokens_of, Own};
use crate::ledger::{
    self, ChildEntry, Continued, Finished, Joined, LedgerLine, PriorRun, Request, Spawned, Started,
};
use crate::process::{
    transcripts_dir, ProcState, ProcessBackend, ProcessLaunch, SpawnedProcess, Stop,
};

/// How often a process child is polled.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How long a SIGTERMed process child gets before SIGKILL. Above the CLI's
/// credential-write drain on SIGTERM (`rupu-cli`'s `exit::CREDENTIAL_WRITE_DRAIN`),
/// which a SIGKILL must never cut short.
pub const TERM_GRACE: Duration = Duration::from_secs(12);

/// How long `finish` waits for a stopped sub-agent to reach a safe boundary
/// before it drops the task.
const SUBAGENT_STOP_GRACE: Duration = Duration::from_secs(30);

/// The default `[dispatch].max_concurrent_children`.
pub const DEFAULT_MAX_CONCURRENT_CHILDREN: usize = 8;

/// What a [`RunLauncher`] runs children with. One per root run (a `rupu
/// run`, a workflow run, a session turn, an agentiflow): every in-process
/// descendant shares it.
pub struct LaunchEnv {
    /// Builds sub-agent runs; its context gives the global and project dirs
    /// agents and workflows are loaded from.
    pub assembler: Arc<RunAssembler>,
    /// The global run store (`<RUPU_HOME>/runs`): sub-run directories and
    /// workflow children's records.
    pub store: Arc<RunStore>,
    /// Where the parent's live view hears `DispatchStarted` /
    /// `DispatchCompleted`.
    pub events: Option<Arc<dyn EventSink>>,
    /// The root run's codename namer (`<run_dir>/codenames.json`). `None` =
    /// an in-memory namer per parent crew.
    pub namer: Option<SharedNamer>,
    /// How process children run.
    pub backend: Arc<dyn ProcessBackend>,
    /// The runtime children's tasks run on — a multi-thread one, so a child
    /// keeps going while its parent is between turns.
    pub runtime: tokio::runtime::Handle,
    /// `[dispatch].max_concurrent_children`, per parent run.
    pub max_concurrent: usize,
    /// What `workflows.generate` authors with; `None` = resolved on first
    /// use from the caller's provider and the credentials on hand.
    pub generation: Option<Generation>,
    /// The project `.rupu` directory agents and workflows are loaded from;
    /// `None` = the assembler's project root's.
    pub project: Option<PathBuf>,
}

/// One child, in memory.
struct Child {
    handle: String,
    kind: ChildKind,
    name: String,
    run_id: String,
    codename: Option<String>,
    address: Option<String>,
    transcript: Option<PathBuf>,
    parent_run: String,
    ledger_dir: PathBuf,
    depth: u32,
    /// What its `started` line records beyond its handle.
    launch: LaunchRecord,
    born: Instant,
    stop: CancellationToken,
    /// What watches the child's task: it settles a child whose task panicked
    /// or was dropped.
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The child's task itself.
    work: Mutex<Option<tokio::task::AbortHandle>>,
    st: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    state: Option<ChildState>,
    success: Option<bool>,
    output: Option<String>,
    error: Option<String>,
    cause: Option<serde_json::Value>,
    tokens: Option<ChildTokens>,
    duration_ms: Option<u64>,
    /// `join` has returned its final status: it is collected.
    joined: bool,
    /// Its process was spawned.
    spawned: bool,
    /// What a stop records, why, and when it was asked for: set by `finish`
    /// before it stops the child (`interrupted` when its parent paused or is
    /// being stopped by a signal).
    stop_as: Option<(ChildState, String, DateTime<Utc>)>,
    /// It failed, but its own records say a stop ended it: what it left
    /// below is its parent's end to decide (kept when the parent is stopped
    /// too, `abandoned` when it ends for good).
    below_pending: bool,
}

/// What a child's `started` line records beyond its handle (see
/// [`Started`]).
#[derive(Clone, Default)]
struct LaunchRecord {
    request: Option<Box<Request>>,
    continues: Option<Box<PriorRun>>,
    prior_tokens: Option<ChildTokens>,
    tree: Option<PathBuf>,
}

/// `child` as its `started` line records it.
fn started_of(child: &Child) -> Started {
    Started {
        codename: child.codename.clone(),
        address: child.address.clone(),
        transcript: child.transcript.clone(),
        parent_start: crate::proc::own_start_stamp(),
        request: child.launch.request.clone(),
        continues: child.launch.continues.clone(),
        prior_tokens: child.launch.prior_tokens,
        tree: child.launch.tree.clone(),
        ..Started::new(
            &child.handle,
            child.kind,
            &child.name,
            &child.run_id,
            std::process::id(),
            child.depth,
        )
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Child {
    fn state(&self) -> ChildState {
        lock(&self.st).state.unwrap_or(ChildState::Pending)
    }

    fn status(&self) -> ChildStatus {
        let st = lock(&self.st);
        ChildStatus {
            handle: self.handle.clone(),
            kind: self.kind,
            name: self.name.clone(),
            run_id: self.run_id.clone(),
            codename: self.codename.clone(),
            address: self.address.clone(),
            transcript: self.transcript.clone(),
            state: st.state.unwrap_or(ChildState::Pending),
            success: st.success,
            output: st.output.clone(),
            error: st.error.clone(),
            cause: st.cause.clone(),
            tokens: st.tokens,
            duration_ms: st.duration_ms,
            elapsed: self.born.elapsed(),
        }
    }
}

/// How a child ended. `tokens` are its run's own: its earlier runs' are
/// added when it is settled.
struct End {
    state: ChildState,
    success: Option<bool>,
    output: Option<String>,
    error: Option<String>,
    cause: Option<serde_json::Value>,
    tokens: Option<ChildTokens>,
    /// When it was stopped, for an end a stop recorded.
    at: Option<DateTime<Utc>>,
}

impl End {
    fn failed(error: impl Into<String>) -> Self {
        Self {
            state: ChildState::Failed,
            success: None,
            output: None,
            error: Some(error.into()),
            cause: None,
            tokens: None,
            at: None,
        }
    }

    fn abandoned(error: impl Into<String>) -> Self {
        Self {
            state: ChildState::Abandoned,
            ..Self::failed(error)
        }
    }

    fn interrupted(error: impl Into<String>) -> Self {
        Self {
            state: ChildState::Interrupted,
            ..Self::failed(error)
        }
    }

    /// `child` was stopped: what its parent's end set ([`Inner::stop_as`]),
    /// else `abandoned` with `why`.
    fn stopped(child: &Child, why: &str) -> Self {
        match lock(&child.st).stop_as.clone() {
            Some((state, why, at)) => Self {
                state,
                at: Some(at),
                ..Self::failed(why)
            },
            None => Self::abandoned(why),
        }
    }
}

/// How a child runs once it has a slot.
enum Work {
    /// In this process, at `ceiling`. `continues` is the earlier sub-run
    /// whose children it takes on, when it continues one.
    Subagent {
        spec: Box<rupu_agent::AgentSpec>,
        prompt: Box<UserTurn>,
        ceiling: PermissionMode,
        continues: Option<String>,
    },
    Process(Box<ProcessLaunch>),
}

/// Why a process child was not spawned.
enum Refused {
    /// This process is being stopped by a signal.
    Stopping,
    Spawn(String),
}

const STOPPING: &str = "its parent's process is stopping";

impl Refused {
    fn into_launch_error(self) -> LaunchError {
        match self {
            Refused::Stopping => LaunchError::new("stopping", "the run is stopping"),
            Refused::Spawn(e) => LaunchError::new("spawn_failed", e),
        }
    }
}

/// What a checked request will run.
enum Vetted {
    Agent {
        name: String,
        /// Loaded for a sub-agent, which this process builds; a process
        /// child loads its own (and reports a missing one as its failure).
        spec: Option<Box<rupu_agent::AgentSpec>>,
        prompt: String,
    },
    Workflow {
        name: String,
        target: WorkflowRef,
        inputs: BTreeMap<String, String>,
    },
}

impl Vetted {
    fn name(&self) -> &str {
        match self {
            Vetted::Agent { name, .. } | Vetted::Workflow { name, .. } => name,
        }
    }
}

/// What a child takes from the call that launched it.
#[derive(Clone)]
struct Caller {
    identity: Arc<RunIdentity>,
    workspace: WorkspaceBinding,
    ceiling: PermissionMode,
    prompter: Option<Arc<dyn Prompter>>,
    coverage_stream: Option<PathBuf>,
    engagement: Vec<String>,
    generator: Option<Arc<dyn WorkflowGenerator>>,
    /// The directory of the run at the root of its launch tree.
    tree: Option<PathBuf>,
    /// The parent's bus on its message space: its children share the space
    /// (spec F1a FD6), except a workflow child, which is its own root.
    bus: Option<Arc<rupu_fleet::Bus>>,
}

impl Caller {
    fn of(ctx: &ToolContext) -> Self {
        let permission = SpawnPermission::from_ctx(ctx);
        Self {
            identity: Arc::clone(&ctx.identity),
            workspace: WorkspaceBinding {
                id: ctx.workspace.id.clone(),
                path: ctx.workspace.path.clone(),
            },
            ceiling: permission.ceiling,
            prompter: permission.prompter,
            coverage_stream: ctx.services.coverage_stream.clone(),
            engagement: ctx
                .services
                .findings
                .as_ref()
                .and_then(|f| f.engagement.as_deref())
                .map(|set| set.ids().into_iter().map(str::to_string).collect())
                .unwrap_or_default(),
            generator: ctx.services.workflow_generator.clone(),
            tree: ctx
                .identity
                .launch
                .rollup_root
                .as_deref()
                .or(ctx.identity.launch.usage_root.as_deref())
                .and_then(Path::parent)
                .map(Path::to_path_buf),
            bus: ctx.services.message_space.clone(),
        }
    }

    /// A process child's ceiling. Under an interactive `ask` parent it can't
    /// reach the operator's prompt, and an `ask` it runs unattended would
    /// allow every write, so it runs readonly. An unattended parent's `ask`
    /// is already unattended and stays.
    fn process_ceiling(&self) -> PermissionMode {
        match (self.ceiling, &self.prompter) {
            (PermissionMode::Ask, Some(_)) => PermissionMode::Readonly,
            (ceiling, _) => ceiling,
        }
    }

    /// What a process child carries across the boundary (D18).
    fn link(&self) -> LaunchLink {
        let id = &self.identity;
        LaunchLink {
            depth: id.depth + 1,
            ceiling: self.process_ceiling(),
            parent_run: id.run_id.clone(),
            root_run: id
                .parent
                .as_ref()
                .map(|p| p.root_run_id.clone())
                .unwrap_or_else(|| id.run_id.clone()),
            // The top of the tree: the root this run rolls up to, else its
            // own ledger.
            usage_root: id
                .launch
                .rollup_root
                .clone()
                .or_else(|| id.launch.usage_root.clone()),
            agents: id.launch.agents.clone(),
            workflows: id.launch.workflows.clone(),
            actions: id.launch.connector_cap.clone(),
        }
    }

    /// The mode and link a child runs at: this caller's for a new one; for
    /// one launched before as `was`, no wider than it ran then (the lower
    /// ceiling, the caps both allow, the deeper depth).
    fn bounds(
        &self,
        kind: ChildKind,
        was: Option<&Request>,
    ) -> (PermissionMode, Option<LaunchLink>) {
        let (mode, link) = match kind {
            ChildKind::Subagent => (self.ceiling, None),
            _ => (self.process_ceiling(), Some(self.link())),
        };
        let Some(was) = was else {
            return (mode, link);
        };
        let link = match (link, &was.link) {
            (Some(now), Some(then)) => Some(LaunchLink {
                depth: now.depth.max(then.depth),
                ceiling: now.ceiling.min(then.ceiling),
                agents: both(&now.agents, &then.agents),
                workflows: both(&now.workflows, &then.workflows),
                actions: match (now.actions, &then.actions) {
                    (Some(a), Some(b)) => Some(both(&a, b)),
                    (a, b) => a.or_else(|| b.clone()),
                },
                ..now
            }),
            (link, _) => link,
        };
        (mode.min(was.mode), link)
    }
}

/// The names in both lists, in `a`'s order.
fn both(a: &[String], b: &[String]) -> Vec<String> {
    a.iter().filter(|x| b.contains(x)).cloned().collect()
}

/// `workflows.generate`'s provider per [`GenerationKey`], each resolved
/// once: callers of one launcher can run on different models.
#[derive(Default)]
struct GenerationCache(Mutex<HashMap<GenerationKey, GenerationSlot>>);

type GenerationSlot = Arc<tokio::sync::OnceCell<Option<Generation>>>;

/// Who a generation provider is resolved for: the caller's provider and
/// model, which the resolution falls back to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GenerationKey {
    provider: String,
    model: String,
}

impl GenerationKey {
    fn of(id: &RunIdentity) -> Self {
        Self {
            provider: id.provider.clone(),
            model: id.model.clone(),
        }
    }
}

impl GenerationCache {
    async fn get_or_resolve<F, Fut>(&self, key: GenerationKey, resolve: F) -> Option<Generation>
    where
        F: FnOnce(GenerationKey) -> Fut,
        Fut: Future<Output = Option<Generation>>,
    {
        let cell = Arc::clone(lock(&self.0).entry(key.clone()).or_default());
        cell.get_or_init(|| resolve(key)).await.clone()
    }
}

/// The one launcher. See the module docs.
pub struct RunLauncher {
    env: LaunchEnv,
    me: Weak<RunLauncher>,
    children: Mutex<Vec<Arc<Child>>>,
    slots: Mutex<HashMap<String, Arc<Semaphore>>>,
    changed: Notify,
    namers: Mutex<HashMap<String, SharedNamer>>,
    /// A fallback address counter for a flow child with no codename.
    next_address: AtomicU32,
    generation: GenerationCache,
}

impl RunLauncher {
    pub fn new(env: LaunchEnv) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            env,
            me: me.clone(),
            children: Mutex::new(Vec::new()),
            slots: Mutex::new(HashMap::new()),
            changed: Notify::new(),
            namers: Mutex::new(HashMap::new()),
            next_address: AtomicU32::new(0),
            generation: GenerationCache::default(),
        })
    }

    fn arc(&self) -> Arc<RunLauncher> {
        self.me
            .upgrade()
            .expect("a launcher is only used through its Arc")
    }

    fn project_rupu(&self) -> Option<PathBuf> {
        self.env.project.clone().or_else(|| {
            self.env
                .assembler
                .context()
                .project_root
                .as_ref()
                .map(|p| p.join(".rupu"))
        })
    }

    fn global(&self) -> &Path {
        &self.env.assembler.context().global
    }

    fn slot(&self, parent: &str) -> Arc<Semaphore> {
        Arc::clone(
            lock(&self.slots)
                .entry(parent.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(self.env.max_concurrent.max(1)))),
        )
    }

    fn children_of(&self, parent: &str) -> Vec<Arc<Child>> {
        lock(&self.children)
            .iter()
            .filter(|c| c.parent_run == parent)
            .cloned()
            .collect()
    }

    /// `<parent>><role>#n` from the root's namer (or an in-memory one for the
    /// parent's crew). `None` when the parent has no codename.
    fn mint_codename(&self, parent: Option<&str>, name: &str) -> Option<String> {
        let parent = parent?;
        let pc: rupu_codename::Codename = parent.parse().ok()?;
        let namer = match &self.env.namer {
            Some(n) => n.clone(),
            None => lock(&self.namers)
                .entry(pc.crew.clone())
                .or_insert_with(|| {
                    SharedNamer::in_memory(rupu_codename::CrewNamer::new(pc.crew.clone()))
                })
                .clone(),
        };
        Some(namer.with(|n| {
            let role = n.canonical_role(name);
            let k = n.next_instance(&pc, &role);
            pc.child(&role, Some(k)).to_string()
        }))
    }

    /// A flow child's address on the board: its codename below the flow's
    /// lead (`lynx#1`, `lynx#1>heron#1`), else `<name>#<n>`. (A child in any
    /// other space is addressed by `Bus::address_below`, with this as the
    /// fallback for one without a codename.)
    fn address(&self, codename: Option<&str>, name: &str) -> String {
        codename
            .and_then(|c| c.split_once('/').map(|(_, path)| path))
            .and_then(|path| path.split_once('>').map(|(_, below)| below.to_string()))
            .unwrap_or_else(|| {
                let n = self.next_address.fetch_add(1, Ordering::Relaxed) + 1;
                format!("{name}#{n}")
            })
    }

    fn emit(&self, parent: &str, ev: Event) {
        if let Some(events) = &self.env.events {
            events.emit(parent, &ev);
        }
    }

    /// Settle `child` with `end`: its status (its tokens with its earlier
    /// runs'), its `finished` line, the parent's `DispatchCompleted`.
    /// Idempotent: an ended child stays ended.
    fn settle(&self, child: &Child, end: End, started: Instant) {
        let duration_ms = started.elapsed().as_millis() as u64;
        let tokens = match end.tokens {
            Some(t) => plus(child.launch.prior_tokens, Some(t)),
            None => child.launch.prior_tokens,
        };
        {
            let mut st = lock(&child.st);
            if st.state.is_some_and(ChildState::is_terminal) {
                return;
            }
            st.state = Some(end.state);
            st.success = end.success;
            st.output = end.output.clone();
            st.error = end.error.clone();
            st.cause = end.cause.clone();
            st.tokens = tokens;
            st.duration_ms = Some(duration_ms);
        }
        ledger::append(
            &child.ledger_dir,
            &LedgerLine::Finished(Finished {
                handle: child.handle.clone(),
                status: end.state,
                success: end.success,
                output: end.output,
                error: end.error,
                cause: end.cause.clone(),
                tokens,
                duration_ms: Some(duration_ms),
                at: end.at.unwrap_or_else(Utc::now),
            }),
        );
        self.emit(
            &child.parent_run,
            Event::DispatchCompleted {
                run_id: child.parent_run.clone(),
                sub_run_id: child.run_id.clone(),
                kind: child.kind.as_str().into(),
                success: end.success == Some(true),
                tokens_in: tokens.map_or(0, |t| t.input),
                tokens_out: tokens.map_or(0, |t| t.output),
                cause: end.cause.and_then(|c| serde_json::from_value(c).ok()),
            },
        );
        self.changed.notify_waiters();
    }

    fn set_state(&self, child: &Child, state: ChildState) {
        let mut st = lock(&child.st);
        if st.state.is_some_and(ChildState::is_terminal) || st.state == Some(state) {
            return;
        }
        st.state = Some(state);
        drop(st);
        self.changed.notify_waiters();
    }

    /// The child's `started` line: written as it is launched, queued or not.
    fn record_launch(&self, child: &Child) {
        crate::reaper::track(&child.ledger_dir);
        ledger::append(&child.ledger_dir, &LedgerLine::Started(started_of(child)));
    }

    /// A process child's process, as it was spawned.
    fn record_spawn(&self, child: &Child, process: Option<SpawnedProcess>) {
        lock(&child.st).spawned = true;
        if let Some(p) = process {
            ledger::append(
                &child.ledger_dir,
                &LedgerLine::Spawned(Spawned {
                    handle: child.handle.clone(),
                    pid: p.pid,
                    proc_start: p.start,
                    at: Utc::now(),
                }),
            );
        }
    }

    /// Refusals are worded for where the caller is: a flow's pool, or its own
    /// allowlist.
    fn not_allowed(
        id: &RunIdentity,
        what: &str,
        name: &str,
        field: &str,
        allowed: &[String],
    ) -> LaunchError {
        if id.launch.pool.is_some() {
            return LaunchError::new(
                "agent_not_dispatchable",
                format!("{what} `{name}` is not in this flow's pool"),
            );
        }
        LaunchError::new(
            "agent_not_dispatchable",
            format!(
                "`{name}` is not in this agent's `{field}` allowlist (allowed: [{}])",
                allowed.join(", ")
            ),
        )
    }

    async fn vet(&self, req: &ChildRequest, ctx: &ToolContext) -> Result<Vetted, LaunchError> {
        let id = &ctx.identity;
        if id.launch.children_dir.is_none() {
            return Err(LaunchError::new(
                "no_parent_run_id",
                "this run has no run directory to record its children under",
            ));
        }
        if id.depth >= MAX_LAUNCH_DEPTH {
            return Err(LaunchError::new(
                "max_dispatch_depth_exceeded",
                format!("current depth {} >= ceiling {MAX_LAUNCH_DEPTH}", id.depth),
            ));
        }
        match req.kind {
            ChildKind::Subagent | ChildKind::Agent => {
                let ChildTarget::Name(name) = &req.target else {
                    return Err(LaunchError::new(
                        "invalid_target",
                        "an agent child is named, not a file",
                    ));
                };
                if !id.launch.agents.contains(name) {
                    return Err(Self::not_allowed(
                        id,
                        "agent",
                        name,
                        "dispatchableAgents",
                        &id.launch.agents,
                    ));
                }
                let prompt = req
                    .prompt
                    .clone()
                    .filter(|p| !p.trim().is_empty())
                    .ok_or_else(|| {
                        LaunchError::new("invalid_prompt", "an agent child needs a prompt")
                    })?;
                let project = self.project_rupu();
                let spec = if req.kind == ChildKind::Subagent {
                    Some(Box::new(
                        rupu_agent::load_agent_admitted(self.global(), project.as_deref(), name)
                            .await
                            .map_err(|e| match e {
                                rupu_agent::AgentLoadError::NotFound(_) => LaunchError::new(
                                    "agent_not_found",
                                    format!(
                                        "agent `{name}` not found in any registered agent path"
                                    ),
                                ),
                                other => LaunchError::new(
                                    "agent_load_failed",
                                    format!("load agent `{name}`: {other}"),
                                ),
                            })?,
                    ))
                } else {
                    None
                };
                Ok(Vetted::Agent {
                    name: name.clone(),
                    spec,
                    prompt: rupu_tools::launch::fold_inputs(&prompt, &req.inputs),
                })
            }
            ChildKind::Workflow => self.vet_workflow(req, id),
        }
    }

    /// A workflow child, fail-closed and before anything is spawned (its
    /// stderr is discarded, so this is the only place the caller learns why
    /// it would refuse to start): it is allowed, it parses, it has no
    /// approval gate and no remote placement, every agent it runs is one the
    /// caller may dispatch, and its inputs resolve.
    fn vet_workflow(&self, req: &ChildRequest, id: &RunIdentity) -> Result<Vetted, LaunchError> {
        let (label, wf, target) = match &req.target {
            ChildTarget::Name(name) => {
                if !id.launch.workflows.contains(name) {
                    if id.launch.pool.is_some() {
                        return Err(LaunchError::new(
                            "workflow_not_dispatchable",
                            format!("workflow '{name}' is not in this flow's pool"),
                        ));
                    }
                    return Err(LaunchError::new(
                        "workflow_not_dispatchable",
                        format!(
                            "`{name}` is not in this agent's `dispatchableWorkflows` allowlist (allowed: [{}])",
                            id.launch.workflows.join(", ")
                        ),
                    ));
                }
                let project = self.project_rupu();
                let not_found = || {
                    LaunchError::new(
                        "workflow_not_found",
                        format!("unknown or unparseable workflow '{name}'"),
                    )
                };
                // The child runs this file, not the name looked up again from
                // its own directory: what runs is what was vetted.
                let path = rupu_orchestrator::catalog::workflow_path(
                    self.global(),
                    project.as_deref(),
                    name,
                )
                .ok_or_else(not_found)?;
                let wf = Workflow::parse_file(&path).map_err(|_| not_found())?;
                (name.clone(), wf, WorkflowRef::File(path))
            }
            ChildTarget::File(file) => {
                // Only a workflow this run generated (`workflows.generate`).
                let generated = id
                    .launch
                    .children_dir
                    .as_ref()
                    .map(|d| d.join("generated"))
                    .and_then(|d| d.canonicalize().ok());
                let inside = match (generated, file.canonicalize().ok()) {
                    (Some(dir), Some(f)) => f.starts_with(dir),
                    _ => false,
                };
                if !inside {
                    return Err(LaunchError::new(
                        "workflow_not_dispatchable",
                        format!("`{}` is not a workflow this run generated", file.display()),
                    ));
                }
                let text = std::fs::read_to_string(file).map_err(|e| {
                    LaunchError::new("workflow_not_found", format!("{}: {e}", file.display()))
                })?;
                let wf = Workflow::parse(&text).map_err(|e| {
                    LaunchError::new("workflow_not_found", format!("{}: {e}", file.display()))
                })?;
                if let Some(why) = connector_actions_not_granted(&wf, id) {
                    return Err(LaunchError::new("workflow_not_runnable", why));
                }
                (wf.name.clone(), wf, WorkflowRef::File(file.clone()))
            }
        };
        if wf.has_approval_gate() || wf.has_placed_step() {
            return Err(LaunchError::new(
                "workflow_not_runnable",
                format!("v1 cannot run a gated or host/distribute workflow as a child ({label})"),
            ));
        }
        let missing: Vec<String> = wf
            .dispatched_agents()
            .into_iter()
            .filter(|a| !id.launch.agents.contains(a))
            .collect();
        if !missing.is_empty() {
            let place = if id.launch.pool.is_some() {
                "outside the pool"
            } else {
                "you may not dispatch"
            };
            return Err(LaunchError::new(
                "workflow_not_runnable",
                format!(
                    "workflow '{label}' dispatches agents {place}: {}",
                    missing.join(", ")
                ),
            ));
        }
        let inputs = rupu_tools::launch::workflow_input_strings(&req.inputs)
            .map_err(|e| LaunchError::new("invalid_inputs", e))?;
        if let Err(e) = rupu_orchestrator::runner::resolve_inputs(&wf, &inputs) {
            return Err(LaunchError::new(
                "invalid_inputs",
                format!("invalid inputs for workflow '{label}': {e}"),
            ));
        }
        Ok(Vetted::Workflow {
            name: label,
            target,
            inputs,
        })
    }

    /// Run a sub-agent child to its end at `ceiling`. One that continues an
    /// earlier sub-run (`continues`) takes on that run's children first.
    async fn run_subagent(
        self: Arc<Self>,
        child: Arc<Child>,
        spec: rupu_agent::AgentSpec,
        prompt: UserTurn,
        ceiling: PermissionMode,
        from: Caller,
        continues: Option<String>,
    ) {
        let started = Instant::now();
        let launch = LaunchSpec {
            agent: spec,
            origin: Origin::SubAgent {
                parent: ParentRun {
                    identity: Arc::clone(&from.identity),
                    usage: from
                        .identity
                        .launch
                        .usage_root
                        .clone()
                        .map(UsageLedger::open),
                    coverage_stream: from.coverage_stream.clone(),
                },
            },
            run_id: child.run_id.clone(),
            transcript_path: child.transcript.clone().unwrap_or_default(),
            codename: child.codename.clone(),
            prompt,
            workspace: from.workspace.clone(),
            mode: ceiling,
            ceiling: Some(ceiling),
            prompter: from.prompter.clone(),
            overrides: Overrides::default(),
            // The parent's printer renders the child from its transcript.
            stream: StreamOpts {
                suppress_stdout: true,
                ..StreamOpts::default()
            },
            hooks: Default::default(),
            // `finish` stops an abandoned child at its next safe boundary.
            pause: Some(child.stop.clone()),
            services: LaunchServices {
                launcher: Some(Arc::clone(&self) as Arc<dyn Launcher>),
                workflow_generator: from.generator.clone(),
                // It shares its parent's message space.
                message_bus: from
                    .bus
                    .as_ref()
                    .zip(child.address.as_ref())
                    .map(|(bus, address)| Arc::new(bus.sibling(address.clone()))),
                ..Default::default()
            },
            collectors: Vec::new(),
            link: None,
        };
        let (provider, model) = self
            .env
            .assembler
            .resolve_provider_model(&launch.agent, &launch.overrides);
        let assembled = self.env.assembler.assemble(launch).await;
        self.set_state(&child, ChildState::Running);
        self.emit(
            &child.parent_run,
            Event::DispatchStarted {
                run_id: child.parent_run.clone(),
                sub_run_id: child.run_id.clone(),
                kind: ChildKind::Subagent.as_str().into(),
                agent: Some(child.name.clone()),
                transcript_path: child.transcript.clone().unwrap_or_default(),
                codename: child.codename.clone(),
                provider: Some(provider),
                model: Some(model),
            },
        );
        let assembled = match assembled {
            Ok(a) => a,
            Err(e) => return self.settle(&child, End::failed(e.to_string()), started),
        };
        if let Some(prior) = continues {
            self.continue_children(&prior, &assembled.opts.tool_context)
                .await;
        }
        let end = match assembled.run().await.result {
            // Stopped with its parent's process: continued with it.
            Err(e)
                if matches!(e, rupu_agent::RunError::Terminating) || crate::reaper::stopping() =>
            {
                End::interrupted(e.to_string())
            }
            Err(e) => End {
                cause: e.outcome().and_then(|o| serde_json::to_value(o).ok()),
                ..End::failed(e.to_string())
            },
            Ok(r) if r.paused => {
                let end = End::stopped(&child, "its parent ended without joining it");
                if end.state != ChildState::Interrupted {
                    // Its own run ended paused, so it recorded its children
                    // `interrupted`; nothing will continue them.
                    self.abandon_in(&child.run_id, "its parent ended without joining it")
                        .await;
                }
                end
            }
            Ok(r) => {
                // `Ok` does not mean the child did its job: a run that burned
                // its whole turn budget comes back `Ok` with a terminal error.
                // A failed child's output is empty (its last text is interim
                // or cut off, spec 2026-10-01 §5.3).
                let terminal = r.terminal_error();
                let success = terminal.is_none();
                End {
                    state: ChildState::Done,
                    success: Some(success),
                    output: Some(if success {
                        child
                            .transcript
                            .as_deref()
                            .and_then(rupu_transcript::final_answer)
                            .unwrap_or_default()
                    } else {
                        String::new()
                    }),
                    error: r.failure_reason(),
                    cause: terminal
                        .as_ref()
                        .and_then(|e| e.outcome())
                        .and_then(|o| serde_json::to_value(o).ok()),
                    tokens: Some(ChildTokens {
                        input: r.total_tokens_in,
                        output: r.total_tokens_out,
                    }),
                    at: None,
                }
            }
        };
        self.settle(&child, end, started);
    }

    /// Spawn a process child: its process line (after its `started` line,
    /// written here unless it was when the child queued) and the parent's
    /// `DispatchStarted`. Refused once this process is stopping
    /// ([`crate::reaper::stop_live_children`]).
    fn spawn_process(
        &self,
        child: &Child,
        launch: &ProcessLaunch,
        queued: bool,
    ) -> Result<Option<SpawnedProcess>, Refused> {
        let Some(_gate) = crate::reaper::spawn_gate() else {
            return Err(Refused::Stopping);
        };
        let process = self.env.backend.spawn(launch).map_err(Refused::Spawn)?;
        if !queued {
            self.record_launch(child);
        }
        self.record_spawn(child, process);
        self.emit(
            &child.parent_run,
            Event::DispatchStarted {
                run_id: child.parent_run.clone(),
                sub_run_id: child.run_id.clone(),
                kind: child.kind.as_str().into(),
                agent: Some(child.name.clone()),
                transcript_path: child.transcript.clone().unwrap_or_default(),
                codename: child.codename.clone(),
                provider: None,
                model: None,
            },
        );
        Ok(process)
    }

    /// Watch a spawned process child to its end; on a stop terminate its
    /// group (SIGKILL after [`TERM_GRACE`]). A child that ended for good
    /// leaves nothing below it to continue.
    async fn watch_process(self: Arc<Self>, child: Arc<Child>, process: Option<SpawnedProcess>) {
        let started = Instant::now();
        let backend = Arc::clone(&self.env.backend);
        let mut stopped_at: Option<Instant> = None;
        let state = loop {
            let poll = {
                let backend = Arc::clone(&backend);
                let run_id = child.run_id.clone();
                tokio::task::spawn_blocking(move || backend.poll(&run_id))
                    .await
                    .unwrap_or_else(|e| ProcState::Failed(format!("poll failed: {e}")))
            };
            match &poll {
                ProcState::Pending => {}
                ProcState::Running => self.set_state(&child, ChildState::Running),
                _ => break poll,
            }
            if let Some(at) = stopped_at {
                if at.elapsed() >= TERM_GRACE {
                    if let Some(p) = process {
                        crate::proc::kill_group(p.pid);
                    }
                }
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }
            tokio::select! {
                () = tokio::time::sleep(POLL_INTERVAL) => {}
                () = child.stop.cancelled() => {
                    let why = match lock(&child.st).stop_as {
                        Some((ChildState::Interrupted, ..)) => Stop::Interrupt,
                        _ => Stop::Abandon,
                    };
                    let backend = Arc::clone(&backend);
                    let run_id = child.run_id.clone();
                    let _ = tokio::task::spawn_blocking(move || backend.terminate(&run_id, why)).await;
                    stopped_at = Some(Instant::now());
                }
            }
        };
        let tokens = tokens_of(
            self.global(),
            child.kind,
            &child.run_id,
            child.transcript.as_deref(),
        );
        // A child that finished as it was stopped keeps its result.
        let end = match state {
            ProcState::Done(o) => End {
                state: ChildState::Done,
                success: Some(o.success),
                error: (!o.success)
                    .then(|| format!("{} {} did not succeed", child.kind.as_str(), child.name)),
                output: Some(o.output),
                cause: None,
                tokens,
                at: None,
            },
            ProcState::Failed(_) | ProcState::Died(_) if stopped_at.is_some() => End {
                tokens,
                ..End::stopped(&child, "its parent ended without joining it")
            },
            // Ended of its parent's process being stopped (killed outright,
            // or its run recorded the stop as it failed): continued with it.
            ProcState::Failed(why) | ProcState::Died(why) if crate::reaper::stopping() => End {
                tokens,
                ..End::interrupted(why)
            },
            ProcState::Failed(why) | ProcState::Died(why) => End {
                tokens,
                ..End::failed(why)
            },
            ProcState::Pending | ProcState::Running => {
                unreachable!("loop exits on a terminal state")
            }
        };
        let below_pending = end.state == ChildState::Failed
            && stopped_at.is_none()
            && self.stopped_from_outside(&started_of(&child)).await;
        let for_good = end.state != ChildState::Interrupted && !below_pending;
        lock(&child.st).below_pending = below_pending;
        self.settle(&child, end, started);
        if for_good {
            self.abandon_below(
                child.kind,
                &child.run_id,
                "its parent ended without continuing it",
            )
            .await;
        }
    }

    /// Watch `work`, `child`'s task: settle a child whose task panicked or
    /// was dropped without settling it.
    fn supervise(&self, child: &Arc<Child>, work: tokio::task::JoinHandle<()>) {
        *lock(&child.work) = Some(work.abort_handle());
        let me = self.arc();
        let watched = Arc::clone(child);
        let watcher = self.env.runtime.spawn(async move {
            let Err(e) = work.await else { return };
            if !e.is_panic() {
                me.settle(
                    &watched,
                    End::stopped(&watched, "it was stopped before it finished"),
                    watched.born,
                );
                return;
            }
            // Nothing watches its process any more: stop it before it is
            // reported over.
            if watched.kind != ChildKind::Subagent && lock(&watched.st).spawned {
                let backend = Arc::clone(&me.env.backend);
                let run_id = watched.run_id.clone();
                let _ =
                    tokio::task::spawn_blocking(move || backend.terminate(&run_id, Stop::Abandon))
                        .await;
            }
            // Its slot went with the task; a legacy waiter would otherwise
            // wait out `SYNC_WAIT` on a child that never settles.
            me.settle(
                &watched,
                End::failed("its task ended abnormally"),
                watched.born,
            );
            if watched.kind == ChildKind::Subagent {
                // Its run never reached its own `finish`.
                me.finish_run(&watched.run_id, ParentEnd::Stopped).await;
            }
        });
        *lock(&child.task) = Some(watcher);
    }

    /// Stop `run_id`'s unjoined children and record them `abandoned` — or
    /// `interrupted` when it paused or its process is being stopped by a
    /// signal, for the run that continues it ([`Launcher::finish`]). A
    /// sub-agent dropped after its grace never ran its own `finish`, so its
    /// children are stopped here too.
    fn finish_run<'a>(
        &'a self,
        run_id: &'a str,
        how: ParentEnd,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let mine = self.children_of(run_id);
            let live: Vec<Arc<Child>> = mine
                .iter()
                .filter(|c| !c.state().is_terminal())
                .cloned()
                .collect();
            let signalled = how != ParentEnd::Paused && crate::reaper::stopping();
            let why = if signalled {
                STOPPING.to_string()
            } else {
                format!("its parent run {} without joining it", how.as_str())
            };
            let (state, nested) = if how == ParentEnd::Paused || signalled {
                (ChildState::Interrupted, ParentEnd::Paused)
            } else {
                (ChildState::Abandoned, ParentEnd::Stopped)
            };
            let at = Utc::now();
            for c in &live {
                lock(&c.st).stop_as = Some((state, why.clone(), at));
                c.stop.cancel();
            }
            for c in &live {
                let task = lock(&c.task).take();
                let Some(mut task) = task else { continue };
                let grace = match c.kind {
                    ChildKind::Subagent => SUBAGENT_STOP_GRACE,
                    _ => TERM_GRACE + Duration::from_secs(3),
                };
                if tokio::time::timeout(grace, &mut task).await.is_err() {
                    self.settle(c, End::stopped(c, &why), c.born);
                    let work = lock(&c.work).take();
                    if let Some(work) = work {
                        work.abort();
                    }
                    task.abort();
                    if c.kind == ChildKind::Subagent {
                        self.finish_run(&c.run_id, nested).await;
                    }
                }
                self.settle(c, End::stopped(c, &why), c.born);
            }
            if state == ChildState::Abandoned {
                for c in mine.iter().filter(|c| lock(&c.st).below_pending) {
                    self.abandon_below(c.kind, &c.run_id, "its parent ended without continuing it")
                        .await;
                }
            }
            lock(&self.children).retain(|c| c.parent_run != run_id);
            lock(&self.slots).remove(run_id);
            if let Some(c) = mine.first() {
                crate::reaper::untrack(&c.ledger_dir);
            }
        })
    }

    /// The child `h` names, of the run `from` describes, recorded in its
    /// ledger with `launch`.
    fn new_child(&self, h: ChildHandle, from: &Caller, launch: LaunchRecord) -> Child {
        let id = &from.identity;
        Child {
            handle: h.handle,
            kind: h.kind,
            name: h.name,
            run_id: h.run_id,
            codename: h.codename,
            address: h.address,
            transcript: h.transcript,
            parent_run: id.run_id.clone(),
            ledger_dir: id
                .launch
                .children_dir
                .clone()
                .expect("a run that launches has a children dir"),
            depth: id.depth + 1,
            launch,
            born: Instant::now(),
            stop: CancellationToken::new(),
            task: Mutex::new(None),
            work: Mutex::new(None),
            st: Mutex::new(Inner::default()),
        }
    }

    /// Run `child` as `work` once it has one of its parent's slots. A process
    /// child with a free slot is spawned before this returns, so "started"
    /// means it exists; a refused spawn is the caller's error, and leaves no
    /// record. Any other child is recorded at once, queued or not.
    fn start(&self, child: Arc<Child>, work: Work, from: Caller) -> Result<(), LaunchError> {
        let stopping = crate::reaper::stop_token();
        let me = self.arc();
        let slot = self.slot(&child.parent_run);
        let task_child = Arc::clone(&child);
        let mut early = None;
        if let Work::Process(launch) = &work {
            if let Ok(permit) = Arc::clone(&slot).try_acquire_owned() {
                let process = self
                    .spawn_process(&child, launch, false)
                    .map_err(Refused::into_launch_error)?;
                early = Some((permit, process));
            }
        }
        if early.is_none() {
            self.record_launch(&child);
        }
        lock(&self.children).push(Arc::clone(&child));
        let task = self.env.runtime.spawn(async move {
            let queued = Instant::now();
            // One slot per running child, per parent (`[dispatch].max_concurrent_children`).
            let (_permit, process) = match early {
                Some((permit, process)) => (Some(permit), Some(process)),
                None => tokio::select! {
                    biased;
                    () = task_child.stop.cancelled() => {
                        return me.settle(
                            &task_child,
                            End::stopped(&task_child, "its parent ended before it started"),
                            queued,
                        );
                    }
                    () = stopping.cancelled() => {
                        return me.settle(&task_child, End::interrupted(STOPPING), queued);
                    }
                    p = slot.acquire_owned() => (p.ok(), None),
                },
            };
            match work {
                Work::Subagent {
                    spec,
                    prompt,
                    ceiling,
                    continues,
                } => {
                    Arc::clone(&me)
                        .run_subagent(
                            Arc::clone(&task_child),
                            *spec,
                            *prompt,
                            ceiling,
                            from,
                            continues,
                        )
                        .await
                }
                Work::Process(launch) => {
                    let process = match process {
                        Some(process) => process,
                        None => match me.spawn_process(&task_child, &launch, true) {
                            Ok(process) => process,
                            Err(Refused::Stopping) => {
                                return me.settle(&task_child, End::interrupted(STOPPING), queued)
                            }
                            Err(Refused::Spawn(e)) => {
                                return me.settle(
                                    &task_child,
                                    End::failed(format!("spawn failed: {e}")),
                                    queued,
                                )
                            }
                        },
                    };
                    Arc::clone(&me)
                        .watch_process(Arc::clone(&task_child), process)
                        .await
                }
            }
        });
        self.supervise(&child, task);
        Ok(())
    }

    /// Take on `prior`'s child as it ended: in memory and in the new
    /// parent's ledger, under its handle, collected again if `join` had
    /// collected it.
    fn restore(&self, prior: &Started, end: Finished, joined: bool, from: &Caller) {
        let launch = LaunchRecord {
            request: prior.request.clone(),
            continues: prior.continues.clone(),
            prior_tokens: prior.prior_tokens,
            tree: from.tree.clone(),
        };
        let mut child = self.new_child(handle_of(prior), from, launch);
        child.st = Mutex::new(Inner {
            state: Some(end.status),
            success: end.success,
            output: end.output.clone(),
            error: end.error.clone(),
            cause: end.cause.clone(),
            tokens: end.tokens,
            duration_ms: end.duration_ms,
            joined,
            spawned: false,
            stop_as: None,
            below_pending: false,
        });
        self.record_launch(&child);
        ledger::append(
            &child.ledger_dir,
            &LedgerLine::Finished(Finished {
                handle: child.handle.clone(),
                ..end
            }),
        );
        if joined {
            self.record_joined(&child);
        }
        lock(&self.children).push(Arc::new(child));
    }

    /// Take on `prior`'s child as its own records say it ended (its tokens
    /// with its earlier runs'), and abandon what it left below it: nothing
    /// will continue it.
    async fn restore_ended(&self, prior: &Started, mut end: Finished, from: &Caller) -> bool {
        end.tokens = plus(prior.prior_tokens, end.tokens);
        self.restore(prior, end, false, from);
        self.abandon_below(prior.kind, &prior.run_id, "its parent ended")
            .await;
        true
    }

    /// `prior`'s child, which its parent's pause or crash interrupted at
    /// `stopped_at`, taken on under the same handle. One its own records say
    /// ended (on its own: not of the stop) is restored with that end, without
    /// a model call; one that never started is launched afresh from its
    /// recorded request; the rest are continued from their records — an
    /// agent as `rupu run --continue`, a workflow as `rupu workflow resume`,
    /// a sub-agent in this process seeded from its transcript. One that can't
    /// be continued is restored `failed`, saying why. Returns whether it was
    /// taken on (a launch refused because this process is stopping leaves it
    /// for the next run).
    async fn continue_child(
        &self,
        prior: &Started,
        stopped_at: DateTime<Utc>,
        from: &Caller,
    ) -> bool {
        if prior.kind == ChildKind::Workflow {
            let (global, s) = (self.global().to_path_buf(), prior.clone());
            let own = tokio::task::spawn_blocking(move || own_end(&global, &s, Some(stopped_at)))
                .await
                .unwrap_or(Own::Unended);
            return match own {
                Own::Ended(end) => self.restore_ended(prior, end, from).await,
                Own::NotStarted => self.fresh(prior, from).await,
                Own::Unended => {
                    let (mode, link) = from.bounds(prior.kind, prior.request.as_deref());
                    // `workflow resume` runs under the link its run records.
                    if let Some(link) = &link {
                        if let Err(e) = self.env.store.write_launch_link(&prior.run_id, link) {
                            tracing::warn!(run = %prior.run_id, error = %e, "could not record a resumed workflow child's link");
                        }
                    }
                    let launch = LaunchRecord {
                        request: bounded(prior.request.as_deref(), mode, link),
                        continues: None,
                        // Its own ledger counts the resumed run's calls with
                        // the earlier ones.
                        prior_tokens: prior.prior_tokens,
                        tree: from.tree.clone(),
                    };
                    let child = self.continued(prior, prior.run_id.clone(), None, launch, from);
                    let work = Work::Process(Box::new(ProcessLaunch {
                        run_id: child.run_id.clone(),
                        kind: child.kind,
                        argv: RunArgv::WorkflowControl(WorkflowControl::Resume {
                            run_id: child.run_id.clone(),
                            mode: Some(mode),
                            if_unfinished: false,
                        }),
                        cwd: Some(from.workspace.path.clone()),
                    }));
                    self.start_continued(child, work, from)
                }
            };
        }

        use rupu_agent::continuation::Continuation;
        // A continuation that never started continues what it continued.
        let mut prior = prior.clone();
        let prepared = loop {
            match prepare(prior.transcript.clone()).await {
                Prepared::NotStarted => match prior.continues.take() {
                    Some(c) => {
                        prior = Started {
                            run_id: c.run_id,
                            transcript: c.transcript,
                            prior_tokens: c.prior_tokens,
                            ..prior
                        }
                    }
                    None => return self.fresh(&prior, from).await,
                },
                other => break other,
            }
        };
        let continuation = match prepared {
            Prepared::Ready(c) => *c,
            Prepared::Broken(e) => {
                return self
                    .restore_failed(&prior, format!("it can't be continued: {e}"), from)
                    .await
            }
            Prepared::NotStarted => unreachable!("taken care of above"),
        };
        let (messages, seed_source, loaded_skills) = match continuation {
            Continuation::Resume {
                messages,
                seed_source,
                loaded_skills,
                ..
            } => (messages, seed_source, loaded_skills),
            Continuation::Finished { output } => {
                let end = Finished {
                    success: Some(true),
                    output: Some(output),
                    tokens: tokens_of(
                        self.global(),
                        prior.kind,
                        &prior.run_id,
                        prior.transcript.as_deref(),
                    ),
                    ..Finished::new(&prior.handle, ChildState::Done, None)
                };
                return self.restore_ended(&prior, end, from).await;
            }
            Continuation::Failed { error, .. } => {
                let why = error.unwrap_or_else(|| "its run failed".into());
                return self.restore_failed(&prior, why, from).await;
            }
        };
        let (mode, link) = from.bounds(prior.kind, prior.request.as_deref());
        let launch = LaunchRecord {
            request: bounded(prior.request.as_deref(), mode, link.clone()),
            continues: Some(Box::new(PriorRun {
                run_id: prior.run_id.clone(),
                transcript: prior.transcript.clone(),
                prior_tokens: prior.prior_tokens,
            })),
            prior_tokens: plus(
                prior.prior_tokens,
                tokens_of(
                    self.global(),
                    prior.kind,
                    &prior.run_id,
                    prior.transcript.as_deref(),
                ),
            ),
            tree: from.tree.clone(),
        };
        match prior.kind {
            ChildKind::Subagent => {
                let spec = match self.load_agent(&prior.name).await {
                    Ok(spec) => spec,
                    Err(why) => return self.restore_failed(&prior, why, from).await,
                };
                let (sub, transcript) = match self
                    .env
                    .store
                    .create_sub_run(&from.identity.run_id, &prior.name)
                {
                    Ok(created) => created,
                    Err(e) => {
                        let why = format!("it can't be continued: {e}");
                        return self.restore_failed(&prior, why, from).await;
                    }
                };
                let mut prompt = UserTurn::new(String::new());
                rupu_agent::continuation::apply_continuation(
                    &mut prompt,
                    messages,
                    seed_source,
                    loaded_skills,
                );
                let child = self.continued(&prior, sub, Some(transcript), launch, from);
                let work = Work::Subagent {
                    spec: Box::new(spec),
                    prompt: Box::new(prompt),
                    ceiling: mode,
                    continues: Some(prior.run_id.clone()),
                };
                self.start_continued(child, work, from)
            }
            _ => {
                let run_id = format!("run_{}", ulid::Ulid::new());
                let transcript = transcripts_dir(self.global(), Some(&from.workspace.path))
                    .join(format!("{run_id}.jsonl"));
                let child = self.continued(&prior, run_id, Some(transcript), launch, from);
                let mut run = AgentRun::new(&prior.name, &child.run_id);
                run.codename = child.codename.clone();
                run.mode = Some(mode);
                run.continue_from = Some(prior.run_id.clone());
                run.engagement_profiles = from.engagement.clone();
                run.message_space = self.message_space(&child, from);
                run.launch = link;
                let work = Work::Process(Box::new(ProcessLaunch {
                    run_id: child.run_id.clone(),
                    kind: child.kind,
                    argv: RunArgv::Agent(run),
                    cwd: Some(from.workspace.path.clone()),
                }));
                self.start_continued(child, work, from)
            }
        }
    }

    /// Whether `s`'s own records say a signal or its runner's death ended
    /// it ([`crate::identity::own_stop`]), off the runtime.
    async fn stopped_from_outside(&self, s: &Started) -> bool {
        let (global, s) = (self.global().to_path_buf(), s.clone());
        tokio::task::spawn_blocking(move || crate::identity::own_stop(&global, &s).is_some())
            .await
            .unwrap_or(false)
    }

    async fn restore_failed(&self, prior: &Started, why: String, from: &Caller) -> bool {
        let end = Finished {
            tokens: tokens_of(
                self.global(),
                prior.kind,
                &prior.run_id,
                prior.transcript.as_deref(),
            ),
            ..Finished::new(&prior.handle, ChildState::Failed, Some(why))
        };
        self.restore_ended(prior, end, from).await
    }

    /// `prior`'s child, which never started, launched afresh under the same
    /// handle from the request it recorded.
    async fn fresh(&self, prior: &Started, from: &Caller) -> bool {
        let Some(request) = prior.request.clone() else {
            let why = "it never started, and how it was launched was not recorded".to_string();
            return self.restore_failed(prior, why, from).await;
        };
        let (mode, link) = from.bounds(prior.kind, Some(&request));
        let launch = LaunchRecord {
            request: bounded(Some(&request), mode, link.clone()),
            continues: None,
            prior_tokens: prior.prior_tokens,
            tree: from.tree.clone(),
        };
        let missing = |what: &str| format!("it never started, and its {what} was not recorded");
        match prior.kind {
            ChildKind::Subagent => {
                let spec = match self.load_agent(&prior.name).await {
                    Ok(spec) => spec,
                    Err(why) => return self.restore_failed(prior, why, from).await,
                };
                let (sub, transcript) = match self
                    .env
                    .store
                    .create_sub_run(&from.identity.run_id, &prior.name)
                {
                    Ok(created) => created,
                    Err(e) => {
                        let why = format!("it can't be started again: {e}");
                        return self.restore_failed(prior, why, from).await;
                    }
                };
                let child = self.continued(prior, sub, Some(transcript), launch, from);
                let work = Work::Subagent {
                    spec: Box::new(spec),
                    prompt: Box::new(UserTurn::new(request.prompt.unwrap_or_default())),
                    ceiling: mode,
                    continues: None,
                };
                self.start_continued(child, work, from)
            }
            ChildKind::Agent => {
                let Some(prompt) = request.prompt else {
                    return self.restore_failed(prior, missing("prompt"), from).await;
                };
                let run_id = format!("run_{}", ulid::Ulid::new());
                let transcript = transcripts_dir(self.global(), Some(&from.workspace.path))
                    .join(format!("{run_id}.jsonl"));
                let child = self.continued(prior, run_id, Some(transcript), launch, from);
                let mut run = AgentRun::new(&prior.name, &child.run_id);
                run.codename = child.codename.clone();
                run.mode = Some(mode);
                run.prompt = Some(prompt);
                run.engagement_profiles = from.engagement.clone();
                run.message_space = self.message_space(&child, from);
                run.launch = link;
                let work = Work::Process(Box::new(ProcessLaunch {
                    run_id: child.run_id.clone(),
                    kind: child.kind,
                    argv: RunArgv::Agent(run),
                    cwd: Some(from.workspace.path.clone()),
                }));
                self.start_continued(child, work, from)
            }
            ChildKind::Workflow => {
                let Some(file) = request.workflow else {
                    return self.restore_failed(prior, missing("workflow"), from).await;
                };
                let run_id = format!("run_{}", ulid::Ulid::new());
                let child = self.continued(prior, run_id, None, launch, from);
                let mut run = WorkflowRun::new(WorkflowRef::File(file), &child.run_id);
                run.workflow_name = Some(child.name.clone());
                run.codename = child.codename.clone();
                run.mode = Some(mode);
                run.plain = true;
                run.inputs = request.inputs;
                run.engagement_profiles = from.engagement.clone();
                run.flow = self.flow_attach(&child, from);
                run.launch = link;
                let work = Work::Process(Box::new(ProcessLaunch {
                    run_id: child.run_id.clone(),
                    kind: child.kind,
                    argv: RunArgv::Workflow(run),
                    cwd: Some(from.workspace.path.clone()),
                }));
                self.start_continued(child, work, from)
            }
        }
    }

    async fn load_agent(&self, name: &str) -> Result<rupu_agent::AgentSpec, String> {
        let project = self.project_rupu();
        rupu_agent::load_agent_admitted(self.global(), project.as_deref(), name)
            .await
            .map_err(|e| format!("load agent `{name}`: {e}"))
    }

    /// The new run of `prior`'s child: same handle, name, codename and
    /// address, under the parent `from` describes.
    fn continued(
        &self,
        prior: &Started,
        run_id: String,
        transcript: Option<PathBuf>,
        launch: LaunchRecord,
        from: &Caller,
    ) -> Child {
        self.new_child(
            ChildHandle {
                run_id,
                transcript,
                ..handle_of(prior)
            },
            from,
            launch,
        )
    }

    fn start_continued(&self, child: Child, work: Work, from: &Caller) -> bool {
        let handle = child.handle.clone();
        match self.start(Arc::new(child), work, from.clone()) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(handle = %handle, error = %e, "could not continue a child");
                false
            }
        }
    }

    /// `child`'s collection by `join`, recorded so a run that continues its
    /// parent's conversation knows it.
    fn record_joined(&self, child: &Child) {
        ledger::append(
            &child.ledger_dir,
            &LedgerLine::Joined(Joined {
                handle: child.handle.clone(),
                at: chrono::Utc::now(),
            }),
        );
    }

    /// Record the children `run_id` left unfinished or interrupted
    /// `abandoned` with `why`, stopping any still running (off the runtime).
    async fn abandon_in(&self, run_id: &str, why: &str) {
        let dir = self.global().join("runs").join(run_id);
        let why = why.to_string();
        let done = tokio::task::spawn_blocking(move || {
            crate::reaper::abandon_unfinished(&dir, &why, TERM_GRACE)
        })
        .await;
        if let Err(e) = done {
            tracing::warn!(run = run_id, error = %e, "could not abandon a run's children");
        }
    }

    /// Record what `kind` child `run_id` left below it without a final end
    /// `abandoned` with `why` (off the runtime): nothing will continue it.
    async fn abandon_below(&self, kind: ChildKind, run_id: &str, why: &str) {
        let (global, run, why) = (
            self.global().to_path_buf(),
            run_id.to_string(),
            why.to_string(),
        );
        let done = tokio::task::spawn_blocking(move || {
            crate::reaper::abandon_below(&global, kind, &run, &why, TERM_GRACE)
        })
        .await;
        if let Err(e) = done {
            tracing::warn!(run = run_id, error = %e, "could not abandon a child's children");
        }
    }

    /// A process agent child's place in its parent's message space: the
    /// space of the parent's bus, else (a caller in a flow without one) the
    /// flow's.
    fn message_space(&self, child: &Child, from: &Caller) -> Option<MessageSpace> {
        let dir = match &from.bus {
            Some(bus) => bus.space().to_path_buf(),
            None => from.identity.launch.flow.clone()?,
        };
        Some(MessageSpace {
            dir,
            participant: child.address.clone()?,
        })
    }

    /// A workflow child's place on its parent's agentiflow board (its steps
    /// report into the flow's findings scope; the workflow is its own
    /// message space).
    fn flow_attach(&self, child: &Child, from: &Caller) -> Option<FlowAttach> {
        from.identity.launch.flow.as_ref().map(|dir| FlowAttach {
            run_dir: dir.clone(),
            participant: child.address.clone().unwrap_or_else(|| child.name.clone()),
        })
    }

    /// The argv of a process child (W6's typed argv, with W7's link).
    fn process_argv(&self, vetted: &Vetted, child: &Child, from: &Caller) -> RunArgv {
        match vetted {
            Vetted::Agent { name, prompt, .. } => {
                let mut run = AgentRun::new(name, &child.run_id);
                run.codename = child.codename.clone();
                run.mode = Some(from.process_ceiling());
                run.prompt = Some(prompt.clone());
                run.engagement_profiles = from.engagement.clone();
                run.message_space = self.message_space(child, from);
                run.launch = Some(from.link());
                RunArgv::Agent(run)
            }
            Vetted::Workflow { target, inputs, .. } => {
                let mut run = WorkflowRun::new(target.clone(), &child.run_id);
                // The name its parent vetted and recorded it under, not the
                // file's own `name:`.
                run.workflow_name = Some(child.name.clone());
                run.codename = child.codename.clone();
                run.mode = Some(from.process_ceiling());
                // Detached with no tty: the plain printer, never the live view.
                run.plain = true;
                run.inputs = inputs.clone();
                run.engagement_profiles = from.engagement.clone();
                run.flow = self.flow_attach(child, from);
                run.launch = Some(from.link());
                RunArgv::Workflow(run)
            }
        }
    }

    /// The generation provider: the site's, else resolved once per caller
    /// provider/model from it and the credentials on hand.
    async fn generation(&self, from: &ToolContext) -> Option<Generation> {
        if let Some(g) = &self.env.generation {
            return Some(g.clone());
        }
        self.generation
            .get_or_resolve(GenerationKey::of(&from.identity), |key| async move {
                let ctx = self.env.assembler.context();
                let sink = from
                    .services
                    .netflow_sink
                    .clone()
                    .unwrap_or_else(|| Arc::new(rupu_netflow::NullSink));
                Generation::resolve(
                    ctx.resolver.clone(),
                    &ctx.config,
                    &key.provider,
                    &key.model,
                    sink,
                )
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "workflows.generate is unavailable: the generation provider did not build");
                    None
                })
            })
            .await
    }
}

/// `request` as a child launched with it now runs: at `mode`, under `link`.
fn bounded(
    request: Option<&Request>,
    mode: PermissionMode,
    link: Option<LaunchLink>,
) -> Option<Box<Request>> {
    request.map(|r| {
        Box::new(Request {
            mode,
            link,
            ..r.clone()
        })
    })
}

/// What an agent child's transcript says about continuing it.
enum Prepared {
    Ready(Box<rupu_agent::continuation::Continuation>),
    /// No transcript, or none of a run that got as far as its first turn.
    NotStarted,
    Broken(String),
}

async fn prepare(transcript: Option<PathBuf>) -> Prepared {
    use rupu_agent::continuation::{prepare_continuation, ContinuationError};
    let Some(transcript) = transcript.filter(|t| t.is_file()) else {
        return Prepared::NotStarted;
    };
    match tokio::task::spawn_blocking(move || prepare_continuation(&transcript)).await {
        Ok(Ok(c)) => Prepared::Ready(Box::new(c)),
        Ok(Err(ContinuationError::NotAnAgentRun { .. })) => Prepared::NotStarted,
        Ok(Err(e)) => Prepared::Broken(e.to_string()),
        Err(e) => Prepared::Broken(e.to_string()),
    }
}

/// The handle a ledger recorded a child under.
fn handle_of(s: &Started) -> ChildHandle {
    ChildHandle {
        handle: s.handle.clone(),
        kind: s.kind,
        name: s.name.clone(),
        run_id: s.run_id.clone(),
        codename: s.codename.clone(),
        address: s.address.clone(),
        transcript: s.transcript.clone(),
    }
}

/// Why a generated workflow may not run for `id`: an `action:` step calls a
/// connector tool `id`'s own grant doesn't offer. Model output reaches no
/// connector its author couldn't call itself; other actions (findings) are
/// no connector.
fn connector_actions_not_granted(wf: &Workflow, id: &RunIdentity) -> Option<String> {
    let outside: Vec<String> = wf
        .action_tools()
        .into_iter()
        .filter(|a| {
            rupu_orchestrator::workflow::action_tool(a).is_some_and(|d| {
                d.is_connector() && !id.launch.granted_connectors.iter().any(|g| g == d.name)
            })
        })
        .collect();
    (!outside.is_empty()).then(|| {
        format!(
            "workflow '{}' calls connector tools this run's grant doesn't offer: {}",
            wf.name,
            outside.join(", ")
        )
    })
}

#[async_trait]
impl Launcher for RunLauncher {
    async fn check(&self, req: &ChildRequest, from: &ToolContext) -> Result<(), LaunchError> {
        self.vet(req, from).await.map(|_| ())
    }

    async fn launch(
        &self,
        req: ChildRequest,
        ctx: &ToolContext,
    ) -> Result<ChildHandle, LaunchError> {
        let stopping = crate::reaper::stop_token();
        if stopping.is_cancelled() {
            return Err(Refused::Stopping.into_launch_error());
        }
        let vetted = self.vet(&req, ctx).await?;
        let from = Caller::of(ctx);
        let id = &from.identity;
        let name = vetted.name().to_string();
        let codename = self.mint_codename(id.codename.as_deref(), &name);
        // A child sharing its parent's message space is addressed by its
        // codename below the parent's (spec F1a §3.1); a workflow child of a
        // flow keeps the address its flow records it under.
        let address = match req.kind {
            ChildKind::Workflow => id
                .launch
                .flow
                .is_some()
                .then(|| self.address(codename.as_deref(), &name)),
            _ => match &from.bus {
                Some(bus) => Some(
                    codename
                        .as_deref()
                        .and_then(|c| bus.address_below(c))
                        .unwrap_or_else(|| self.address(None, &name)),
                ),
                None => (req.kind != ChildKind::Subagent && id.launch.flow.is_some())
                    .then(|| self.address(codename.as_deref(), &name)),
            },
        };
        let (run_id, transcript) = match req.kind {
            ChildKind::Subagent => {
                let (sub, transcript) = self
                    .env
                    .store
                    .create_sub_run(&id.run_id, &name)
                    .map_err(|e| LaunchError::new("run_store", e.to_string()))?;
                (sub, Some(transcript))
            }
            ChildKind::Agent => {
                let run_id = format!("run_{}", ulid::Ulid::new());
                let dir = transcripts_dir(self.global(), Some(&from.workspace.path));
                let transcript = dir.join(format!("{run_id}.jsonl"));
                (run_id, Some(transcript))
            }
            ChildKind::Workflow => (format!("run_{}", ulid::Ulid::new()), None),
        };
        // A child sharing the parent's space (a workflow child is its own):
        // its broadcast cursor starts at the space's current tip before it
        // exists, so nothing broadcast before its first turn is swallowed as
        // backlog.
        if let (Some(bus), Some(address), false) =
            (&from.bus, &address, req.kind == ChildKind::Workflow)
        {
            if let Err(e) = bus.mailbox.register_broadcast_cursor(address) {
                tracing::warn!(participant = %address, error = %e, "could not register the child's broadcast cursor");
            }
        }
        let handle = ChildHandle {
            handle: format!("ch_{}", ulid::Ulid::new()),
            kind: req.kind,
            name,
            run_id,
            codename,
            address,
            transcript,
        };
        let (mode, link) = from.bounds(req.kind, None);
        let request = match &vetted {
            Vetted::Agent { prompt, .. } => Request {
                mode,
                link,
                prompt: Some(prompt.clone()),
                workflow: None,
                inputs: BTreeMap::new(),
            },
            Vetted::Workflow { target, inputs, .. } => Request {
                mode,
                link,
                prompt: None,
                workflow: match target {
                    WorkflowRef::File(f) => Some(f.clone()),
                    WorkflowRef::Name(_) => None,
                },
                inputs: inputs.clone(),
            },
        };
        let launch = LaunchRecord {
            request: Some(Box::new(request)),
            tree: from.tree.clone(),
            ..LaunchRecord::default()
        };
        let child = self.new_child(handle.clone(), &from, launch);
        let work = match (req.kind, vetted) {
            (
                ChildKind::Subagent,
                Vetted::Agent {
                    spec: Some(spec),
                    prompt,
                    ..
                },
            ) => Work::Subagent {
                spec,
                prompt: Box::new(UserTurn::new(prompt)),
                ceiling: mode,
                continues: None,
            },
            (ChildKind::Subagent, _) => {
                unreachable!("a sub-agent's spec is loaded when it is vetted")
            }
            (_, vetted) => Work::Process(Box::new(ProcessLaunch {
                run_id: child.run_id.clone(),
                kind: child.kind,
                argv: self.process_argv(&vetted, &child, &from),
                cwd: Some(from.workspace.path.clone()),
            })),
        };
        self.start(Arc::new(child), work, from)?;
        Ok(handle)
    }

    async fn join(
        &self,
        from: &RunIdentity,
        which: JoinSet,
        timeout: Duration,
        until: Until,
    ) -> Vec<ChildStatus> {
        let mine = self.children_of(&from.run_id);
        let picked: Vec<Result<Arc<Child>, String>> = match which {
            JoinSet::All => mine
                .into_iter()
                .filter(|c| !lock(&c.st).joined)
                .map(Ok)
                .collect(),
            JoinSet::Handles(handles) => handles
                .into_iter()
                .map(|h| mine.iter().find(|c| c.handle == h).cloned().ok_or(h))
                .collect(),
        };
        let deadline = Instant::now().checked_add(timeout);
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let known: Vec<&Arc<Child>> = picked.iter().filter_map(|c| c.as_ref().ok()).collect();
            let done = known.iter().filter(|c| c.state().is_terminal()).count();
            let satisfied = match until {
                Until::All => done == known.len(),
                Until::Any => done > 0 || known.is_empty(),
            };
            if satisfied || timeout.is_zero() {
                break;
            }
            match deadline {
                Some(d) if Instant::now() >= d => break,
                Some(d) => {
                    let _ = tokio::time::timeout_at(d.into(), notified).await;
                }
                None => notified.await,
            }
        }
        picked
            .into_iter()
            .map(|c| match c {
                Ok(c) => {
                    let status = c.status();
                    let first = status.state.is_terminal()
                        && !std::mem::replace(&mut lock(&c.st).joined, true);
                    if first {
                        self.record_joined(&c);
                    }
                    status
                }
                Err(handle) => ChildStatus {
                    handle,
                    kind: ChildKind::Subagent,
                    name: String::new(),
                    run_id: String::new(),
                    codename: None,
                    address: None,
                    transcript: None,
                    state: ChildState::Failed,
                    success: None,
                    output: None,
                    error: Some("unknown handle".into()),
                    cause: None,
                    tokens: None,
                    duration_ms: None,
                    elapsed: Duration::ZERO,
                },
            })
            .collect()
    }

    async fn finish(&self, from: &RunIdentity, how: ParentEnd) {
        self.finish_run(&from.run_id, how).await
    }

    fn children(&self, from: &RunIdentity) -> Vec<ChildStatus> {
        self.children_of(&from.run_id)
            .into_iter()
            .filter(|c| !lock(&c.st).joined)
            .map(|c| c.status())
            .collect()
    }

    async fn continue_children(&self, prior_run: &str, to: &ToolContext) {
        let Some(by_dir) = to.identity.launch.children_dir.clone() else {
            return;
        };
        let from = Caller::of(to);
        let dir = self.global().join("runs").join(prior_run);
        // A crashed attempt recorded nothing: what it left running is
        // stopped (identity-checked) and recorded interrupted first.
        let entries = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || crate::reaper::interrupt_orphans(&dir, TERM_GRACE))
                .await
                .unwrap_or_default()
        };
        let have: Vec<String> = self
            .children_of(&to.identity.run_id)
            .iter()
            .map(|c| c.handle.clone())
            .collect();
        for ChildEntry {
            started,
            finished,
            joined,
            continued_by,
            ..
        } in entries
        {
            if continued_by.is_some() || have.contains(&started.handle) {
                continue;
            }
            let Some(end) = finished else {
                tracing::warn!(
                    handle = %started.handle,
                    run = prior_run,
                    "a child's launching process is still running; it is not continued"
                );
                continue;
            };
            // Claimed before it is taken on, so of two runs continuing
            // this one, one does.
            let claimed = {
                let (dir, handle, by_run, by_dir) = (
                    dir.clone(),
                    started.handle.clone(),
                    to.identity.run_id.clone(),
                    by_dir.clone(),
                );
                tokio::task::spawn_blocking(move || ledger::claim(&dir, &handle, &by_run, &by_dir))
                    .await
                    .unwrap_or(false)
            };
            if !claimed {
                continue;
            }
            let taken = match end {
                end if end.status == ChildState::Interrupted => {
                    self.continue_child(&started, end.at, &from).await
                }
                // Recorded failed before its parent recorded the stop that
                // ended it too (a reboot signals every process at once),
                // even if the parent collected that end before it stopped.
                end if end.status == ChildState::Failed
                    && self.stopped_from_outside(&started).await =>
                {
                    self.continue_child(&started, end.at, &from).await
                }
                end => {
                    self.restore(&started, end, joined, &from);
                    true
                }
            };
            if taken {
                ledger::append(
                    &dir,
                    &LedgerLine::Continued(Continued {
                        handle: started.handle.clone(),
                        by_run: to.identity.run_id.clone(),
                        at: Utc::now(),
                    }),
                );
            } else {
                ledger::release(&dir, &started.handle, &to.identity.run_id);
            }
        }
    }

    async fn abandon_children(&self, prior_run: &str, why: &str) {
        self.abandon_in(prior_run, why).await
    }

    async fn end_interrupted(&self, workflow_run: &str) {
        let (global, run) = (self.global().to_path_buf(), workflow_run.to_string());
        let done = tokio::task::spawn_blocking(move || {
            crate::reaper::end_interrupted_of(&global, &run, TERM_GRACE)
        })
        .await;
        if let Err(e) = done {
            tracing::warn!(run = workflow_run, error = %e, "could not end a finished run's interrupted children");
        }
    }
}

/// Appended to the description so the generator knows the workflow runs
/// unattended in one local process.
const GENERATE_CONSTRAINT: &str = "\n\nConstraints: the workflow runs unattended as a single \
local process. Do NOT use approval gates (`approval:`), `host:`, or `distribute:`.";

/// A filename-safe form of a workflow name: lowercase ASCII alphanumerics,
/// with every other run of characters collapsed to one `-`. Never empty.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(48).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "workflow".to_string()
    } else {
        out
    }
}

#[async_trait]
impl WorkflowGenerator for RunLauncher {
    /// Author a workflow with a model and save it under the caller's run
    /// directory (`<children dir>/generated/<name>-<ulid>.yaml`). A generated
    /// workflow is model output, so it is vetted fail-closed before it is
    /// written: the caller's depth, a workflow that parses, no approval gate
    /// or remote placement, and only agents the caller may dispatch.
    async fn generate(
        &self,
        description: &str,
        from: &ToolContext,
    ) -> Result<GeneratedWorkflow, LaunchError> {
        let id = &from.identity;
        if id.depth >= MAX_LAUNCH_DEPTH {
            return Err(LaunchError::new(
                "max_dispatch_depth_exceeded",
                format!("workflows.generate refused: maximum dispatch depth ({MAX_LAUNCH_DEPTH}) reached"),
            ));
        }
        let Some(dir) = id.launch.children_dir.clone() else {
            return Err(LaunchError::new(
                "no_parent_run_id",
                "this run has no run directory to save a workflow under",
            ));
        };
        let Some(generation) = self.generation(from).await else {
            return Err(LaunchError::new(
                "generation_unavailable",
                "workflow generation is unavailable: no credential for a generating provider",
            ));
        };
        let req = rupu_orchestrator::generate::GenerateRequest {
            kind: rupu_orchestrator::generate::GenKind::Workflow,
            description: format!("{description}{GENERATE_CONSTRAINT}"),
            provider: generation.provider.clone(),
            model: generation.model.clone(),
            available_agents: id.launch.agents.clone(),
            available_skills: rupu_orchestrator::generate::installed_skill_names(
                self.global(),
                self.project_rupu().as_deref(),
            ),
        };
        let mut provider = (generation.factory)();
        let outcome =
            rupu_orchestrator::generate::generate_definition_with_provider(&req, provider.as_mut())
                .await
                .map_err(|e| {
                    LaunchError::new(
                        "generation_failed",
                        format!("workflow generation failed: {e}"),
                    )
                })?;
        let wf = Workflow::parse(&outcome.content).map_err(|e| {
            LaunchError::new(
                "generation_failed",
                format!("generated workflow did not parse: {e}"),
            )
        })?;
        if wf.has_approval_gate() || wf.has_placed_step() {
            return Err(LaunchError::new(
                "generation_failed",
                "generated workflow cannot run as a child: it uses an approval gate or \
                 host/distribute placement",
            ));
        }
        let missing: Vec<String> = wf
            .dispatched_agents()
            .into_iter()
            .filter(|a| !id.launch.agents.contains(a))
            .collect();
        if !missing.is_empty() {
            let place = if id.launch.pool.is_some() {
                "not in this flow's pool"
            } else {
                "you may not dispatch"
            };
            return Err(LaunchError::new(
                "generation_failed",
                format!("generated workflow dispatches agents {place}: {missing:?}"),
            ));
        }
        if let Some(why) = connector_actions_not_granted(&wf, id) {
            return Err(LaunchError::new("generation_failed", why));
        }
        let dir = dir.join("generated");
        std::fs::create_dir_all(&dir)
            .map_err(|e| LaunchError::new("io", format!("could not create generated/ dir: {e}")))?;
        let file = dir.join(format!("{}-{}.yaml", slug(&wf.name), ulid::Ulid::new()));
        std::fs::write(&file, &outcome.content).map_err(|e| {
            LaunchError::new("io", format!("could not write generated workflow: {e}"))
        })?;
        Ok(GeneratedWorkflow {
            file,
            name: wf.name.clone(),
            steps: wf.steps.len(),
            input_keys: wf.inputs.keys().cloned().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `workflows.generate`'s provider is resolved once per caller provider
    /// and model: a caller on another model gets its own slot, one on the
    /// same model reuses the first's.
    #[tokio::test]
    async fn generation_is_resolved_once_per_caller_model() {
        let global = tempfile::tempdir().unwrap();
        // No credentials under this home: every resolution comes back `None`.
        let context = rupu_runtime::assembly::AssemblyContext::minimal(global.path().to_path_buf());
        let l = RunLauncher::new(LaunchEnv {
            assembler: Arc::new(RunAssembler::new(context)),
            store: Arc::new(RunStore::new(global.path().join("runs"))),
            events: None,
            namer: None,
            backend: Arc::new(crate::testing::ScriptedProcesses::done("x")),
            runtime: tokio::runtime::Handle::current(),
            max_concurrent: 1,
            generation: None,
            project: None,
        });
        let caller = |model: &str| {
            let mut ctx = ToolContext::in_workspace(global.path());
            let id = ctx.identity_mut();
            id.provider = "anthropic".into();
            id.model = model.into();
            ctx
        };
        let slots = || {
            let mut keys: Vec<String> = lock(&l.generation.0)
                .keys()
                .map(|k| format!("{}/{}", k.provider, k.model))
                .collect();
            keys.sort();
            keys
        };

        assert!(l.generation(&caller("a")).await.is_none());
        assert!(l.generation(&caller("b")).await.is_none());
        assert_eq!(slots(), ["anthropic/a", "anthropic/b"], "one each");
        assert!(l.generation(&caller("a")).await.is_none());
        assert_eq!(
            slots(),
            ["anthropic/a", "anthropic/b"],
            "the first's, reused"
        );
    }

    #[test]
    fn slug_is_filename_safe_and_never_empty() {
        assert_eq!(slug("Web Assess: v2!"), "web-assess-v2");
        assert_eq!(slug("***"), "workflow");
        assert_eq!(slug(&"a".repeat(80)).len(), 48);
    }
}
