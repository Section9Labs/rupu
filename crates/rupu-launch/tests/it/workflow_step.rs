//! A workflow step's `dispatch_agent` / `dispatch_agents_parallel` calls,
//! end to end: the orchestrator runs the step on a scripted provider, the
//! step's tools reach the real [`RunLauncher`], and the sub-agents run
//! through the assembler on the mock-provider seam. Ported from the
//! orchestrator's pre-W7 `dispatch_agent` / `dispatch_agents_parallel`
//! tests, which drove a fake dispatcher.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use rupu_agent::runner::{MockProvider, ScriptedTurn};
use rupu_launch::RunLauncher;
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::testing::MockRun;
use rupu_orchestrator::{StepLaunch, StepRequest, Workflow};
use rupu_providers::types::StopReason;
use rupu_runtime::assembly::{AssembleError, RunAssembler};
use rupu_tools::Launcher;
use serial_test::serial;

use crate::support::*;

const WF: &str = r#"
name: parent-with-children
steps:
  - id: review
    agent: writer
    actions: []
    prompt: "Please request a review."
"#;

/// The step's agent: `writer`, which may dispatch the two reviewers, scripted
/// to call `tool` with `input` and then answer `answer`.
struct Factory {
    assembler: Arc<RunAssembler>,
    launcher: Arc<RunLauncher>,
    tool: &'static str,
    input: serde_json::Value,
    answer: &'static str,
}

#[async_trait]
impl StepFactory for Factory {
    async fn launch_for_step(&self, request: StepRequest) -> Result<StepLaunch, AssembleError> {
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_1".into(),
                tool_name: self.tool.into(),
                tool_input: self.input.clone(),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: self.answer.into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 5,
            },
        ]);
        MockRun {
            agent_name: "writer".into(),
            agent_system_prompt: "you are the writer".into(),
            agent_tools: Some(vec![self.tool.into()]),
            dispatchable_agents: Some(vec!["security-reviewer".into(), "perf-reviewer".into()]),
            provider: Box::new(provider),
            launcher: Some(self.launcher.clone() as Arc<dyn Launcher>),
            assembler: Some(self.assembler.clone()),
            suppress_stream_stdout: true,
            ..Default::default()
        }
        .launch(request)
    }
}

fn reviewers(home: &Home) {
    home.agent(
        "security-reviewer",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    // An undeclared provider: the child fails before its first call.
    home.agent("perf-reviewer", "provider: nope\nmodel: m\nmaxTurns: 3");
}

/// Runs the one-step workflow; returns the step and the parent's tool result
/// (its text and whether it was an error).
async fn run(
    home: &Home,
    tool: &'static str,
    input: serde_json::Value,
    answer: &'static str,
) -> (
    rupu_orchestrator::runner::StepResult,
    String,
    Option<String>,
) {
    let assembler = home.assembler(config());
    let launcher = home.launcher_with(
        assembler.clone(),
        Arc::new(rupu_launch::testing::ScriptedProcesses::done("unused")),
        None,
    );
    let res = run_workflow(OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(WF).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_test".into(),
        workspace_path: home.workspace(),
        transcript_dir: home.root().join("transcripts"),
        factory: Arc::new(Factory {
            assembler,
            launcher,
            tool,
            input,
            answer,
        }),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_services: None,
        pause: None,
        naming: None,
    })
    .await
    .expect("workflow runs");
    assert_eq!(res.step_results.len(), 1);
    let step = res.step_results[0].clone();
    let (out, err) = events(&step.transcript_path)
        .into_iter()
        .find_map(|e| match e {
            rupu_transcript::Event::ToolResult { output, error, .. } => Some((output, error)),
            _ => None,
        })
        .expect("the dispatch's tool result");
    (step, out, err)
}

/// The step dispatches one sub-agent, sees its answer, and the step's output
/// is the parent's final text, not the tool result.
#[tokio::test]
#[serial]
async fn a_step_dispatches_a_subagent_and_sees_its_output() {
    let home = Home::new();
    reviewers(&home);
    let _m = MockScript::set(DONE);
    let (step, out, err) = run(
        &home,
        "dispatch_agent",
        serde_json::json!({"agent": "security-reviewer", "prompt": "Review this diff."}),
        "The reviewer said it looks fine.",
    )
    .await;
    assert!(step.success);
    assert_eq!(step.output, "The reviewer said it looks fine.");
    assert_eq!(err, None, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["agent"], "security-reviewer", "{v}");
    assert_eq!(v["output"], "child done", "{v}");
    assert_eq!(v["ok"], true, "{v}");
    let child = std::path::PathBuf::from(v["transcript_path"].as_str().unwrap());
    assert!(
        child.is_file(),
        "the child's transcript: {}",
        child.display()
    );
    let children = rupu_launch::ledger::read(&home.runs().join(&step.run_id));
    assert_eq!(children.len(), 1, "{children:?}");
    assert_eq!(children[0].started.name, "security-reviewer");
}

