//! Taking a paused or crashed parent's children on (W7 continue-on-resume):
//! what is continued, launched afresh, restored or abandoned, under which
//! bounds, and what the ledgers say afterwards.

use std::sync::Arc;
use std::time::Duration;

use rupu_launch::ledger;
use rupu_launch::testing::ScriptedProcesses;
use rupu_launch::{Stop, Subprocess};
use rupu_runtime::argv::{RunArgv, WorkflowControl, WorkflowRef};
use rupu_tools::{
    ChildKind, ChildState, ChildTokens, JoinSet, Launcher, ParentEnd, PermissionMode, Prompter,
    Until,
};
use serial_test::serial;

use crate::support::{
    config, interrupted_transcript, interruptible, orphan_line, request, running_workflow, Home,
};
use crate::w7::{attempt, join_one, kinds_home};

fn runs_dir(home: &Home, run: &str) -> std::path::PathBuf {
    home.runs().join(run)
}

fn entry(home: &Home, run: &str, handle: &str) -> ledger::ChildEntry {
    ledger::read(&runs_dir(home, run))
        .into_iter()
        .find(|c| c.started.handle == handle)
        .unwrap_or_else(|| panic!("{handle} is not in {run}'s ledger"))
}

/// Write `rows` usage rows of (input, output) tokens to `run`'s usage
/// ledger.
fn usage(home: &Home, run: &str, rows: &[(&str, u64, u64)]) {
    use std::io::Write;
    let path = home.runs().join(run).join("usage.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for (id, input, output) in rows {
        let row = rupu_runtime::usage_ledger::LedgerRow {
            v: 1,
            id: (*id).into(),
            at: chrono::Utc::now(),
            kind: rupu_runtime::usage_ledger::LedgerKind::Turn,
            step_id: None,
            unit_index: None,
            unit_key: None,
            agent_run_id: run.into(),
            parent_agent_run_id: None,
            transcript: "/t.jsonl".into(),
            agent: "probe".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            input_tokens: *input,
            output_tokens: *output,
            cached_tokens: 0,
            cache_write_tokens: 0,
            rollup: false,
        };
        writeln!(f, "{}", serde_json::to_string(&row).unwrap()).unwrap();
    }
}

/// A run record of `status` for workflow run `run_id`, finished at `at`.
fn workflow_record(home: &Home, run_id: &str, status: &str, at: chrono::DateTime<chrono::Utc>) {
    let store = rupu_orchestrator::RunStore::new(home.runs());
    let mut rec = store.load(run_id).unwrap();
    rec.status = match status {
        "failed" => rupu_orchestrator::RunStatus::Failed,
        "completed" => rupu_orchestrator::RunStatus::Completed,
        other => panic!("{other}"),
    };
    rec.error_message = Some("step `scan` failed: boom".into());
    rec.finished_at = Some(at);
    store.update(&rec).unwrap();
}

/// (item 1) A workflow child stopped by its parent's pause through the real
/// backend is left resumable: its run is not recorded `Cancelled` the way an
/// operator's cancel is, and the ledger says WE interrupted it. The run that
/// continues the parent resumes it. Once an operator cancels it, the next
/// continuation restores it `failed` and runs nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn a_paused_workflow_child_is_resumed_and_an_operator_cancel_stands() {
    let home = kinds_home();
    let real =
        Arc::new(Subprocess::new("/bin/sh", home.global()).with_pre_args(["-c", "sleep 30", "sh"]));
    let l1 = home.launcher_with(home.assembler(config()), real, None);
    let ctx1 = attempt(&home, "parent_run_1");
    let h = l1
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx1)
        .await
        .unwrap();
    // What its runner would have written by now.
    running_workflow(&home.global(), &h.run_id);
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);

    let store = rupu_orchestrator::RunStore::new(home.runs());
    assert_eq!(
        store.load(&h.run_id).unwrap().status,
        rupu_orchestrator::RunStatus::Running,
        "an interrupt is not an operator's cancel"
    );
    assert_eq!(
        entry(&home, "parent_run_1", &h.handle).state(),
        ChildState::Interrupted
    );

    let after = Arc::new(ScriptedProcesses::scripted(vec![
        rupu_launch::ProcState::Running,
    ]));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    assert!(matches!(
        &after.spawned()[0].argv,
        RunArgv::WorkflowControl(WorkflowControl::Resume { run_id, .. }) if *run_id == h.run_id
    ));
    l2.finish(&ctx2.identity, ParentEnd::Paused).await;
    assert_eq!(after.stops(), [(h.run_id.clone(), Stop::Interrupt)]);
    drop(l2);

    store
        .cancel(&h.run_id, "operator", "not needed", chrono::Utc::now())
        .unwrap();
    let again = Arc::new(ScriptedProcesses::done("rerun"));
    let l3 = home.launcher_with(home.assembler(config()), again.clone(), None);
    let ctx3 = attempt(&home, "parent_run_3");
    l3.continue_children("parent_run_2", &ctx3).await;
    let st = join_one(&l3, &ctx3, &h.handle).await;
    assert_eq!(
        (st.state, st.error.as_deref()),
        (ChildState::Failed, Some("not needed"))
    );
    assert!(again.spawned().is_empty(), "a cancelled child was re-run");
}

