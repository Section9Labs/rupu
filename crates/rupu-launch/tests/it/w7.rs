//! The W7 acceptance tests (spec §6) that run inside one process: sub-agents
//! through the real assembler and agent loop, process children through a
//! scripted backend or real `/bin/sh` children. The cross-process ones (a
//! real `rupu run` child) live in `rupu-cli`'s serial tests.

use std::sync::Arc;
use std::time::Duration;

use rupu_launch::ledger;
use rupu_launch::testing::ScriptedProcesses;
use rupu_launch::{ProcState, Subprocess};
use rupu_orchestrator::executor::Event as WfEvent;
use rupu_runtime::argv::RunArgv;
use rupu_runtime::assembly::{LaunchSpec, Origin};
use rupu_tools::{ChildKind, ChildState, JoinSet, Launcher, ParentEnd, PermissionMode, Until};
use serial_test::serial;

use crate::support::{
    config, events, interrupted_transcript, interruptible, request, run, Home, MockScript,
    Recorder, DONE,
};

const WF: &str = "name: sweep\nsteps:\n  - id: scan\n    agent: probe\n    prompt: go\n";

pub(crate) fn kinds_home() -> Home {
    let home = Home::new();
    home.agent(
        "helper",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    home.agent(
        "probe",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    home.workflow("sweep", WF);
    home
}

pub(crate) fn parent_ctx(home: &Home, ceiling: PermissionMode) -> rupu_tools::ToolContext {
    let mut ctx = home.parent(Some("jade-reef/heron"), &["helper", "probe"], ceiling);
    Arc::make_mut(&mut ctx.identity).launch.workflows = vec!["sweep".into()];
    ctx
}

/// §6.1: every kind dispatches, joins `done` with its final text, is in the
/// ledger as started + finished, and is announced with its kind; a
/// sub-agent's usage lands in the root ledger, and a process child is told
/// the root ledger to charge (`--usage-root`).
#[tokio::test]
#[serial]
async fn dispatch_kinds_matrix() {
    let home = kinds_home();
    let recorder = Arc::new(Recorder::default());
    let backend = Arc::new(ScriptedProcesses::done("process answer"));
    let l = home.launcher_with(
        home.assembler(config()),
        backend.clone(),
        Some(recorder.clone()),
    );
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    let _m = MockScript::set(DONE);

    let mut statuses = Vec::new();
    for (kind, name, prompt) in [
        (ChildKind::Subagent, "helper", Some("go")),
        (ChildKind::Agent, "probe", Some("go")),
        (ChildKind::Workflow, "sweep", None),
    ] {
        statuses.push(run(&l, &ctx, request(kind, name, prompt)).await);
    }
    let outputs: Vec<_> = statuses
        .iter()
        .map(|s| (s.kind, s.state, s.output.clone()))
        .collect();
    assert_eq!(
        outputs,
        [
            (
                ChildKind::Subagent,
                ChildState::Done,
                Some("child done".into())
            ),
            (
                ChildKind::Agent,
                ChildState::Done,
                Some("process answer".into())
            ),
            (
                ChildKind::Workflow,
                ChildState::Done,
                Some("process answer".into())
            ),
        ]
    );

    let dir = home.runs().join("parent_run_1");
    let ledger = ledger::read(&dir);
    assert_eq!(ledger.len(), 3);
    for (entry, status) in ledger.iter().zip(&statuses) {
        assert_eq!(entry.started.handle, status.handle);
        assert_eq!(entry.started.kind, status.kind);
        assert_eq!(entry.state(), ChildState::Done);
        assert_eq!(entry.started.depth, 1);
    }

    let kinds: Vec<(String, String)> = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|(_, e)| match e {
            WfEvent::DispatchStarted { kind, .. } => ("started".into(), kind.clone()),
            WfEvent::DispatchCompleted { kind, .. } => ("completed".into(), kind.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    for kind in ["subagent", "agent", "workflow"] {
        assert!(
            kinds.contains(&("started".into(), kind.into())),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&("completed".into(), kind.into())),
            "{kinds:?}"
        );
    }

    let root = dir.join("usage.jsonl");
    let rows = std::fs::read_to_string(&root).expect("the sub-agent charged the root ledger");
    assert!(rows.contains(&statuses[0].run_id), "{rows}");
    for launch in backend.spawned() {
        let link = match &launch.argv {
            RunArgv::Agent(r) => r.launch.clone(),
            RunArgv::Workflow(r) => r.launch.clone(),
            other => panic!("{other:?}"),
        }
        .expect("a process child carries its link");
        assert_eq!(link.usage_root.as_deref(), Some(root.as_path()));
        assert_eq!((link.depth, link.parent_run.as_str()), (1, "parent_run_1"));
    }
}

/// §6.2: `dispatch` of a sub-agent returns before the child's first model
/// call. Children run on a runtime nobody drives yet: the launch returns a
/// handle, the child has not started (its transcript is still empty); only
/// when that runtime runs does `join` see it finish.
#[test]
#[serial]
fn subagent_is_async() {
    let home = Home::new();
    home.agent(
        "helper",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let parked = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let caller = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _m = MockScript::set(DONE);
    let l = rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: home.assembler(config()),
        store: Arc::new(rupu_orchestrator::RunStore::new(home.runs())),
        events: None,
        namer: None,
        backend: Arc::new(ScriptedProcesses::done("x")),
        runtime: parked.handle().clone(),
        max_concurrent: 8,
        generation: None,
        project: None,
    });
    let ctx = home.parent(None, &["helper"], PermissionMode::Bypass);
    let handle = caller
        .block_on(l.launch(request(ChildKind::Subagent, "helper", Some("go")), &ctx))
        .unwrap();
    let transcript = handle.transcript.clone().unwrap();
    assert_eq!(
        std::fs::metadata(&transcript).unwrap().len(),
        0,
        "not started"
    );
    assert_eq!(l.children(&ctx.identity)[0].state, ChildState::Pending);

    let status = parked.block_on(l.join(
        &ctx.identity,
        JoinSet::Handles(vec![handle.handle]),
        Duration::from_secs(30),
        Until::All,
    ));
    assert_eq!(status[0].state, ChildState::Done);
    assert_eq!(status[0].output.as_deref(), Some("child done"));
}

/// §6.3 (in process): a run at the ceiling depth is refused, whichever kind
/// it asks for; one below it may launch, and its child carries depth + 1.
#[tokio::test]
#[serial]
async fn depth_is_capped_at_max_launch_depth() {
    let home = kinds_home();
    let backend = Arc::new(ScriptedProcesses::done("x"));
    let l = home.launcher_with(home.assembler(config()), backend.clone(), None);
    let mut ctx = parent_ctx(&home, PermissionMode::Bypass);
    Arc::make_mut(&mut ctx.identity).depth = rupu_tools::MAX_LAUNCH_DEPTH;
    for (kind, name) in [(ChildKind::Subagent, "helper"), (ChildKind::Agent, "probe")] {
        let err = l
            .launch(request(kind, name, Some("go")), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.code, "max_dispatch_depth_exceeded", "{err}");
    }
    Arc::make_mut(&mut ctx.identity).depth = rupu_tools::MAX_LAUNCH_DEPTH - 1;
    run(&l, &ctx, request(ChildKind::Agent, "probe", Some("go"))).await;
    let RunArgv::Agent(run) = &backend.spawned()[0].argv else {
        panic!()
    };
    assert_eq!(
        run.launch.as_ref().unwrap().depth,
        rupu_tools::MAX_LAUNCH_DEPTH
    );
}

/// §6.4 (in process): a readonly parent's process child is launched at
/// `--mode readonly` with a readonly ceiling, whatever its own mode.
#[tokio::test]
#[serial]
async fn a_process_child_carries_the_parents_ceiling() {
    let home = kinds_home();
    home.agent(
        "probe",
        "provider: anthropic\nmodel: claude-sonnet-4-6\npermissionMode: bypass",
    );
    let backend = Arc::new(ScriptedProcesses::done("x"));
    let l = home.launcher_with(home.assembler(config()), backend.clone(), None);
    let ctx = parent_ctx(&home, PermissionMode::Readonly);
    run(&l, &ctx, request(ChildKind::Agent, "probe", Some("go"))).await;
    let RunArgv::Agent(run) = &backend.spawned()[0].argv else {
        panic!()
    };
    assert_eq!(run.mode, Some(PermissionMode::Readonly));
    assert_eq!(
        run.launch.as_ref().unwrap().ceiling,
        PermissionMode::Readonly
    );
}

/// Spec F1d D6: a parent whose messaging is off starts its process children
/// (an agent, a workflow) with `--launch-no-messaging`; one whose messaging
/// is on passes no such flag.
#[tokio::test]
#[serial]
async fn messaging_off_is_a_launch_cap() {
    for (off, kind, name) in [
        (true, ChildKind::Agent, "helper"),
        (true, ChildKind::Workflow, "sweep"),
        (false, ChildKind::Agent, "helper"),
    ] {
        let home = kinds_home();
        let backend = Arc::new(ScriptedProcesses::done("x"));
        let l = home.launcher_with(home.assembler(config()), backend.clone(), None);
        let mut ctx = parent_ctx(&home, PermissionMode::Bypass);
        Arc::make_mut(&mut ctx.identity).launch.messaging_off = off;
        run(&l, &ctx, request(kind, name, Some("go"))).await;
        let argv = &backend.spawned()[0].argv;
        let link = match argv {
            RunArgv::Agent(r) => r.launch.clone(),
            RunArgv::Workflow(w) => w.launch.clone(),
            other => panic!("{other:?}"),
        }
        .unwrap();
        assert_eq!(link.no_messaging, off, "{kind:?}");
        let flagged = argv
            .to_args()
            .iter()
            .any(|a| a.to_string_lossy() == "--launch-no-messaging");
        assert_eq!(flagged, off, "{kind:?}: {:?}", argv.to_args());
    }
}

/// §6.5: a workflow child's step agent declares `dispatchableAgents:
/// [outsider]`, but its parent's cap (passed as `--launch-agents`) excludes
/// it, so the step can't dispatch `outsider`.
#[tokio::test]
#[serial]
async fn allowlist_inherited() {
    let home = kinds_home();
    home.agent(
        "probe",
        "provider: anthropic\nmodel: claude-sonnet-4-6\ndispatchableAgents: [outsider, helper]",
    );
    home.agent("outsider", "provider: anthropic\nmodel: claude-sonnet-4-6");
    let backend = Arc::new(ScriptedProcesses::done("x"));
    let assembler = home.assembler(config());
    let l = home.launcher_with(assembler.clone(), backend.clone(), None);
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    run(&l, &ctx, request(ChildKind::Workflow, "sweep", None)).await;
    let RunArgv::Workflow(wf) = &backend.spawned()[0].argv else {
        panic!()
    };
    let link = wf.launch.clone().unwrap();
    assert!(!link.agents.contains(&"outsider".to_string()), "{link:?}");

    // The workflow child's step, assembled the way its process assembles it.
    let agent = rupu_agent::load_agent(&home.global(), None, "probe").unwrap();
    let _m = MockScript::set(DONE);
    let step = assembler
        .assemble(LaunchSpec {
            agent,
            origin: Origin::WorkflowStep {
                workflow_run_id: wf.run_id.clone(),
                workflow_name: "sweep".into(),
                step_id: "scan".into(),
                unit: None,
                step_actions: Vec::new(),
                scope_override: None,
                messaging: rupu_tools::MessagingSwitch::Default,
            },
            run_id: "run_step".into(),
            transcript_path: home.root().join("step.jsonl"),
            codename: None,
            prompt: rupu_agent::UserTurn::new("go"),
            workspace: home.binding(),
            mode: PermissionMode::Bypass,
            ceiling: None,
            prompter: None,
            overrides: Default::default(),
            stream: Default::default(),
            hooks: Default::default(),
            pause: None,
            services: Default::default(),
            collectors: Vec::new(),
            link: Some(link),
        })
        .await
        .unwrap();
    let step_ctx = step.opts.tool_context.clone();
    assert_eq!(step_ctx.identity.launch.agents, vec!["helper".to_string()]);
    let err = l
        .check(
            &request(ChildKind::Subagent, "outsider", Some("go")),
            &step_ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "agent_not_dispatchable", "{err}");
}

/// §6.6: a parent narrowed to `actions: [issues.get]` dispatches a
/// sub-agent whose own `tools:` holds every issue tool; the child can't call
/// `issues.create` (L4).
#[tokio::test]
#[serial]
async fn actions_cap_inherited() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\ntools: [\"issues.*\"]",
    );
    let l = home.launcher(config(), None);
    let mut ctx = home.parent(None, &["child"], PermissionMode::Bypass);
    Arc::make_mut(&mut ctx.identity).launch.connector_cap = Some(vec!["issues.get".into()]);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "c1", "tool_name": "issues.create", "tool_input": {"title": "x"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "done", "stop": "end_turn" } }
        ]"#,
    );
    let status = run(&l, &ctx, request(ChildKind::Subagent, "child", Some("go"))).await;
    let evs = events(status.transcript.as_deref().unwrap());
    let grant = evs
        .iter()
        .find_map(|e| match e {
            rupu_transcript::Event::ToolGrant {
                entries, narrowed, ..
            } => Some((entries.clone(), narrowed.clone())),
            _ => None,
        })
        .expect("the child's grant");
    let offered: Vec<&str> = grant.0.iter().map(|t| t.tool.as_str()).collect();
    assert_eq!(offered, ["issues.get"]);
    assert!(grant.1.contains(&"issues.create".to_string()), "{grant:?}");
    assert!(evs.iter().any(|e| matches!(
        e,
        rupu_transcript::Event::ToolAudit { tool, decision, .. }
            if tool == "issues.create" && decision.as_deref() == Some("not_granted")
    )));
}