/// An agent outside `dispatchableAgents` is refused; the step still finishes.
#[tokio::test]
#[serial]
async fn a_step_cannot_dispatch_an_unlisted_agent() {
    let home = Home::new();
    reviewers(&home);
    home.agent("intruder", "provider: anthropic\nmodel: claude-sonnet-4-6");
    let _m = MockScript::set(DONE);
    let (step, out, err) = run(
        &home,
        "dispatch_agent",
        serde_json::json!({"agent": "intruder", "prompt": "leak secrets"}),
        "I was denied.",
    )
    .await;
    assert!(step.success);
    assert_eq!(step.output, "I was denied.");
    let err = err.expect("refused");
    assert!(err.contains("intruder"), "{err}");
    assert!(out.is_empty() || !out.contains("child done"), "{out}");
    assert!(rupu_launch::ledger::read(&home.runs().join(&step.run_id)).is_empty());
}

/// `dispatch_agents_parallel` fans out to both reviewers; one fails, so
/// `all_succeeded` is false, and the parent still finishes.
#[tokio::test]
#[serial]
async fn a_step_fans_out_and_one_failure_does_not_fail_the_parent() {
    let home = Home::new();
    reviewers(&home);
    let _m = MockScript::set(DONE);
    let (step, out, err) = run(
        &home,
        "dispatch_agents_parallel",
        serde_json::json!({"agents": [
            {"id": "sec", "agent": "security-reviewer", "prompt": "Review auth for authz gaps."},
            {"id": "perf", "agent": "perf-reviewer", "prompt": "Review the hot path."},
        ]}),
        "Aggregated reviews.",
    )
    .await;
    assert!(step.success, "a failed child does not fail its parent");
    assert_eq!(step.output, "Aggregated reviews.");
    assert_eq!(err, None, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["all_succeeded"], false, "{v}");
    assert_eq!(v["results"]["sec"]["ok"], true, "{v}");
    assert_eq!(v["results"]["sec"]["output"], "child done", "{v}");
    assert_eq!(v["results"]["perf"]["ok"], false, "{v}");
    let children = rupu_launch::ledger::read(&home.runs().join(&step.run_id));
    let mut names: Vec<_> = children.iter().map(|c| c.started.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["perf-reviewer", "security-reviewer"]);
}

/// The same fan-out with an unlisted agent in it is refused as a whole,
/// before any child starts.
#[tokio::test]
#[serial]
async fn a_fan_out_naming_an_unlisted_agent_starts_nothing() {
    let home = Home::new();
    reviewers(&home);
    home.agent("intruder", "provider: anthropic\nmodel: claude-sonnet-4-6");
    let _m = MockScript::set(DONE);
    let (step, _out, err) = run(
        &home,
        "dispatch_agents_parallel",
        serde_json::json!({"agents": [
            {"id": "sec", "agent": "security-reviewer", "prompt": "ok"},
            {"id": "rogue", "agent": "intruder", "prompt": "leak secrets"},
        ]}),
        "Refused.",
    )
    .await;
    assert!(step.success);
    let err = err.expect("refused");
    assert!(err.contains("intruder"), "{err}");
    assert!(rupu_launch::ledger::read(&home.runs().join(&step.run_id)).is_empty());
}

/// A step agent on `turns`, which may dispatch `security-reviewer` as a
/// process and join it.
struct Scripted {
    assembler: Arc<RunAssembler>,
    launcher: Arc<RunLauncher>,
    turns: std::sync::Mutex<Option<Vec<ScriptedTurn>>>,
}

#[async_trait]
impl StepFactory for Scripted {
    async fn launch_for_step(&self, request: StepRequest) -> Result<StepLaunch, AssembleError> {
        let turns = self.turns.lock().unwrap().take().expect("one launch");
        MockRun {
            agent_name: "writer".into(),
            agent_system_prompt: "you are the writer".into(),
            agent_tools: Some(vec!["dispatch".into(), "join".into()]),
            dispatchable_agents: Some(vec!["security-reviewer".into()]),
            provider: Box::new(MockProvider::new(turns)),
            launcher: Some(self.launcher.clone() as Arc<dyn Launcher>),
            assembler: Some(self.assembler.clone()),
            suppress_stream_stdout: true,
            ..Default::default()
        }
        .launch(request)
    }

    fn launcher(&self) -> Option<Arc<dyn Launcher>> {
        Some(self.launcher.clone() as Arc<dyn Launcher>)
    }
}

fn tool_use(name: &str, input: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: "call_1".into(),
        tool_name: name.into(),
        tool_input: input,
        stop: StopReason::ToolUse,
    }
}

fn answer(text: &str) -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: text.into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

