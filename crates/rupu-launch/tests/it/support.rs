//! Fixtures: a throwaway `RUPU_HOME` with agents and workflows, a launcher
//! over it (sub-agents through the real assembler and agent loop; process
//! children through a scripted backend), the dispatching parent's context,
//! an event recorder, and the `RUPU_MOCK_PROVIDER_SCRIPT` seam (callers are
//! `#[serial]`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rupu_launch::{LaunchEnv, ProcessBackend, RunLauncher};
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::RunStore;
use rupu_runtime::assembly::{AssemblyContext, RunAssembler, WorkspaceBinding};
use rupu_tools::{
    ChildKind, ChildRequest, ChildStatus, ChildTarget, JoinSet, LaunchScope, Launcher,
    PermissionMode, RunIdentity, ToolContext, Until,
};

/// A one-turn answer.
pub const DONE: &str = r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#;

/// Sets `RUPU_MOCK_PROVIDER_SCRIPT` for its life (cleared on drop).
pub struct MockScript;

impl MockScript {
    pub fn set(script: &str) -> Self {
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", script);
        MockScript
    }
}

impl Drop for MockScript {
    fn drop(&mut self) {
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    }
}

pub struct Home {
    pub dir: tempfile::TempDir,
}

impl Home {
    pub fn new() -> Self {
        let home = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        for d in ["agents", "workflows", "runs"] {
            std::fs::create_dir_all(home.global().join(d)).unwrap();
        }
        std::fs::create_dir_all(home.workspace()).unwrap();
        home
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn global(&self) -> PathBuf {
        self.dir.path().join("global")
    }

    pub fn workspace(&self) -> PathBuf {
        self.dir.path().join("workspace")
    }

    pub fn runs(&self) -> PathBuf {
        self.global().join("runs")
    }

    /// Write `agents/<name>.md` with `frontmatter` (YAML lines) and a prompt.
    pub fn agent(&self, name: &str, frontmatter: &str) {
        std::fs::write(
            self.global().join("agents").join(format!("{name}.md")),
            format!("---\nname: {name}\n{frontmatter}\n---\nyou are {name}.\n"),
        )
        .unwrap();
    }

    /// Install `<global>/skills/<name>` with a reference file.
    pub fn skill(&self, name: &str, description: &str) -> PathBuf {
        let d = self.global().join("skills").join(name);
        std::fs::create_dir_all(d.join("references")).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nx\n"),
        )
        .unwrap();
        std::fs::write(d.join("references/REF.md"), "reference text\n").unwrap();
        d
    }

    /// Write `workflows/<name>.yaml`.
    pub fn workflow(&self, name: &str, yaml: &str) {
        std::fs::write(
            self.global().join("workflows").join(format!("{name}.yaml")),
            yaml,
        )
        .unwrap();
    }

    pub fn binding(&self) -> WorkspaceBinding {
        WorkspaceBinding {
            id: "ws_test".into(),
            path: self.workspace(),
        }
    }

    pub fn context(&self, config: rupu_config::Config) -> AssemblyContext {
        AssemblyContext {
            global: self.global(),
            project_root: None,
            config,
            customer: None,
            resolver: Arc::new(rupu_auth::KeychainResolver::new()),
            scm: Some(Arc::new(rupu_scm::Registry::default())),
            findings: rupu_coverage::FindingWriteOptions::default(),
            net_capture: None,
            catalog: None,
        }
    }

    pub fn assembler(&self, config: rupu_config::Config) -> Arc<RunAssembler> {
        Arc::new(RunAssembler::new(self.context(config)))
    }

    /// A launcher over `assembler`, with `backend` for process children.
    pub fn launcher_with(
        &self,
        assembler: Arc<RunAssembler>,
        backend: Arc<dyn ProcessBackend>,
        events: Option<Arc<Recorder>>,
    ) -> Arc<RunLauncher> {
        RunLauncher::new(LaunchEnv {
            assembler,
            store: Arc::new(RunStore::new(self.runs())),
            events: events.map(|e| e as Arc<dyn EventSink>),
            namer: None,
            backend,
            runtime: tokio::runtime::Handle::current(),
            max_concurrent: 8,
            generation: None,
            project: None,
        })
    }