/// (item 1) A crashed parent's children that ended on their own after the
/// crash are restored with their real end, never run again: a workflow child
/// whose run failed, an agent child whose run failed.
#[tokio::test]
#[serial]
async fn a_child_that_failed_on_its_own_after_its_parent_crashed_is_restored_failed() {
    let home = kinds_home();
    let dir = runs_dir(&home, "parent_run_1");
    running_workflow(&home.global(), "run_wf");
    ledger::append(&dir, &orphan_line("ch_wf", ChildKind::Workflow, "run_wf"));
    workflow_record(&home, "run_wf", "failed", chrono::Utc::now());
    let transcript = home.global().join("transcripts").join("run_ag.jsonl");
    interrupted_transcript(&transcript, "run_ag", "probe");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(b"{\"type\":\"run_complete\",\"data\":{\"run_id\":\"run_ag\",\"status\":\"error\",\"total_tokens\":0,\"duration_ms\":1,\"error\":\"the model refused\"}}\n")
        })
        .unwrap();
    let rupu_launch::ledger::LedgerLine::Started(s) =
        orphan_line("ch_ag", ChildKind::Agent, "run_ag")
    else {
        unreachable!()
    };
    ledger::append(
        &dir,
        &ledger::LedgerLine::Started(ledger::Started {
            transcript: Some(transcript),
            ..s
        }),
    );

    let after = Arc::new(ScriptedProcesses::done("rerun"));
    let l = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx = attempt(&home, "parent_run_2");
    l.continue_children("parent_run_1", &ctx).await;
    let all = l
        .join(&ctx.identity, JoinSet::All, Duration::ZERO, Until::All)
        .await;

    let got: Vec<_> = all
        .iter()
        .map(|s| (s.handle.as_str(), s.state, s.error.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "ch_wf",
                ChildState::Failed,
                Some("step `scan` failed: boom")
            ),
            ("ch_ag", ChildState::Failed, Some("the model refused")),
        ]
    );
    assert!(after.spawned().is_empty(), "a failed child was re-run");
    let prior: Vec<_> = ledger::read(&dir).iter().map(|c| c.state()).collect();
    assert_eq!(prior, [ChildState::Failed, ChildState::Failed]);
}