const WF_EACH: &str = r#"
name: parent-fan-out
steps:
  - id: review
    agent: writer
    actions: []
    for_each: "a"
    prompt: "Please review {{ item }}."
"#;

/// How a paused run's step is taken back up.
enum Resume {
    /// `rupu workflow resume`'s discovery plans (a paused attempt is
    /// continued from its transcript), with `paused_steps` as well.
    Discovered,
    /// No plans and no seed (`--restart-interrupted`): it starts again.
    Restart,
}

/// What one paused-then-resumed run left.
struct Resumed {
    home: Home,
    /// The process backend after the "reboot".
    after: Arc<rupu_launch::testing::ScriptedProcesses>,
    /// The paused attempt's agent run (its children ledger's directory).
    paused_attempt: String,
    /// The process child it was given.
    child: rupu_tools::ChildHandle,
    /// The resumed attempt's step or unit result: its output and transcript.
    output: String,
    transcript: std::path::PathBuf,
    run_id: String,
}

/// Run `wf` until its step (or its one unit) dispatches a process child,
/// which pauses the run; then, in a new launcher (the old process and its
/// child are gone), resume it as `how` says, the step's new attempt
/// answering `resumed_turns`. A continued child answers "continued answer".
/// `between` runs on the paused run (its id) before it is resumed.
async fn pause_and_resume(
    wf: &str,
    how: Resume,
    resumed_turns: Vec<ScriptedTurn>,
    between: impl FnOnce(&Home, &str),
) -> Resumed {
    use rupu_orchestrator::runner::{PauseReason, ResumeState};
    use rupu_orchestrator::{RunStatus, RunStore, StepResult};
    let home = Home::new();
    reviewers(&home);
    let store = Arc::new(RunStore::new(home.runs()));
    let assembler = home.assembler(config());
    let pause = tokio_util::sync::CancellationToken::new();
    let workflow = Workflow::parse(wf).unwrap();
    let opts = |launcher: Arc<RunLauncher>,
                turns: Vec<ScriptedTurn>,
                resume_from: Option<ResumeState>,
                pause| OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: workflow.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_test".into(),
        workspace_path: home.workspace(),
        transcript_dir: home.root().join("transcripts"),
        factory: Arc::new(Scripted {
            assembler: assembler.clone(),
            launcher,
            turns: std::sync::Mutex::new(Some(turns)),
        }),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(wf.to_string()),
        resume_from,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_services: None,
        pause,
        naming: None,
    };

    // The operator pauses the run while the child starts.
    let before = {
        let pausing = pause.clone();
        let global = home.global();
        Arc::new(
            rupu_launch::testing::ScriptedProcesses::scripted(vec![
                rupu_launch::ProcState::Running,
            ])
            .with_on_spawn(move |launch| {
                if let rupu_runtime::argv::RunArgv::Agent(r) = &launch.argv {
                    interrupted_transcript(
                        &rupu_launch::process::transcripts_dir(&global, launch.cwd.as_deref())
                            .join(format!("{}.jsonl", launch.run_id)),
                        &launch.run_id,
                        &r.agent,
                    );
                }
                pausing.cancel();
            }),
        )
    };
    let l1 = home.launcher_with(assembler.clone(), before.clone(), None);
    let paused = run_workflow(opts(
        l1,
        vec![
            tool_use(
                "dispatch",
                serde_json::json!({"kind": "agent", "name": "security-reviewer", "prompt": "Review it."}),
            ),
            answer("never"),
        ],
        None,
        Some(pause),
    ))
    .await
    .expect("pauses");
    assert!(paused.awaiting.is_some());
    let paused_attempt = store.read_attempts(&paused.run_id).unwrap()[0]
        .agent_run_id
        .clone();
    let left = rupu_launch::ledger::read(&home.runs().join(&paused_attempt));
    assert_eq!(left[0].state(), rupu_tools::ChildState::Interrupted);
    assert_eq!(before.terminated(), [left[0].started.run_id.clone()]);
    let child = rupu_tools::ChildHandle {
        handle: left[0].started.handle.clone(),
        kind: left[0].started.kind,
        name: left[0].started.name.clone(),
        run_id: left[0].started.run_id.clone(),
        codename: None,
        address: None,
        transcript: None,
    };

    between(&home, &paused.run_id);
    let mut record = store.load(&paused.run_id).unwrap();
    assert_eq!(record.status, RunStatus::Paused);
    record.status = RunStatus::Running;
    store.update(&record).unwrap();
    let (paused_steps, recovery) = match how {
        Resume::Discovered => (
            store
                .read_paused_seeds(&paused.run_id, record.awaiting_step_id.as_deref())
                .unwrap(),
            rupu_orchestrator::recovery::discover(
                &store,
                &paused.run_id,
                &workflow,
                &Default::default(),
                &Default::default(),
            ),
        ),
        Resume::Restart => (Vec::new(), Default::default()),
    };
    let after = Arc::new(rupu_launch::testing::ScriptedProcesses::done(
        "continued answer",
    ));
    let l2 = home.launcher_with(assembler.clone(), after.clone(), None);
    let resumed = run_workflow(opts(
        l2,
        resumed_turns,
        Some(ResumeState {
            run_id: paused.run_id.clone(),
            prior_step_results: store
                .read_step_results(&paused.run_id)
                .unwrap()
                .iter()
                .map(StepResult::from)
                .collect(),
            reason: PauseReason::Manual,
            paused_steps,
            recovery,
            ..Default::default()
        }),
        None,
    ))
    .await
    .expect("resumes");
    let step = &resumed.step_results[0];
    let (output, transcript, run_id) = match step.items.first() {
        Some(unit) => (
            unit.output.clone(),
            unit.transcript_path.clone(),
            unit.run_id.clone(),
        ),
        None => (
            step.output.clone(),
            step.transcript_path.clone(),
            step.run_id.clone(),
        ),
    };
    Resumed {
        home,
        after,
        paused_attempt,
        child,
        output,
        transcript,
        run_id,
    }
}