/// §6.7: when the parent ends, an unjoined sub-agent and an unjoined process
/// child are stopped and recorded `abandoned`, and the process is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn parent_end_abandons() {
    let home = kinds_home();
    let pidfile = home.root().join("child.pid");
    let backend = Arc::new(Subprocess::new("/bin/sh", home.global()).with_pre_args([
        "-c".to_string(),
        format!("echo $$ > {}; exec sleep 30", pidfile.display()),
        "sh".to_string(),
    ]));
    let l = home.launcher_with(home.assembler(config()), backend, None);
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "b1", "tool_name": "bash", "tool_input": {"command": "sleep 2"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "never", "stop": "end_turn" } }
        ]"#,
    );
    let sub = l
        .launch(request(ChildKind::Subagent, "helper", Some("go")), &ctx)
        .await
        .unwrap();
    let proc = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let pid: u32 = loop {
        if let Some(p) = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|t| t.trim().parse().ok())
        {
            break p;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the process child never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    l.finish(&ctx.identity, ParentEnd::Completed).await;

    let ledger = ledger::read(&home.runs().join("parent_run_1"));
    let by_handle = |h: &str| ledger.iter().find(|c| c.started.handle == h).unwrap();
    for h in [&sub.handle, &proc.handle] {
        assert_eq!(by_handle(h).state(), ChildState::Abandoned, "{h}");
    }
    assert!(
        !rupu_launch::proc::pid_is_running(pid),
        "the process child is gone"
    );
    assert!(
        l.children(&ctx.identity).is_empty(),
        "forgotten once finished"
    );
}

/// §6.10: with one running child, the next turn's injection names it.
#[tokio::test]
#[serial]
async fn collector_reports_children() {
    let home = kinds_home();
    let backend = Arc::new(ScriptedProcesses::scripted(vec![ProcState::Running]));
    let l = home.launcher_with(home.assembler(config()), backend, None);
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    let h = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();
    let collector = rupu_runtime::children::ChildrenCollector::new(
        Arc::clone(&l) as Arc<dyn Launcher>,
        Arc::clone(&ctx.identity),
    );
    let inj = rupu_agent::TurnCollector::collect(
        &collector,
        &rupu_agent::collector::TurnContext {
            run_id: "parent_run_1".into(),
            codename: None,
            participant: String::new(),
            turn_index: 1,
        },
    );
    assert_eq!(inj.len(), 1);
    assert!(inj[0].content.contains(&h.handle), "{}", inj[0].content);
    assert!(inj[0].content.contains("agent probe"), "{}", inj[0].content);
    l.finish(&ctx.identity, ParentEnd::Completed).await;
}

struct Allow;

impl rupu_tools::Prompter for Allow {
    fn ask(&self, _: &rupu_tools::PromptRequest<'_>) -> rupu_tools::PromptAnswer {
        rupu_tools::PromptAnswer::Allow
    }
}

fn spawned_modes(backend: &ScriptedProcesses) -> Vec<(Option<PermissionMode>, PermissionMode)> {
    backend
        .spawned()
        .iter()
        .map(|l| match &l.argv {
            RunArgv::Agent(r) => (r.mode, r.launch.as_ref().unwrap().ceiling),
            RunArgv::Workflow(r) => (r.mode, r.launch.as_ref().unwrap().ceiling),
            other => panic!("{other:?}"),
        })
        .collect()
}