/// (item 3, D1) A child that never started — one still waiting for a slot,
/// and a process child stopped before it wrote its transcript or its
/// `run.json` — is launched afresh on continue under its same handle, from
/// the request its `started` line recorded. One that did start is continued.
#[tokio::test]
#[serial]
async fn a_child_that_never_started_is_launched_afresh() {
    let home = kinds_home();
    let global = home.global();
    // The first child leaves a transcript; the rest never get that far.
    let backend = Arc::new(
        rupu_launch::testing::ScriptedProcesses::scripted(vec![rupu_launch::ProcState::Running])
            .with_on_spawn(move |launch| {
                if let RunArgv::Agent(r) = &launch.argv {
                    if r.prompt.as_deref() == Some("one") {
                        interrupted_transcript(
                            &rupu_launch::process::transcripts_dir(&global, launch.cwd.as_deref())
                                .join(format!("{}.jsonl", launch.run_id)),
                            &launch.run_id,
                            &r.agent,
                        );
                    }
                }
            }),
    );
    let l1 = rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: home.assembler(config()),
        store: Arc::new(rupu_orchestrator::RunStore::new(home.runs())),
        events: None,
        namer: None,
        backend,
        runtime: tokio::runtime::Handle::current(),
        max_concurrent: 2,
        generation: None,
        project: None,
    });
    let ctx1 = attempt(&home, "parent_run_1");
    let started = l1
        .launch(request(ChildKind::Agent, "probe", Some("one")), &ctx1)
        .await
        .unwrap();
    let wf = l1
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx1)
        .await
        .unwrap();
    let queued = l1
        .launch(request(ChildKind::Agent, "probe", Some("two")), &ctx1)
        .await
        .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);
    for h in [&started, &wf, &queued] {
        assert_eq!(
            entry(&home, "parent_run_1", &h.handle).state(),
            ChildState::Interrupted
        );
    }
    assert_eq!(
        entry(&home, "parent_run_1", &queued.handle).started.pid,
        None
    );

    let after = Arc::new(ScriptedProcesses::done("answer"));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let all = l2
        .join(
            &ctx2.identity,
            JoinSet::All,
            Duration::from_secs(30),
            Until::All,
        )
        .await;

    assert!(all.iter().all(|s| s.state == ChildState::Done), "{all:?}");
    let spawned = after.spawned();
    let agent = |prompt: &str| {
        spawned
            .iter()
            .find_map(|l| match &l.argv {
                RunArgv::Agent(r)
                    if r.prompt.as_deref() == Some(prompt)
                        || r.prompt.is_none() && prompt == "-" =>
                {
                    Some(r.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no agent launch for {prompt}: {spawned:?}"))
    };
    let continued = agent("-");
    assert_eq!(
        continued.continue_from.as_deref(),
        Some(started.run_id.as_str())
    );
    let fresh = agent("two");
    assert_eq!(fresh.continue_from, None);
    assert_ne!(fresh.run_id, queued.run_id);
    assert_eq!(fresh.codename, queued.codename);
    let RunArgv::Workflow(w) = spawned
        .iter()
        .map(|l| &l.argv)
        .find(|a| matches!(a, RunArgv::Workflow(_)))
        .expect("the workflow child launched afresh")
    else {
        unreachable!()
    };
    assert!(matches!(&w.workflow, WorkflowRef::File(f) if f.ends_with("sweep.yaml")));
    assert_eq!(w.workflow_name.as_deref(), Some("sweep"));
    let now = ledger::read(&runs_dir(&home, "parent_run_2"));
    let handles: Vec<_> = now.iter().map(|c| c.started.handle.clone()).collect();
    for h in [&started, &wf, &queued] {
        assert!(handles.contains(&h.handle), "{handles:?}");
    }
}

/// (item 4) Once a run takes a child on, the earlier ledger's record of it is
/// closed (`continued`): a sweep tick afterwards has nothing to do, and
/// nothing more is recorded there.
#[tokio::test]
#[serial]
async fn a_taken_on_childs_earlier_record_is_closed() {
    let home = kinds_home();
    let l1 = home.launcher_with(home.assembler(config()), interruptible(&home), None);
    let ctx1 = attempt(&home, "parent_run_1");
    let h = l1
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx1)
        .await
        .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);

    let l2 = home.launcher_with(
        home.assembler(config()),
        Arc::new(ScriptedProcesses::done("answer")),
        None,
    );
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    join_one(&l2, &ctx2, &h.handle).await;
    l2.finish(&ctx2.identity, ParentEnd::Completed).await;

    let prior = entry(&home, "parent_run_1", &h.handle);
    assert_eq!(prior.continued_by.as_deref(), Some("parent_run_2"));
    let swept = rupu_launch::reaper::reap_orphaned_children(&home.global(), Duration::ZERO);
    assert!(swept.reaped.is_empty(), "{swept:?}");
    let text =
        std::fs::read_to_string(runs_dir(&home, "parent_run_1").join("children.jsonl")).unwrap();
    assert!(
        text.trim_end().ends_with('}') && text.lines().last().unwrap().contains("\"continued\"")
    );
}

/// (item 6) A continued workflow child is its resume process: the ledger
/// records that process's own pid and start time, and its liveness is read
/// from them, not from its `run.json` (which still names the runner that was
/// killed until the resume claims the run). A continuing parent that crashes
/// before the resume claims the run leaves a live child the next
/// continuation stops before it resumes it again.
#[test]
fn a_workflow_childs_resume_process_is_what_identifies_it() {
    use std::os::unix::process::CommandExt;
    let home = kinds_home();
    let global = home.global();
    running_workflow(&global, "run_wf");
    let store = rupu_orchestrator::RunStore::new(home.runs());
    let mut rec = store.load("run_wf").unwrap();
    // The runner the pause killed.
    rec.runner_pid = Some(0x7fff_fffd);
    store.update(&rec).unwrap();
    let mut resume = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .process_group(0)
        .spawn()
        .unwrap();
    let dir = runs_dir(&home, "parent_run_2");
    ledger::append(&dir, &orphan_line("ch_wf", ChildKind::Workflow, "run_wf"));
    ledger::append(
        &dir,
        &ledger::LedgerLine::Spawned(ledger::Spawned {
            handle: "ch_wf".into(),
            pid: resume.id(),
            proc_start: rupu_launch::proc::start_stamp(resume.id()),
            at: chrono::Utc::now(),
        }),
    );

    let entries = rupu_launch::reaper::interrupt_orphans(&dir, Duration::from_secs(5));

    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        resume.wait().unwrap().signal(),
        Some(15),
        "the resume was stopped"
    );
    assert_eq!(entries[0].state(), ChildState::Interrupted);
}