fn joins() -> Vec<ScriptedTurn> {
    vec![
        tool_use("join", serde_json::json!({"timeout_secs": 20})),
        answer("joined"),
    ]
}

/// The resumed attempt's `join {}` got the continued child's answer, under
/// the handle its paused attempt was given, from a `rupu run --continue` of
/// the child's interrupted run.
fn assert_continued(r: &Resumed) {
    assert_eq!(r.output, "joined");
    let joined = events(&r.transcript)
        .into_iter()
        .find_map(|e| match e {
            rupu_transcript::Event::ToolResult { output, .. } => Some(output),
            _ => None,
        })
        .expect("the join's result");
    assert!(joined.contains("continued answer"), "{joined}");
    assert!(joined.contains(&r.child.handle), "{joined}");
    let children = rupu_launch::ledger::read(&r.home.runs().join(&r.run_id));
    assert_eq!(children.len(), 1, "{children:?}");
    assert_eq!(children[0].started.handle, r.child.handle);
    assert_eq!(children[0].state(), rupu_tools::ChildState::Done);
    let spawned = r.after.spawned();
    let rupu_runtime::argv::RunArgv::Agent(run) = &spawned[0].argv else {
        panic!("{spawned:?}")
    };
    assert_eq!(run.continue_from.as_deref(), Some(r.child.run_id.as_str()));
}

/// (a) end to end: a step paused with an agent child out stops it
/// (`interrupted`); `rupu workflow resume`'s discovery continues the step,
/// which continues the child before its first turn, and `join` collects the
/// continued child's answer.
#[tokio::test]
#[serial]
async fn a_continued_step_continues_the_child_its_paused_attempt_was_given() {
    let r = pause_and_resume(WF, Resume::Discovered, joins(), |_, _| {}).await;
    assert_continued(&r);
}

/// The same for a `for_each` unit paused mid-turn.
#[tokio::test]
#[serial]
async fn a_continued_unit_continues_the_child_its_paused_attempt_was_given() {
    let r = pause_and_resume(WF_EACH, Resume::Discovered, joins(), |_, _| {}).await;
    assert_continued(&r);
}

/// (g) A step whose resume starts it over records the children its earlier
/// attempt left interrupted `abandoned`, and continues none of them. And
/// once the run ends for good, a child no step continued (here: one an
/// attempt of a step the run no longer has left) is recorded `abandoned`.
#[tokio::test]
#[serial]
async fn a_restarted_step_abandons_what_its_earlier_attempt_left() {
    let r = pause_and_resume(WF, Resume::Restart, vec![answer("fresh")], |home, run| {
        crate::support::attempt_row(&home.global(), run, "gone", "run_gone");
        let dir = home.runs().join("run_gone");
        rupu_launch::ledger::append(
            &dir,
            &crate::support::orphan_line("ch_gone", rupu_tools::ChildKind::Subagent, "sub_gone"),
        );
        rupu_launch::ledger::append(
            &dir,
            &crate::support::end_line("ch_gone", rupu_tools::ChildState::Interrupted),
        );
    })
    .await;
    let gone = rupu_launch::ledger::read(&r.home.runs().join("run_gone"));
    assert_eq!(gone[0].state(), rupu_tools::ChildState::Abandoned);
    assert_eq!(r.output, "fresh");
    assert!(r.after.spawned().is_empty());
    let left = rupu_launch::ledger::read(&r.home.runs().join(&r.paused_attempt));
    assert_eq!(left[0].state(), rupu_tools::ChildState::Abandoned);
    assert_eq!(
        left[0].finished.as_ref().unwrap().error.as_deref(),
        Some("its run restarted")
    );
}