/// A process child of an interactive `ask` parent can't reach the operator,
/// so it runs readonly; an unattended `ask` parent's keeps `ask`, and a
/// sub-agent shares the parent's prompter at `ask`.
#[tokio::test]
#[serial]
async fn an_interactive_ask_parents_process_children_run_readonly() {
    let home = kinds_home();
    let backend = Arc::new(ScriptedProcesses::done("x"));
    let l = home.launcher_with(home.assembler(config()), backend.clone(), None);
    let mut interactive = parent_ctx(&home, PermissionMode::Ask);
    interactive.services.prompter = Some(Arc::new(Allow));
    let unattended = parent_ctx(&home, PermissionMode::Ask);
    run(
        &l,
        &interactive,
        request(ChildKind::Agent, "probe", Some("go")),
    )
    .await;
    run(
        &l,
        &interactive,
        request(ChildKind::Workflow, "sweep", None),
    )
    .await;
    run(
        &l,
        &unattended,
        request(ChildKind::Agent, "probe", Some("go")),
    )
    .await;
    assert_eq!(
        spawned_modes(&backend),
        [
            (Some(PermissionMode::Readonly), PermissionMode::Readonly),
            (Some(PermissionMode::Readonly), PermissionMode::Readonly),
            (Some(PermissionMode::Ask), PermissionMode::Ask),
        ]
    );

    let _m = MockScript::set(DONE);
    let sub = run(
        &l,
        &interactive,
        request(ChildKind::Subagent, "helper", Some("go")),
    )
    .await;
    assert!(events(sub.transcript.as_deref().unwrap())
        .iter()
        .any(|e| matches!(
            e,
            rupu_transcript::Event::RunStart {
                mode: rupu_transcript::RunMode::Ask,
                ..
            }
        )));
}

/// `join` with no handles waits only for the children not yet collected: a
/// child already joined is neither returned again nor satisfies `until: any`.
#[tokio::test]
#[serial]
async fn join_without_handles_skips_collected_children() {
    let home = kinds_home();
    let backend = Arc::new(ScriptedProcesses::scripted(vec![
        ProcState::Running,
        ProcState::Running,
        ProcState::Done(rupu_launch::ProcOutcome {
            output: "x".into(),
            success: true,
        }),
    ]));
    let l = home.launcher_with(home.assembler(config()), backend, None);
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    let first = run(&l, &ctx, request(ChildKind::Agent, "probe", Some("go"))).await;
    assert_eq!(first.state, ChildState::Done);
    let second = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();

    let got = l
        .join(
            &ctx.identity,
            JoinSet::All,
            Duration::from_secs(30),
            Until::Any,
        )
        .await;
    let got: Vec<_> = got.iter().map(|s| (s.handle.as_str(), s.state)).collect();
    assert_eq!(got, [(second.handle.as_str(), ChildState::Done)]);
    assert!(l
        .join(&ctx.identity, JoinSet::All, Duration::ZERO, Until::All)
        .await
        .is_empty());
}

/// A child whose task panics is settled `failed` and gives its slot back.
#[tokio::test]
#[serial]
async fn a_panicking_child_task_settles_failed_and_frees_its_slot() {
    let home = kinds_home();
    let spawns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&spawns);
    let backend = Arc::new(
        ScriptedProcesses::scripted(vec![
            ProcState::Running,
            ProcState::Done(rupu_launch::ProcOutcome {
                output: "x".into(),
                success: true,
            }),
        ])
        .with_on_spawn(move |_| {
            if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                panic!("the backend blew up");
            }
        }),
    );
    let l = rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: home.assembler(config()),
        store: Arc::new(rupu_orchestrator::RunStore::new(home.runs())),
        events: None,
        namer: None,
        backend,
        runtime: tokio::runtime::Handle::current(),
        max_concurrent: 1,
        generation: None,
        project: None,
    });
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    let first = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();
    // No slot until the first finishes: this one spawns from its task.
    let queued = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();
    let got = l
        .join(
            &ctx.identity,
            JoinSet::Handles(vec![first.handle, queued.handle]),
            Duration::from_secs(20),
            Until::All,
        )
        .await;
    assert_eq!(got[0].state, ChildState::Done);
    assert_eq!(
        (got[1].state, got[1].error.as_deref()),
        (ChildState::Failed, Some("its task ended abnormally"))
    );
    let after = run(&l, &ctx, request(ChildKind::Agent, "probe", Some("go"))).await;
    assert_eq!(after.state, ChildState::Done, "the slot came back");
}

/// A sub-agent that ends abnormally stops the process children it launched
/// and records them `abandoned` in its own ledger.
#[tokio::test]
#[serial]
async fn an_abnormally_ended_subagent_stops_its_own_children() {
    let home = kinds_home();
    home.agent(
        "helper",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 4\ndispatchableAgents: [probe]",
    );
    let spawns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&spawns);
    let backend = Arc::new(
        ScriptedProcesses::scripted(vec![ProcState::Running]).with_on_spawn(move |_| {
            if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                panic!("the backend blew up");
            }
        }),
    );
    let l = home.launcher_with(home.assembler(config()), backend.clone(), None);
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "d1", "tool_name": "dispatch", "tool_input": {"kind": "agent", "name": "probe", "prompt": "go"}, "stop": "tool_use" } },
          { "AssistantToolUse": { "text": null, "tool_id": "d2", "tool_name": "dispatch", "tool_input": {"kind": "agent", "name": "probe", "prompt": "go"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "never", "stop": "end_turn" } }
        ]"#,
    );
    let sub = run(&l, &ctx, request(ChildKind::Subagent, "helper", Some("go"))).await;
    assert_eq!(sub.state, ChildState::Failed, "{sub:?}");

    let grandchild = backend.spawned()[0].run_id.clone();
    let sub_dir = home.runs().join(&sub.run_id);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let states: Vec<_> = ledger::read(&sub_dir).iter().map(|c| c.state()).collect();
        if states == [ChildState::Abandoned] {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the sub-agent's child was never stopped: {states:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(backend.terminated(), [grandchild]);
}

/// A process child that finished just as its parent stopped it keeps its
/// result.
#[tokio::test]
#[serial]
async fn a_child_done_as_it_is_stopped_reads_done() {
    struct DoneOnStop(std::sync::Mutex<bool>);
    impl rupu_launch::ProcessBackend for DoneOnStop {
        fn spawn(
            &self,
            _: &rupu_launch::ProcessLaunch,
        ) -> Result<Option<rupu_launch::SpawnedProcess>, String> {
            Ok(None)
        }
        fn poll(&self, _: &str) -> ProcState {
            if *self.0.lock().unwrap() {
                ProcState::Done(rupu_launch::ProcOutcome {
                    output: "finished".into(),
                    success: true,
                })
            } else {
                ProcState::Running
            }
        }
        fn terminate(&self, _: &str, _: rupu_launch::Stop) {
            *self.0.lock().unwrap() = true;
        }
    }
    let home = kinds_home();
    let l = home.launcher_with(
        home.assembler(config()),
        Arc::new(DoneOnStop(std::sync::Mutex::new(false))),
        None,
    );
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    let h = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    l.finish(&ctx.identity, ParentEnd::Completed).await;
    let entry = ledger::read(&home.runs().join("parent_run_1"))
        .into_iter()
        .find(|c| c.started.handle == h.handle)
        .unwrap();
    assert_eq!(entry.state(), ChildState::Done);
}

/// A named workflow child runs the very file it was vetted against (its argv
/// names the file, not the name for the child to look up again), under the
/// codename its parent minted and the name it was vetted as.
#[tokio::test]
#[serial]
async fn a_named_workflow_child_runs_the_vetted_file() {
    let home = kinds_home();
    let backend = Arc::new(ScriptedProcesses::done("x"));
    let l = home.launcher_with(home.assembler(config()), backend.clone(), None);
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    run(&l, &ctx, request(ChildKind::Workflow, "sweep", None)).await;
    let RunArgv::Workflow(wf) = &backend.spawned()[0].argv else {
        panic!()
    };
    assert_eq!(
        wf.workflow,
        rupu_runtime::argv::WorkflowRef::File(home.global().join("workflows").join("sweep.yaml"))
    );
    assert_eq!(wf.workflow_name.as_deref(), Some("sweep"));
    assert_eq!(
        wf.codename.as_deref(),
        ledger::read(&home.runs().join("parent_run_1"))[0]
            .started
            .codename
            .as_deref()
    );
    assert!(wf.codename.is_some());
}