/// (item 8) A continued child runs no wider than it was launched: a child an
/// interactive `ask` parent launched readonly stays readonly under an
/// unattended continuing parent, and its caps are what both allow — for a
/// resumed workflow child, in the link its run records, which its resume
/// runs under.
#[tokio::test]
#[serial]
async fn a_continued_child_runs_no_wider_than_it_was_launched() {
    struct Allow;
    impl Prompter for Allow {
        fn ask(&self, _: &rupu_tools::PromptRequest<'_>) -> rupu_tools::PromptAnswer {
            rupu_tools::PromptAnswer::Allow
        }
    }
    let home = kinds_home();
    let l1 = home.launcher_with(home.assembler(config()), interruptible(&home), None);
    let mut ctx1 = attempt(&home, "parent_run_1");
    ctx1.call.spawn_ceiling = Some(PermissionMode::Ask);
    ctx1.services.prompter = Some(Arc::new(Allow));
    Arc::make_mut(&mut ctx1.identity).launch.agents = vec!["probe".into()];
    let h = l1
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx1)
        .await
        .unwrap();
    let wf = l1
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx1)
        .await
        .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);

    let after = Arc::new(ScriptedProcesses::done("answer"));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let mut ctx2 = attempt(&home, "parent_run_2");
    ctx2.call.spawn_ceiling = Some(PermissionMode::Ask);
    Arc::make_mut(&mut ctx2.identity).launch.agents = vec!["probe".into(), "helper".into()];
    l2.continue_children("parent_run_1", &ctx2).await;
    join_one(&l2, &ctx2, &h.handle).await;
    join_one(&l2, &ctx2, &wf.handle).await;

    // The resumed workflow runs under the link its run records.
    let resumed = rupu_orchestrator::RunStore::new(home.runs())
        .read_launch_link(&wf.run_id)
        .expect("the resumed workflow's link");
    assert!(after.spawned().iter().any(|l| matches!(
        &l.argv,
        RunArgv::WorkflowControl(WorkflowControl::Resume {
            mode: Some(PermissionMode::Readonly),
            ..
        })
    )));
    assert_eq!(
        (
            resumed.ceiling,
            resumed.agents.clone(),
            resumed.parent_run.as_str()
        ),
        (
            PermissionMode::Readonly,
            vec!["probe".to_string()],
            "parent_run_2"
        )
    );
    let run = after
        .spawned()
        .into_iter()
        .find_map(|l| match l.argv {
            RunArgv::Agent(r) => Some(r),
            _ => None,
        })
        .expect("the continued agent child");
    let link = run.launch.as_ref().expect("a link");
    assert_eq!(
        (
            run.mode,
            link.ceiling,
            link.agents.clone(),
            link.parent_run.as_str()
        ),
        (
            Some(PermissionMode::Readonly),
            PermissionMode::Readonly,
            vec!["probe".to_string()],
            "parent_run_2"
        )
    );
}