    pub fn launcher(
        &self,
        config: rupu_config::Config,
        events: Option<Arc<Recorder>>,
    ) -> Arc<RunLauncher> {
        self.launcher_with(
            self.assembler(config),
            Arc::new(rupu_launch::testing::ScriptedProcesses::done(
                "process done",
            )),
            events,
        )
    }

    /// The dispatching parent's context: a top-level `rupu run`
    /// (`parent_run_1`) that may launch `agents`, at `ceiling`, charging
    /// `<runs>/parent_run_1/usage.jsonl`.
    pub fn parent(
        &self,
        codename: Option<&str>,
        agents: &[&str],
        ceiling: PermissionMode,
    ) -> ToolContext {
        let mut ctx = ToolContext::in_workspace(self.workspace());
        ctx.workspace.id = "ws_test".into();
        *ctx.identity_mut() = RunIdentity {
            run_id: "parent_run_1".into(),
            codename: codename.map(str::to_string),
            agent: "parent".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            launch: LaunchScope {
                agents: agents.iter().map(|s| s.to_string()).collect(),
                children_dir: Some(self.runs().join("parent_run_1")),
                usage_root: Some(self.runs().join("parent_run_1").join("usage.jsonl")),
                ..Default::default()
            },
            ..Default::default()
        };
        ctx.call.spawn_ceiling = Some(ceiling);
        ctx
    }
}

/// A config whose default provider is the built-in `anthropic`.
pub fn config() -> rupu_config::Config {
    rupu_config::Config {
        default_provider: Some("anthropic".into()),
        default_model: Some("claude-sonnet-4-6".into()),
        ..Default::default()
    }
}

pub fn request(kind: ChildKind, name: &str, prompt: Option<&str>) -> ChildRequest {
    ChildRequest {
        kind,
        target: ChildTarget::Name(name.into()),
        prompt: prompt.map(str::to_string),
        inputs: Default::default(),
    }
}

/// Launch `req` from `ctx` and wait for it.
pub async fn run(l: &RunLauncher, ctx: &ToolContext, req: ChildRequest) -> ChildStatus {
    let h = l.launch(req, ctx).await.expect("launches");
    l.join(
        &ctx.identity,
        JoinSet::Handles(vec![h.handle]),
        Duration::from_secs(60),
        Until::All,
    )
    .await
    .remove(0)
}

/// Run sub-agent `agent` on `prompt` from a bypass parent that may launch
/// it.
pub async fn subagent(
    home: &Home,
    l: &RunLauncher,
    agent: &str,
    codename: Option<&str>,
) -> ChildStatus {
    let ctx = home.parent(codename, &[agent], PermissionMode::Bypass);
    run(l, &ctx, request(ChildKind::Subagent, agent, Some("go"))).await
}

/// Records the parent's live events.
#[derive(Default)]
pub struct Recorder(pub Mutex<Vec<(String, Event)>>);

impl EventSink for Recorder {
    fn emit(&self, run_id: &str, ev: &Event) {
        self.0.lock().unwrap().push((run_id.into(), ev.clone()));
    }
}

pub fn events(path: &Path) -> Vec<rupu_transcript::Event> {
    rupu_transcript::JsonlReader::iter(path)
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

/// `(provider, model)` of a transcript's `RunStart`.
pub fn run_start(path: &Path) -> (String, String) {
    events(path)
        .into_iter()
        .find_map(|e| match e {
            rupu_transcript::Event::RunStart {
                provider, model, ..
            } => Some((provider, model)),
            _ => None,
        })
        .expect("a RunStart")
}

/// What an interrupted agent run leaves: started, its prompt sent, no answer.
pub fn interrupted_transcript(path: &std::path::Path, run_id: &str, agent: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "{{\"type\":\"run_start\",\"data\":{{\"run_id\":\"{run_id}\",\"workspace_id\":\"ws_test\",\"agent\":\"{agent}\",\"provider\":\"anthropic\",\"model\":\"claude-sonnet-4-6\",\"started_at\":\"2026-10-12T00:00:00Z\",\"mode\":\"bypass\"}}}}\n\
             {{\"type\":\"user_message\",\"data\":{{\"content\":\"go\"}}}}\n\
             {{\"type\":\"turn_start\",\"data\":{{\"turn_idx\":0}}}}\n"
        ),
    )
    .unwrap();
}