const FINDINGS_WF: &str = "name: record\ndefaults:\n  findings_profile: summary\nsteps:\n  - id: rec\n    action: findings.record\n    with:\n      scope: repo\n      summary: s\n      severity: low\n      rationale: r\n";

fn generating_launcher(home: &Home, yaml: &'static str) -> Arc<rupu_launch::RunLauncher> {
    use rupu_agent::runner::{MockProvider, ScriptedTurn};
    rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: home.assembler(config()),
        store: Arc::new(rupu_orchestrator::RunStore::new(home.runs())),
        events: None,
        namer: None,
        backend: Arc::new(ScriptedProcesses::done("x")),
        runtime: tokio::runtime::Handle::current(),
        max_concurrent: 8,
        generation: Some(rupu_launch::Generation {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            factory: Arc::new(move || {
                Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                    text: yaml.into(),
                    stop: rupu_providers::types::StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 1,
                }])) as Box<dyn rupu_providers::LlmProvider>
            }),
        }),
        project: None,
    })
}

const GET_WF: &str = "name: look\nsteps:\n  - id: get\n    action: issues.get\n    with:\n      project: acme/w\n      number: 1\n";

/// The tool context of a standalone run of an agent with `tools`, as the
/// assembler builds it, launching through `l`.
async fn assembled_caller(
    home: &Home,
    l: &Arc<rupu_launch::RunLauncher>,
    tools: &str,
) -> rupu_tools::ToolContext {
    home.agent(
        "caller",
        &format!("provider: anthropic\nmodel: claude-sonnet-4-6\ntools: {tools}"),
    );
    let agent = rupu_agent::load_agent(&home.global(), None, "caller").unwrap();
    let _m = MockScript::set(DONE);
    let run = home
        .assembler(config())
        .assemble(LaunchSpec {
            agent,
            origin: Origin::Standalone,
            run_id: "run_caller".into(),
            transcript_path: home.root().join("caller.jsonl"),
            codename: None,
            prompt: rupu_agent::UserTurn::new("go"),
            workspace: home.binding(),
            mode: PermissionMode::Bypass,
            ceiling: None,
            prompter: None,
            overrides: Default::default(),
            stream: Default::default(),
            hooks: Default::default(),
            pause: None,
            services: rupu_runtime::assembly::LaunchServices {
                launcher: Some(l.clone() as Arc<dyn Launcher>),
                workflow_generator: Some(l.clone() as Arc<dyn rupu_tools::WorkflowGenerator>),
                ..Default::default()
            },
            collectors: Vec::new(),
            link: None,
        })
        .await
        .unwrap();
    run.opts.tool_context.clone()
}

/// A generated workflow's connector `action:` steps may call only connector
/// tools the caller's own grant offers, narrowed or not: a `tools: ["*"]`
/// caller with no cap may author `issues.get`; one granted only the launch
/// tools may not, nor dispatch the file the first one wrote. A findings
/// action is no connector.
#[tokio::test]
#[serial]
async fn a_generated_workflows_connector_actions_need_the_callers_grant() {
    use rupu_tools::WorkflowGenerator;
    let home = kinds_home();
    let l = generating_launcher(&home, GET_WF);

    let star = assembled_caller(&home, &l, "[\"*\"]").await;
    assert_eq!(star.identity.launch.connector_cap, None);
    let ok = l
        .generate("look", &star)
        .await
        .expect("issues.get is granted");

    let launch_only = assembled_caller(&home, &l, "[dispatch, workflows.generate]").await;
    let err = l.generate("look", &launch_only).await.unwrap_err();
    assert_eq!(err.code, "generation_failed", "{err}");
    assert!(err.message.contains("issues.get"), "{err}");
    let req = rupu_tools::ChildRequest {
        kind: ChildKind::Workflow,
        target: rupu_tools::ChildTarget::File(ok.file),
        prompt: None,
        inputs: Default::default(),
    };
    // Same run id, so the same `generated/` directory.
    let err = l.check(&req, &launch_only).await.unwrap_err();
    assert_eq!(err.code, "workflow_not_runnable", "{err}");

    let l = generating_launcher(&home, FINDINGS_WF);
    l.generate("record", &launch_only)
        .await
        .expect("findings need no connector");
}

/// A provider that records each request's system prompt, then answers like
/// the `MockProvider` it wraps.
struct RecordingProvider {
    inner: rupu_agent::runner::MockProvider,
    systems: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl rupu_providers::LlmProvider for RecordingProvider {
    async fn send(
        &mut self,
        request: &rupu_providers::LlmRequest,
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        self.systems
            .lock()
            .unwrap()
            .push(request.system.clone().unwrap_or_default());
        self.inner.send(request).await
    }

    async fn stream(
        &mut self,
        request: &rupu_providers::LlmRequest,
        on_event: &mut (dyn FnMut(rupu_providers::StreamEvent) + Send),
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        self.inner.stream(request, on_event).await
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        self.inner.provider_id()
    }
}

/// `workflows.generate` tells the model which skills are installed, in the
/// global layer and the launcher's project `.rupu`, so a generated step's
/// `skills:` names ones that exist.
#[tokio::test]
#[serial]
async fn generate_offers_the_installed_skills_of_both_layers() {
    use rupu_agent::runner::{MockProvider, ScriptedTurn};
    use rupu_tools::WorkflowGenerator;
    let home = kinds_home();
    home.skill("pdf", "read pdfs");
    let project = home.root().join("repo/.rupu");
    std::fs::create_dir_all(project.join("skills/notes")).unwrap();
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = systems.clone();
    let l = rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: home.assembler(config()),
        store: Arc::new(rupu_orchestrator::RunStore::new(home.runs())),
        events: None,
        namer: None,
        backend: Arc::new(ScriptedProcesses::done("x")),
        runtime: tokio::runtime::Handle::current(),
        max_concurrent: 8,
        generation: Some(rupu_launch::Generation {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            factory: Arc::new(move || {
                Box::new(RecordingProvider {
                    inner: MockProvider::new(vec![ScriptedTurn::AssistantText {
                        text: FINDINGS_WF.into(),
                        stop: rupu_providers::types::StopReason::EndTurn,
                        input_tokens: 1,
                        output_tokens: 1,
                    }]),
                    systems: recorded.clone(),
                }) as Box<dyn rupu_providers::LlmProvider>
            }),
        }),
        project: Some(project),
    });
    let caller = assembled_caller(&home, &l, "[\"*\"]").await;
    l.generate("record", &caller).await.expect("generated");
    let systems = systems.lock().unwrap();
    assert_eq!(systems.len(), 1);
    assert!(
        systems[0].ends_with("leave both empty when none fits): notes, pdf"),
        "{}",
        systems[0]
    );
}

/// A recorded process child is signalled only when its pid and recorded
/// start time confirm it. One recorded without a start time (it exited
/// before its start could be read) can't be confirmed: its pid, now held by
/// another process that leads its own group, is never signalled, and it is
/// not taken for running — the run that continues its parent finds it
/// `interrupted`.
#[test]
fn a_pid_recorded_without_its_start_time_is_never_signalled() {
    use std::os::unix::process::CommandExt;
    let global = tempfile::tempdir().unwrap();
    let dir = global.path().join("runs").join("run_parent");
    let mut stranger = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .process_group(0)
        .spawn()
        .unwrap();
    ledger::append(
        &dir,
        &ledger::LedgerLine::Started(ledger::Started {
            pid: Some(stranger.id()),
            pgid: Some(stranger.id()),
            proc_start: None,
            ..ledger::Started::new(
                "ch_unconfirmed",
                ChildKind::Agent,
                "probe",
                "run_1",
                0x7fff_fffe,
                1,
            )
        }),
    );
    let started = std::time::Instant::now();
    rupu_launch::reaper::interrupt_orphans(&dir, Duration::from_secs(5));
    let took = started.elapsed();
    let alive = matches!(stranger.try_wait(), Ok(None));
    stranger.kill().unwrap();
    stranger.wait().unwrap();
    assert!(alive, "an unconfirmed pid was signalled");
    assert!(
        took < Duration::from_secs(2),
        "waited {took:?} on a group it never owned"
    );
    assert_eq!(ledger::read(&dir)[0].state(), ChildState::Interrupted);
}