/// (item 9) `join` reports a child's tokens over all its runs: a continued
/// agent child's earlier run's and its continuation's; a resumed workflow
/// child's once, from its one ledger, which the resume appends to.
#[tokio::test]
#[serial]
async fn join_reports_a_childs_tokens_over_all_its_runs() {
    let home = kinds_home();
    let l1 = home.launcher_with(home.assembler(config()), interruptible(&home), None);
    let ctx1 = attempt(&home, "parent_run_1");
    let agent = l1
        .launch(request(ChildKind::Agent, "probe", Some("go")), &ctx1)
        .await
        .unwrap();
    let wf = l1
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx1)
        .await
        .unwrap();
    usage(&home, &agent.run_id, &[("a1", 100, 10)]);
    usage(&home, &wf.run_id, &[("w1", 7, 3)]);
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);

    let global = home.global();
    let wf_run = wf.run_id.clone();
    let runs = home.runs();
    let after = Arc::new(
        ScriptedProcesses::done("answer").with_on_spawn(move |launch| {
            let (run, id, n) = match &launch.argv {
                RunArgv::Agent(r) => (r.run_id.clone(), "a2", 5),
                _ => (wf_run.clone(), "w2", 1),
            };
            let _ = &global;
            let path = runs.join(&run).join("usage.jsonl");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let row = serde_json::json!({
                "v": 1, "id": id, "at": "2026-10-12T00:00:00Z", "kind": "turn",
                "agent_run_id": run, "transcript": "/t.jsonl", "agent": "probe",
                "provider": "anthropic", "model": "claude-sonnet-4-6",
                "input_tokens": n, "output_tokens": n,
            });
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(f, "{row}").unwrap();
        }),
    );
    let l2 = home.launcher_with(home.assembler(config()), after, None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let a = join_one(&l2, &ctx2, &agent.handle).await;
    let w = join_one(&l2, &ctx2, &wf.handle).await;

    assert_eq!(
        a.tokens,
        Some(ChildTokens {
            input: 105,
            output: 15
        })
    );
    assert_eq!(
        w.tokens,
        Some(ChildTokens {
            input: 8,
            output: 4
        })
    );
}

/// `run_id`'s run record ended `Failed` with `cause` as its typed cause
/// (none: a failure of its own).
fn failed_with(home: &Home, run_id: &str, cause: Option<rupu_transcript::StopCause>) {
    let store = rupu_orchestrator::RunStore::new(home.runs());
    let mut rec = store.load(run_id).unwrap();
    rec.status = rupu_orchestrator::RunStatus::Failed;
    rec.error_message = Some("step `scan` failed: boom".into());
    rec.finished_at = Some(chrono::Utc::now());
    rec.cause = cause.map(|c| c.record());
    store.update(&rec).unwrap();
}

/// An agent transcript closed by `run_complete` with `status` and, as its
/// typed cause, `cause`.
fn closed_transcript(
    path: &std::path::Path,
    run_id: &str,
    status: &str,
    cause: Option<rupu_transcript::StopCause>,
) {
    use std::io::Write;
    interrupted_transcript(path, run_id, "probe");
    let line = serde_json::json!({
        "type": "run_complete",
        "data": {
            "run_id": run_id, "status": status, "total_tokens": 0, "duration_ms": 1,
            "error": "it ended",
            "outcome": cause.map(|c| c.record()),
        }
    });
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(f, "{line}").unwrap();
}