/// A workflow run record as a running `rupu workflow run` leaves it.
pub fn running_workflow(global: &std::path::Path, run_id: &str) {
    let record: rupu_orchestrator::RunRecord = serde_json::from_value(serde_json::json!({
        "id": run_id,
        "workflow_name": "sweep",
        "status": "running",
        "inputs": {},
        "workspace_id": "ws_test",
        "workspace_path": "/work",
        "transcript_dir": "/work/transcripts",
        "started_at": "2026-10-12T00:00:00Z",
    }))
    .unwrap();
    rupu_orchestrator::RunStore::new(global.join("runs"))
        .create(
            record,
            "name: sweep\nsteps:\n  - id: scan\n    agent: probe\n    prompt: go\n",
        )
        .unwrap();
}

/// Process children that run until they are stopped, each leaving the
/// records a real one would: an agent's interrupted transcript, a
/// workflow's run record.
pub fn interruptible(home: &Home) -> Arc<rupu_launch::testing::ScriptedProcesses> {
    let global = home.global();
    Arc::new(
        rupu_launch::testing::ScriptedProcesses::scripted(vec![rupu_launch::ProcState::Running])
            .with_on_spawn(move |launch| match &launch.argv {
                rupu_runtime::argv::RunArgv::Agent(r) => interrupted_transcript(
                    &rupu_launch::process::transcripts_dir(&global, launch.cwd.as_deref())
                        .join(format!("{}.jsonl", launch.run_id)),
                    &launch.run_id,
                    &r.agent,
                ),
                rupu_runtime::argv::RunArgv::Workflow(_) => {
                    running_workflow(&global, &launch.run_id)
                }
                _ => {}
            }),
    )
}

/// Record `agent_run` as an attempt of `step` in workflow run
/// `workflow_run`'s `attempts.jsonl`.
pub fn attempt_row(global: &std::path::Path, workflow_run: &str, step: &str, agent_run: &str) {
    let row: rupu_orchestrator::runs::AttemptRecord = serde_json::from_value(serde_json::json!({
        "v": 1,
        "step_id": step,
        "agent_run_id": agent_run,
        "transcript_path": "/work/transcripts/step.jsonl",
        "started_at": "2026-10-12T00:00:00Z",
    }))
    .unwrap();
    std::fs::create_dir_all(global.join("runs").join(workflow_run)).unwrap();
    rupu_orchestrator::RunStore::new(global.join("runs"))
        .append_attempt(workflow_run, &row)
        .unwrap();
}

/// A ledger `started` line for a child of a process that is gone.
pub fn orphan_line(
    handle: &str,
    kind: rupu_tools::ChildKind,
    run_id: &str,
) -> rupu_launch::ledger::LedgerLine {
    rupu_launch::ledger::LedgerLine::Started(rupu_launch::ledger::Started::new(
        handle,
        kind,
        "probe",
        run_id,
        0x7fff_fffe,
        1,
    ))
}

/// [`orphan_line`] for a child of a launch tree rooted at `tree`.
pub fn orphan_in_tree(
    handle: &str,
    kind: rupu_tools::ChildKind,
    run_id: &str,
    tree: &std::path::Path,
) -> rupu_launch::ledger::LedgerLine {
    let rupu_launch::ledger::LedgerLine::Started(s) = orphan_line(handle, kind, run_id) else {
        unreachable!()
    };
    rupu_launch::ledger::LedgerLine::Started(rupu_launch::ledger::Started {
        tree: Some(tree.to_path_buf()),
        ..s
    })
}

/// A ledger `finished` line.
pub fn end_line(handle: &str, status: rupu_tools::ChildState) -> rupu_launch::ledger::LedgerLine {
    rupu_launch::ledger::LedgerLine::Finished(rupu_launch::ledger::Finished {
        handle: handle.into(),
        status,
        success: None,
        output: None,
        error: None,
        cause: None,
        tokens: None,
        duration_ms: None,
        at: chrono::Utc::now(),
    })
}