/// A recorded pid now taken by an unrelated process that led its own group
/// and exited, leaving members behind: the group can't be tied to the child
/// (the pid's process is gone), so nothing is signalled, and the child is
/// still recorded.
#[test]
fn a_reused_pids_leftover_group_is_never_signalled() {
    use std::os::unix::process::CommandExt;
    let global = tempfile::tempdir().unwrap();
    let dir = global.path().join("runs").join("run_parent");
    let pidfile = global.path().join("member.pid");
    let mut leader = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            &format!("sleep 30 & echo $! > {}; exit 0", pidfile.display()),
        ])
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = leader.id();
    let leader_start = rupu_launch::proc::start_stamp(pgid).expect("a start time");
    leader.wait().unwrap();
    let member: u32 = {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(&pidfile)
                .ok()
                .and_then(|t| t.trim().parse().ok())
            {
                break pid;
            }
            assert!(std::time::Instant::now() < deadline, "no member pid");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    // The child that had the pid before the stranger did.
    ledger::append(
        &dir,
        &ledger::LedgerLine::Started(ledger::Started {
            pid: Some(pgid),
            pgid: Some(pgid),
            proc_start: Some(leader_start - 1),
            ..ledger::Started::new(
                "ch_crashed",
                ChildKind::Agent,
                "probe",
                "run_crashed",
                0x7fff_fffe,
                1,
            )
        }),
    );

    let summary =
        rupu_launch::reaper::reap_orphaned_children(global.path(), Duration::from_secs(5));

    let survived = rupu_launch::proc::pid_is_running(member);
    rupu_launch::proc::kill_group(pgid);
    assert_eq!(summary.reaped.len(), 1);
    assert!(survived, "the stranger's group was signalled");
    assert_eq!(ledger::read(&dir)[0].state(), ChildState::Interrupted);
}

/// An event sink that panics on the second child's start.
#[derive(Default)]
struct PanicsOnSecondStart(std::sync::atomic::AtomicUsize);

impl rupu_orchestrator::executor::EventSink for PanicsOnSecondStart {
    fn emit(&self, _: &str, ev: &WfEvent) {
        if matches!(ev, WfEvent::DispatchStarted { .. })
            && self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
        {
            panic!("the sink blew up");
        }
    }
}

/// A process child whose task panics after it was spawned is stopped before
/// it is reported `failed`: nothing would watch it any more.
#[tokio::test]
#[serial]
async fn a_panicked_process_childs_process_is_stopped() {
    let home = kinds_home();
    let backend = Arc::new(ScriptedProcesses::scripted(vec![
        ProcState::Running,
        ProcState::Done(rupu_launch::ProcOutcome {
            output: "x".into(),
            success: true,
        }),
    ]));
    let l = rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: home.assembler(config()),
        store: Arc::new(rupu_orchestrator::RunStore::new(home.runs())),
        events: Some(Arc::new(PanicsOnSecondStart::default())),
        namer: None,
        backend: backend.clone(),
        runtime: tokio::runtime::Handle::current(),
        max_concurrent: 1,
        generation: None,
        project: None,
    });
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    l.launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();
    // No slot until the first finishes: this one spawns from its task.
    let queued = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();
    let got = l
        .join(
            &ctx.identity,
            JoinSet::Handles(vec![queued.handle]),
            Duration::from_secs(20),
            Until::All,
        )
        .await;
    assert_eq!(
        (got[0].state, got[0].error.as_deref()),
        (ChildState::Failed, Some("its task ended abnormally"))
    );
    assert_eq!(backend.terminated(), [queued.run_id]);
}

/// Lifts the process-wide stop when a test ends, however it ends.
struct Unstop;

impl Drop for Unstop {
    fn drop(&mut self) {
        rupu_launch::reaper::reset_stopping();
    }
}

/// A process being stopped by a signal starts no more children: one waiting
/// for a slot ends `interrupted` instead of taking the slot the stop frees,
/// a new launch is refused, and no child is left running. What it stopped is
/// recorded `interrupted`: the run that continues it continues them (D2).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn a_stopping_process_starts_no_more_children() {
    let home = kinds_home();
    let _unstop = Unstop;
    let backend =
        Arc::new(Subprocess::new("/bin/sh", home.global()).with_pre_args(["-c", "sleep 30", "sh"]));
    let l = rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: home.assembler(config()),
        store: Arc::new(rupu_orchestrator::RunStore::new(home.runs())),
        events: None,
        namer: None,
        backend,
        runtime: tokio::runtime::Handle::current(),
        max_concurrent: 1,
        generation: None,
        project: None,
    });
    let ctx = parent_ctx(&home, PermissionMode::Bypass);
    let first = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();
    let queued = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap();

    tokio::task::spawn_blocking(|| {
        rupu_launch::reaper::stop_live_children(Duration::from_secs(5), "stopped")
    })
    .await
    .unwrap();
    // Time for a freed slot to start a waiting child.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let refused = l
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "stopping", "{refused}");
    let entries = ledger::read(&home.runs().join("parent_run_1"));
    let entry = |h: &str| entries.iter().find(|c| c.started.handle == h).cloned();
    assert_eq!(
        entry(&first.handle).map(|c| c.state()),
        Some(ChildState::Interrupted)
    );
    let queued = entry(&queued.handle).expect("recorded");
    assert_eq!(
        (queued.state(), queued.started.pid),
        (ChildState::Interrupted, None),
        "never started"
    );
    for c in &entries {
        if let Some(pid) = c.started.pid {
            assert!(
                !rupu_launch::proc::pid_is_running(pid) || rupu_launch::proc::is_zombie(pid),
                "{} still runs",
                c.started.handle
            );
        }
    }
}

/// The parent as attempt `run_id`: a pause or a crash ends one attempt of a
/// run, and the run that continues its conversation is the next attempt.
pub(crate) fn attempt(home: &Home, run_id: &str) -> rupu_tools::ToolContext {
    let mut ctx = parent_ctx(home, PermissionMode::Bypass);
    let id = Arc::make_mut(&mut ctx.identity);
    id.run_id = run_id.into();
    id.launch.children_dir = Some(home.runs().join(run_id));
    id.launch.usage_root = Some(home.runs().join(run_id).join("usage.jsonl"));
    ctx
}

pub(crate) async fn join_one(
    l: &rupu_launch::RunLauncher,
    ctx: &rupu_tools::ToolContext,
    handle: &str,
) -> rupu_tools::ChildStatus {
    l.join(
        &ctx.identity,
        JoinSet::Handles(vec![handle.to_string()]),
        Duration::from_secs(60),
        Until::All,
    )
    .await
    .remove(0)
}

/// (a) A pause stops a running agent child and records it `interrupted`.
/// After a reboot (a new launcher; the old process is gone), the run that
/// continues the parent's conversation continues the child under the same
/// handle and codename, as `rupu run --continue <its run>`, and `join`
/// returns the continued run's answer.
#[tokio::test]
#[serial]
async fn a_paused_parents_agent_child_is_continued_by_the_run_that_continues_it() {
    let home = kinds_home();
    let before = interruptible(&home);
    let l1 = home.launcher_with(home.assembler(config()), before.clone(), None);
    let ctx1 = attempt(&home, "parent_run_1");
    let h = l1
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx1)
        .await
        .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);
    assert_eq!(before.terminated(), std::slice::from_ref(&h.run_id));
    let prior = ledger::read(&home.runs().join("parent_run_1"));
    assert_eq!(prior[0].state(), ChildState::Interrupted);

    let after = Arc::new(ScriptedProcesses::done("continued answer"));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let st = join_one(&l2, &ctx2, &h.handle).await;

    assert_eq!(
        (st.state, st.output.as_deref()),
        (ChildState::Done, Some("continued answer"))
    );
    let spawned = after.spawned();
    let RunArgv::Agent(run) = &spawned[0].argv else {
        panic!("{spawned:?}")
    };
    assert_eq!(run.continue_from.as_deref(), Some(h.run_id.as_str()));
    assert_ne!(run.run_id, h.run_id, "a continuation is a new run");
    assert_eq!(run.codename, h.codename);
    assert_eq!(run.prompt, None);
    let now = ledger::read(&home.runs().join("parent_run_2"));
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].started.handle, h.handle);
    assert_eq!(now[0].started.run_id, run.run_id);
    assert_eq!(now[0].state(), ChildState::Done);
}