/// (reboot race) Children that recorded their own end before their parent
/// recorded the stop: one ended by the signal (its record's typed cause says
/// so) is continued on resume, the way an interrupted one is, though its
/// parent had already recorded it failed; one that failed for a reason of
/// its own is restored failed and never run again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn a_child_ended_by_the_signal_before_its_parent_is_continued_and_a_real_failure_stands() {
    use rupu_transcript::StopCause;
    let home = kinds_home();
    let global = home.global();
    let runs = home.runs();
    let workflows = std::sync::atomic::AtomicUsize::new(0);
    let backend = Arc::new(
        ScriptedProcesses::scripted(vec![rupu_launch::ProcState::Failed("it ended".into())])
            .with_on_spawn(move |launch| match &launch.argv {
                RunArgv::Agent(r) => {
                    let transcript =
                        rupu_launch::process::transcripts_dir(&global, launch.cwd.as_deref())
                            .join(format!("{}.jsonl", launch.run_id));
                    if r.prompt.as_deref() == Some("stopped") {
                        closed_transcript(
                            &transcript,
                            &launch.run_id,
                            "aborted",
                            Some(StopCause::StoppedBySignal),
                        );
                    } else {
                        closed_transcript(&transcript, &launch.run_id, "error", None);
                    }
                }
                RunArgv::Workflow(_) => {
                    running_workflow(&global, &launch.run_id);
                    let store = rupu_orchestrator::RunStore::new(runs.clone());
                    let mut rec = store.load(&launch.run_id).unwrap();
                    rec.status = rupu_orchestrator::RunStatus::Failed;
                    rec.error_message = Some("step `scan` failed: boom".into());
                    rec.finished_at = Some(chrono::Utc::now());
                    if workflows.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        rec.cause = Some(StopCause::StoppedBySignal.record());
                    }
                    store.update(&rec).unwrap();
                }
                _ => {}
            }),
    );
    let l1 = home.launcher_with(home.assembler(config()), backend, None);
    let ctx1 = attempt(&home, "parent_run_1");
    let wf_stopped = l1
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx1)
        .await
        .unwrap();
    let wf_failed = l1
        .launch(request(ChildKind::Workflow, "sweep", None), &ctx1)
        .await
        .unwrap();
    let ag_stopped = l1
        .launch(request(ChildKind::Agent, "probe", Some("stopped")), &ctx1)
        .await
        .unwrap();
    let ag_failed = l1
        .launch(request(ChildKind::Agent, "probe", Some("failed")), &ctx1)
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !l1
        .children(&ctx1.identity)
        .iter()
        .all(|c| c.state == ChildState::Failed)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the children never ended"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The parent is stopped only now, after it recorded each child failed.
    l1.finish(&ctx1.identity, ParentEnd::Paused).await;
    drop(l1);
    for h in [&wf_stopped, &wf_failed, &ag_stopped, &ag_failed] {
        assert_eq!(
            entry(&home, "parent_run_1", &h.handle).state(),
            ChildState::Failed
        );
    }

    let after = Arc::new(ScriptedProcesses::done("answer"));
    let l2 = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx2 = attempt(&home, "parent_run_2");
    l2.continue_children("parent_run_1", &ctx2).await;
    let all = l2
        .join(
            &ctx2.identity,
            JoinSet::All,
            Duration::from_secs(30),
            Until::All,
        )
        .await;

    let state = |h: &rupu_tools::ChildHandle| {
        all.iter()
            .find(|s| s.handle == h.handle)
            .map(|s| (s.state, s.error.clone()))
            .unwrap_or_else(|| panic!("{} was not taken on: {all:?}", h.handle))
    };
    assert_eq!(state(&wf_stopped), (ChildState::Done, None));
    assert_eq!(state(&ag_stopped), (ChildState::Done, None));
    assert_eq!(state(&wf_failed).0, ChildState::Failed);
    assert_eq!(state(&ag_failed).0, ChildState::Failed);
    let spawned = after.spawned();
    assert_eq!(spawned.len(), 2, "{spawned:?}");
    assert!(spawned.iter().any(|l| matches!(
        &l.argv,
        RunArgv::WorkflowControl(WorkflowControl::Resume { run_id, .. }) if *run_id == wf_stopped.run_id
    )));
    assert!(spawned.iter().any(|l| matches!(
        &l.argv,
        RunArgv::Agent(r) if r.continue_from.as_deref() == Some(ag_stopped.run_id.as_str())
    )));
}

/// A crashed parent's workflow child whose runner died with it, reaped
/// `Failed` (typed: its runner died) before the parent is continued, is
/// continued; one whose run failed on its own is restored failed.
#[tokio::test]
#[serial]
async fn a_child_whose_runner_died_is_continued_after_its_parent_crashed() {
    let home = kinds_home();
    let dir = runs_dir(&home, "parent_run_1");
    for (handle, run) in [("ch_died", "run_died"), ("ch_failed", "run_failed")] {
        running_workflow(&home.global(), run);
        ledger::append(&dir, &orphan_line(handle, ChildKind::Workflow, run));
    }
    failed_with(
        &home,
        "run_died",
        Some(rupu_transcript::StopCause::RunnerDied),
    );
    failed_with(&home, "run_failed", None);

    let after = Arc::new(ScriptedProcesses::done("answer"));
    let l = home.launcher_with(home.assembler(config()), after.clone(), None);
    let ctx = attempt(&home, "parent_run_2");
    l.continue_children("parent_run_1", &ctx).await;
    let died = join_one(&l, &ctx, "ch_died").await;
    let failed = join_one(&l, &ctx, "ch_failed").await;

    assert_eq!(died.state, ChildState::Done);
    assert_eq!(
        (failed.state, failed.error.as_deref()),
        (ChildState::Failed, Some("step `scan` failed: boom"))
    );
    assert!(matches!(
        &after.spawned()[..],
        [l] if matches!(&l.argv, RunArgv::WorkflowControl(WorkflowControl::Resume { run_id, .. }) if run_id == "run_died")
    ));
}