/// (b) The same for a workflow child: continued as `rupu workflow resume`
/// of its own run, which keeps its run id.
#[tokio::test]
#[serial]
async fn a_paused_parents_workflow_child_is_resumed_by_the_run_that_continues_it() {
    let home = kinds_home();
    let before = interruptible(&home);
    let l1 = home.launcher_with(home.assembler(config()), before.clone(), None);
    let ctx1 = attempt(&home, "parent_run_1");
    let h = l1
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx1)
        .await
        .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);
    assert_eq!(
        ledger::read(&home.runs().join("parent_run_1"))[0].state(),
        ChildState::Interrupted
    );

    let after = Arc::new(ScriptedProcesses::done("continued answer"));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let st = join_one(&l2, &ctx2, &h.handle).await;

    assert_eq!(
        (st.state, st.output.as_deref()),
        (ChildState::Done, Some("continued answer"))
    );
    assert_eq!(
        after.spawned()[0].argv,
        RunArgv::WorkflowControl(rupu_runtime::argv::WorkflowControl::Resume {
            run_id: h.run_id.clone(),
            mode: Some(PermissionMode::Bypass),
            if_unfinished: false,
        })
    );
    assert_eq!(
        ledger::read(&home.runs().join("parent_run_2"))[0]
            .started
            .run_id,
        h.run_id
    );
}

/// (c) A sub-agent stopped at its next boundary by its parent's pause is
/// recorded `interrupted`; the continuing run continues it in-process from
/// its transcript (seeded by reference), and `join` gets its answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn a_paused_parents_sub_agent_is_continued_from_its_transcript() {
    let home = kinds_home();
    let marker = home.root().join("in-tool");
    let _m = MockScript::set(&format!(
        r#"[{{ "AssistantToolUse": {{ "text": null, "tool_id": "t1", "tool_name": "bash", "tool_input": {{"command": "touch {} && sleep 3"}}, "stop": "tool_use" }} }},
            {{ "AssistantText": {{ "text": "never", "stop": "end_turn" }} }}]"#,
        marker.display()
    ));
    let l1 = home.launcher(config(), None);
    let ctx1 = attempt(&home, "parent_run_1");
    let h = l1
        .launch(request(ChildKind::Subagent, "helper", Some("go")), &ctx1)
        .await
        .unwrap();
    let transcript = h.transcript.clone().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the sub-agent never ran its tool"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);
    assert_eq!(
        ledger::read(&home.runs().join("parent_run_1"))[0].state(),
        ChildState::Interrupted
    );

    let _m = MockScript::set(
        r#"[{ "AssistantText": { "text": "continued sub answer", "stop": "end_turn" } }]"#,
    );
    let l2 = home.launcher(config(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let st = join_one(&l2, &ctx2, &h.handle).await;

    assert_eq!(
        (st.state, st.output.as_deref()),
        (ChildState::Done, Some("continued sub answer"))
    );
    assert_ne!(st.run_id, h.run_id);
    let seeded = events(st.transcript.as_ref().unwrap())
        .into_iter()
        .find_map(|e| match e {
            rupu_transcript::Event::Seed {
                source_transcript, ..
            } => source_transcript,
            _ => None,
        });
    assert_eq!(seeded, Some(transcript.display().to_string()));
}

/// A continued sub-agent starts with the skills its interrupted run had
/// loaded: the continuation records each as `skill_load { via:
/// continuation }`, so its next compaction re-attaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn a_continued_sub_agent_keeps_the_skills_it_had_loaded() {
    let home = kinds_home();
    home.skill("pdf", "PDF things.");
    home.agent(
        "helper",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 4\nskills: [pdf]",
    );
    let marker = home.root().join("in-tool");
    let _m = MockScript::set(&format!(
        r#"[{{ "AssistantToolUse": {{ "text": null, "tool_id": "s1", "tool_name": "skills.load", "tool_input": {{"name": "pdf"}}, "stop": "tool_use" }} }},
            {{ "AssistantToolUse": {{ "text": null, "tool_id": "t1", "tool_name": "bash", "tool_input": {{"command": "touch {} && sleep 3"}}, "stop": "tool_use" }} }},
            {{ "AssistantText": {{ "text": "never", "stop": "end_turn" }} }}]"#,
        marker.display()
    ));
    let l1 = home.launcher(config(), None);
    let ctx1 = attempt(&home, "parent_run_1");
    let h = l1
        .launch(request(ChildKind::Subagent, "helper", Some("go")), &ctx1)
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the sub-agent never ran its tool"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);

    let _m = MockScript::set(
        r#"[{ "AssistantText": { "text": "continued sub answer", "stop": "end_turn" } }]"#,
    );
    let l2 = home.launcher(config(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let st = join_one(&l2, &ctx2, &h.handle).await;

    assert_eq!(st.state, ChildState::Done, "{st:?}");
    let loads: Vec<(String, String)> = events(st.transcript.as_ref().unwrap())
        .into_iter()
        .filter_map(|e| match e {
            rupu_transcript::Event::SkillLoad { name, via, .. } => Some((name, via)),
            _ => None,
        })
        .collect();
    assert_eq!(loads, [("pdf".to_string(), "continuation".to_string())]);
}

/// (d) A child that finished before the pause and was never joined is
/// restored as it ended: `join` after the resume returns its original
/// answer, and nothing is run again.
#[tokio::test]
#[serial]
async fn a_child_that_finished_before_the_pause_is_restored_not_rerun() {
    let home = kinds_home();
    let l1 = home.launcher_with(
        home.assembler(config()),
        Arc::new(ScriptedProcesses::done("early answer")),
        None,
    );
    let ctx1 = attempt(&home, "parent_run_1");
    let h = l1
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx1)
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while l1.children(&ctx1.identity)[0].state != ChildState::Done {
        assert!(std::time::Instant::now() < deadline, "never finished");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);

    let after = Arc::new(ScriptedProcesses::done("rerun"));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let st = join_one(&l2, &ctx2, &h.handle).await;

    assert_eq!(
        (st.state, st.output.as_deref(), st.run_id.as_str()),
        (ChildState::Done, Some("early answer"), h.run_id.as_str())
    );
    assert!(after.spawned().is_empty());
}

/// (e) A child `join` collected before the pause stays collected: `join {}`
/// after the resume returns only the one it never collected.
#[tokio::test]
#[serial]
async fn a_child_joined_before_the_pause_is_not_returned_by_join_all_after_it() {
    let home = kinds_home();
    let l1 = home.launcher_with(
        home.assembler(config()),
        Arc::new(ScriptedProcesses::done("answer")),
        None,
    );
    let ctx1 = attempt(&home, "parent_run_1");
    let joined = l1
        .launch(request(ChildKind::Agent, "probe", Some("one")), &ctx1)
        .await
        .unwrap();
    let left = l1
        .launch(request(ChildKind::Agent, "probe", Some("two")), &ctx1)
        .await
        .unwrap();
    assert_eq!(
        join_one(&l1, &ctx1, &joined.handle).await.state,
        ChildState::Done
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while l1
        .children(&ctx1.identity)
        .iter()
        .any(|c| c.state != ChildState::Done)
    {
        assert!(std::time::Instant::now() < deadline, "never finished");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);

    let l2 = home.launcher(config(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let all = l2
        .join(&ctx2.identity, JoinSet::All, Duration::ZERO, Until::All)
        .await;

    let handles: Vec<_> = all.iter().map(|s| s.handle.as_str()).collect();
    assert_eq!(handles, [left.handle.as_str()]);
    assert_eq!(
        join_one(&l2, &ctx2, &joined.handle).await.output.as_deref(),
        Some("answer"),
        "a collected child still answers by handle"
    );
}

/// (f) A crashed attempt recorded nothing: the run that continues it stops
/// what is still running — only a pid its own records and start time say is
/// the child — records it `interrupted` and continues it. A recorded pid now
/// held by an unrelated process (one that started at another time) is never
/// signalled; that child is continued too.
#[tokio::test]
#[serial]
async fn a_crashed_attempts_live_child_is_stopped_and_continued_and_a_recycled_pid_is_left_alone() {
    use std::os::unix::process::CommandExt;
    let home = kinds_home();
    let transcripts = home.global().join("transcripts");
    let dir = home.runs().join("parent_run_1");
    let started = |handle: &str, run_id: &str, pid: u32, proc_start: Option<u64>| {
        ledger::LedgerLine::Started(ledger::Started {
            transcript: Some(transcripts.join(format!("{run_id}.jsonl"))),
            pid: Some(pid),
            pgid: Some(pid),
            proc_start,
            // The crashed parent.
            ..ledger::Started::new(handle, ChildKind::Agent, "probe", run_id, 0x7fff_fffe, 1)
        })
    };
    for run in ["run_live", "run_recycled"] {
        interrupted_transcript(&transcripts.join(format!("{run}.jsonl")), run, "probe");
    }
    // The child, still running: its run metadata names its own pid.
    let meta = transcripts.join("run_live.meta.json");
    let mut live = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            &format!(
                "echo \"{{\\\"pid\\\": $$}}\" > {}; exec sleep 30",
                meta.display()
            ),
        ])
        .process_group(0)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&meta).is_ok_and(|t| t.contains("pid")) {
        assert!(
            std::time::Instant::now() < deadline,
            "the child never wrote its metadata"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // A stranger on a recorded pid: it leads its own group too, but started
    // after the child recorded there did.
    let mut stranger = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .process_group(0)
        .spawn()
        .unwrap();
    let live_start = rupu_launch::proc::start_stamp(live.id());
    let stranger_start = rupu_launch::proc::start_stamp(stranger.id());
    ledger::append(&dir, &started("ch_live", "run_live", live.id(), live_start));
    ledger::append(
        &dir,
        &started(
            "ch_recycled",
            "run_recycled",
            stranger.id(),
            stranger_start.map(|s| s - 1),
        ),
    );

    let after = Arc::new(ScriptedProcesses::done("continued answer"));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let statuses = l2
        .join(
            &ctx2.identity,
            JoinSet::All,
            Duration::from_secs(60),
            Until::All,
        )
        .await;

    let live_exit = live.wait().unwrap();
    let stranger_alive = matches!(stranger.try_wait(), Ok(None));
    stranger.kill().unwrap();
    stranger.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(live_exit.signal(), Some(15), "the child was SIGTERMed");
    assert!(stranger_alive, "the stranger was signalled");
    let prior: Vec<_> = ledger::read(&dir)
        .iter()
        .map(ledger::ChildEntry::state)
        .collect();
    assert_eq!(prior, [ChildState::Interrupted, ChildState::Interrupted]);
    let continued: Vec<_> = statuses
        .iter()
        .map(|s| (s.handle.as_str(), s.state, s.output.as_deref()))
        .collect();
    assert_eq!(
        continued,
        [
            ("ch_live", ChildState::Done, Some("continued answer")),
            ("ch_recycled", ChildState::Done, Some("continued answer")),
        ]
    );
    let from: Vec<_> = after
        .spawned()
        .into_iter()
        .map(|l| match l.argv {
            RunArgv::Agent(r) => r.continue_from,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        from,
        [
            Some("run_live".to_string()),
            Some("run_recycled".to_string())
        ]
    );
}

/// (point 4) The sweep, for a crashed step's children: recorded
/// `interrupted` while the step's workflow run can still be resumed —
/// including a sub-agent's own child — and `abandoned` once that run has
/// ended for good.
#[test]
fn the_sweep_interrupts_a_crashed_steps_children_until_its_run_ends() {
    use crate::support::{attempt_row, orphan_in_tree, running_workflow};
    let home = kinds_home();
    let global = home.global();
    running_workflow(&global, "wf_1");
    attempt_row(&global, "wf_1", "scan", "step_run_1");
    let tree = home.runs().join("wf_1");
    let step = home.runs().join("step_run_1");
    let sub = home.runs().join("sub_1");
    ledger::append(
        &step,
        &orphan_in_tree("ch_sub", ChildKind::Subagent, "sub_1", &tree),
    );
    ledger::append(
        &sub,
        &orphan_in_tree("ch_nested", ChildKind::Agent, "run_nested", &tree),
    );
    let states = || [&step, &sub].map(|d| ledger::read(d)[0].state()).to_vec();

    let swept = rupu_launch::reaper::reap_orphaned_children(&global, Duration::ZERO);
    assert_eq!(swept.reaped.len(), 2);
    assert_eq!(states(), [ChildState::Interrupted, ChildState::Interrupted]);

    rupu_launch::reaper::reap_orphaned_children(&global, Duration::ZERO);
    assert_eq!(
        states(),
        [ChildState::Interrupted, ChildState::Interrupted],
        "a resumable run's children wait for its resume"
    );

    let store = rupu_orchestrator::RunStore::new(home.runs());
    let mut rec = store.load("wf_1").unwrap();
    rec.status = rupu_orchestrator::RunStatus::Cancelled;
    store.update(&rec).unwrap();
    rupu_launch::reaper::reap_orphaned_children(&global, Duration::ZERO);
    assert_eq!(states(), [ChildState::Abandoned, ChildState::Abandoned]);
}

/// (item 7) A crashed `rupu run`'s child is recorded `interrupted` by the
/// sweep, never `abandoned` while the run can still be continued: `rupu run
/// --continue` continues it. Once a parent's own run record says it ended
/// (here: another crashed run that later finished), its leftover child is
/// `abandoned`.
#[tokio::test]
#[serial]
async fn a_crashed_standalone_parents_child_waits_for_its_continuation() {
    use crate::support::orphan_line;
    let home = kinds_home();
    let global = home.global();
    let lone = home.runs().join("parent_run_1");
    let transcript = home.global().join("transcripts").join("run_x.jsonl");
    interrupted_transcript(&transcript, "run_x", "probe");
    let rupu_launch::ledger::LedgerLine::Started(s) =
        orphan_line("ch_lone", ChildKind::Agent, "run_x")
    else {
        unreachable!()
    };
    ledger::append(
        &lone,
        &ledger::LedgerLine::Started(ledger::Started {
            transcript: Some(transcript),
            tree: Some(lone.clone()),
            ..s
        }),
    );
    for _ in 0..2 {
        rupu_launch::reaper::reap_orphaned_children(&global, Duration::ZERO);
        assert_eq!(ledger::read(&lone)[0].state(), ChildState::Interrupted);
    }

    let after = Arc::new(ScriptedProcesses::done("continued answer"));
    let l = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx = attempt(&home, "parent_run_2");
    l.continue_children("parent_run_1", &ctx).await;
    let st = join_one(&l, &ctx, "ch_lone").await;
    assert_eq!(
        (st.state, st.output.as_deref()),
        (ChildState::Done, Some("continued answer"))
    );

    let ended = home.runs().join("run_ended");
    ledger::append(&ended, &orphan_line("ch_left", ChildKind::Agent, "run_y"));
    let record: rupu_orchestrator::RunRecord = serde_json::from_value(serde_json::json!({
        "id": "run_ended",
        "workflow_name": "agent:parent",
        "status": "completed",
        "inputs": {},
        "workspace_id": "ws_test",
        "workspace_path": "/work",
        "transcript_dir": "/work/transcripts",
        "started_at": "2026-10-12T00:00:00Z",
    }))
    .unwrap();
    rupu_orchestrator::RunStore::new(home.runs())
        .create(record, "")
        .unwrap();
    rupu_launch::reaper::reap_orphaned_children(&global, Duration::ZERO);
    assert_eq!(ledger::read(&ended)[0].state(), ChildState::Abandoned);
}

/// A `rupu run` parent stopped by a signal records its run `Failed` with the
/// stop as its typed cause: the sweep leaves its interrupted child for `rupu
/// run --continue`. One whose run failed on its own ended for good, and its
/// child is `abandoned`.
#[test]
fn a_signal_stopped_standalone_parents_child_waits_for_its_continuation() {
    use crate::support::{end_line, orphan_line};
    let home = kinds_home();
    let global = home.global();
    for (run, cause) in [
        (
            "run_stopped",
            Some(rupu_transcript::StopCause::StoppedBySignal),
        ),
        ("run_failed", None),
    ] {
        let dir = home.runs().join(run);
        ledger::append(&dir, &orphan_line("ch_1", ChildKind::Agent, "run_child"));
        ledger::append(&dir, &end_line("ch_1", ChildState::Interrupted));
        let mut record: rupu_orchestrator::RunRecord = serde_json::from_value(serde_json::json!({
            "id": run,
            "workflow_name": "agent:parent",
            "status": "failed",
            "inputs": {},
            "workspace_id": "ws_test",
            "workspace_path": "/work",
            "transcript_dir": "/work/transcripts",
            "started_at": "2026-10-12T00:00:00Z",
        }))
        .unwrap();
        record.cause = cause.map(|c| c.record());
        rupu_orchestrator::RunStore::new(home.runs())
            .create(record, "")
            .unwrap();
    }

    rupu_launch::reaper::reap_orphaned_children(&global, Duration::ZERO);

    let state = |run: &str| ledger::read(&home.runs().join(run))[0].state();
    assert_eq!(state("run_stopped"), ChildState::Interrupted);
    assert_eq!(state("run_failed"), ChildState::Abandoned);
}

/// (point 6) A workflow run that ends for good records what its attempts
/// left `interrupted` `abandoned`, judging each child by its latest record:
/// one a later attempt took on keeps that attempt's end, and the earlier
/// attempt's closed record of it is left as it was.
#[test]
fn a_finished_runs_leftover_interrupted_children_are_abandoned() {
    use crate::support::{attempt_row, end_line, orphan_line};
    let home = kinds_home();
    let global = home.global();
    attempt_row(&global, "wf_1", "scan", "step_run_1");
    attempt_row(&global, "wf_1", "scan", "step_run_2");
    let first = home.runs().join("step_run_1");
    let second = home.runs().join("step_run_2");
    for h in ["ch_left", "ch_continued"] {
        ledger::append(&first, &orphan_line(h, ChildKind::Agent, "run_x"));
        ledger::append(&first, &end_line(h, ChildState::Interrupted));
    }
    ledger::append(
        &first,
        &ledger::LedgerLine::Continued(ledger::Continued {
            handle: "ch_continued".into(),
            by_run: "step_run_2".into(),
            at: chrono::Utc::now(),
        }),
    );
    ledger::append(
        &second,
        &orphan_line("ch_continued", ChildKind::Agent, "run_y"),
    );
    ledger::append(&second, &end_line("ch_continued", ChildState::Done));

    let ended = rupu_launch::reaper::end_interrupted_of(&global, "wf_1", Duration::ZERO);

    assert_eq!(ended, [(first.clone(), "ch_left".to_string())]);
    let states: Vec<_> = ledger::read(&first)
        .iter()
        .map(ledger::ChildEntry::state)
        .collect();
    assert_eq!(states, [ChildState::Abandoned, ChildState::Interrupted]);
    assert_eq!(ledger::read(&second)[0].state(), ChildState::Done);
}

/// A workflow child whose runner fails of a stop once `stop` is set: it
/// writes its `run.json` `Failed` with the stop as its typed cause and exits.
/// Its step left a child of its own `interrupted`.
struct FailsOfAStop {
    global: std::path::PathBuf,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

const STEP_RUN: &str = "step_run_of_the_child";

impl rupu_launch::ProcessBackend for FailsOfAStop {
    fn spawn(
        &self,
        launch: &rupu_launch::ProcessLaunch,
    ) -> Result<Option<rupu_launch::SpawnedProcess>, String> {
        use crate::support::{attempt_row, end_line, orphan_line, running_workflow};
        running_workflow(&self.global, &launch.run_id);
        attempt_row(&self.global, &launch.run_id, "scan", STEP_RUN);
        let step = self.global.join("runs").join(STEP_RUN);
        ledger::append(
            &step,
            &orphan_line("ch_grand", ChildKind::Agent, "run_grand"),
        );
        ledger::append(&step, &end_line("ch_grand", ChildState::Interrupted));
        Ok(None)
    }
    fn poll(&self, run_id: &str) -> ProcState {
        if !self.stop.load(std::sync::atomic::Ordering::SeqCst) {
            return ProcState::Running;
        }
        let store = rupu_orchestrator::RunStore::new(self.global.join("runs"));
        let mut rec = store.load(run_id).unwrap();
        if rec.status != rupu_orchestrator::RunStatus::Failed {
            rec.status = rupu_orchestrator::RunStatus::Failed;
            rec.cause = Some(rupu_transcript::StopCause::StoppedBySignal.record());
            rec.finished_at = Some(chrono::Utc::now());
            store.update(&rec).unwrap();
        }
        ProcState::Failed("stopped by a signal".into())
    }
    fn terminate(&self, _: &str, _: rupu_launch::Stop) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn fails_of_a_stop(home: &Home) -> (Arc<FailsOfAStop>, Arc<std::sync::atomic::AtomicBool>) {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let backend = Arc::new(FailsOfAStop {
        global: home.global(),
        stop: Arc::clone(&stop),
    });
    (backend, stop)
}

fn state_in(dir: &std::path::Path, handle: &str) -> ChildState {
    ledger::read(dir)
        .into_iter()
        .find(|c| c.started.handle == handle)
        .map(|c| c.state())
        .expect("recorded")
}

/// A workflow child that fails of its parent's process being stopped by a
/// signal (its runner records the stop as its cause) is `interrupted`, like
/// one killed outright: what its step left `interrupted` stays so, a waiting
/// `join` collecting that end leaves it continuable, and the run that
/// continues the parent resumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn a_workflow_child_failing_of_its_parents_stop_is_interrupted_and_continued() {
    let home = kinds_home();
    let _unstop = Unstop;
    let (backend, stop) = fails_of_a_stop(&home);
    let l1 = home.launcher_with(home.assembler(config()), backend, None);
    let ctx1 = attempt(&home, "parent_run_1");
    let h = l1
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx1)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    tokio::task::spawn_blocking(|| {
        rupu_launch::reaper::stop_live_children(Duration::from_secs(5), "stopped")
    })
    .await
    .unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let st = join_one(&l1, &ctx1, &h.handle).await;
    assert_eq!(st.state, ChildState::Interrupted, "{:?}", st.error);
    let step = home.runs().join(STEP_RUN);
    assert_eq!(state_in(&step, "ch_grand"), ChildState::Interrupted);
    let parent = home.runs().join("parent_run_1");
    assert_eq!(state_in(&parent, &h.handle), ChildState::Interrupted);
    drop(l1);
    rupu_launch::reaper::reset_stopping();

    let after = Arc::new(ScriptedProcesses::done("continued answer"));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let st = join_one(&l2, &ctx2, &h.handle).await;
    assert_eq!(
        (st.state, st.output.as_deref()),
        (ChildState::Done, Some("continued answer"))
    );
    assert_eq!(
        after.spawned()[0].argv,
        RunArgv::WorkflowControl(rupu_runtime::argv::WorkflowControl::Resume {
            run_id: h.run_id.clone(),
            mode: Some(PermissionMode::Bypass),
            if_unfinished: false,
        })
    );
}

/// A workflow child stopped from outside while its parent goes on (its own
/// records say a signal ended it) is `failed` to that parent, but what its
/// step left is its parent's end to decide: kept while the parent runs (a
/// reboot's stop may reach the parent next), `abandoned` once the parent
/// ends for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn a_workflow_child_stopped_from_outside_leaves_its_step_child_to_its_parents_end() {
    let home = kinds_home();
    let (backend, stop) = fails_of_a_stop(&home);
    let l = home.launcher_with(home.assembler(config()), backend, None);
    let ctx = attempt(&home, "parent_run_1");
    let h = l
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let st = join_one(&l, &ctx, &h.handle).await;
    assert_eq!(st.state, ChildState::Failed);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let step = home.runs().join(STEP_RUN);
    assert_eq!(state_in(&step, "ch_grand"), ChildState::Interrupted);

    l.finish(&ctx.identity, ParentEnd::Completed).await;
    assert_eq!(state_in(&step, "ch_grand"), ChildState::Abandoned);
}

/// A parent that crashed after collecting a child its own records say a
/// stop ended (the reboot's signal reached the child first) still continues
/// it: the `join` does not make it unrecoverable.
#[tokio::test]
#[serial]
async fn a_joined_child_a_stop_ended_is_continued() {
    use crate::support::{orphan_line, running_workflow};
    let home = kinds_home();
    let global = home.global();
    running_workflow(&global, "run_wf");
    let store = rupu_orchestrator::RunStore::new(home.runs());
    let mut rec = store.load("run_wf").unwrap();
    rec.status = rupu_orchestrator::RunStatus::Failed;
    rec.cause = Some(rupu_transcript::StopCause::StoppedBySignal.record());
    rec.finished_at = Some(chrono::Utc::now());
    store.update(&rec).unwrap();
    let dir = home.runs().join("parent_run_1");
    ledger::append(&dir, &orphan_line("ch_1", ChildKind::Workflow, "run_wf"));
    ledger::append(&dir, &crate::support::end_line("ch_1", ChildState::Failed));
    ledger::append(
        &dir,
        &ledger::LedgerLine::Joined(ledger::Joined {
            handle: "ch_1".into(),
            at: chrono::Utc::now(),
        }),
    );

    let after = Arc::new(ScriptedProcesses::done("continued answer"));
    let l = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx = attempt(&home, "parent_run_2");
    l.continue_children("parent_run_1", &ctx).await;
    let st = join_one(&l, &ctx, "ch_1").await;
    assert_eq!(
        (st.state, st.output.as_deref()),
        (ChildState::Done, Some("continued answer"))
    );
}
