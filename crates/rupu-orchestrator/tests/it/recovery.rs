//! `recovery::discover` — plans for the agent attempts an interrupted run left
//! behind (Task 4 of recover-on-interrupt plan 2).
//!
//! The transcripts are real: `run_agent` + `MockProvider` writes them, and a
//! run that "died mid-turn" is a finished transcript cut back to just after
//! its first tool turn (what a killed process leaves on disk — the same shape
//! `rupu-agent`'s continuation tests use). `attempts.jsonl` and `events.jsonl`
//! are written by hand through the store's own writers so each test states
//! exactly which attempts the run made.

use chrono::Utc;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_orchestrator::executor::{Event, EventSink, JsonlSink};
use rupu_orchestrator::recovery::{discover, AttemptPlan, PlanCounts, RecoveryPlans};
use rupu_orchestrator::runs::AttemptRecord;
use rupu_orchestrator::{RunStore, Workflow};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const RUN_ID: &str = "run_recovery";

/// A run directory in a tempdir, plus the transcripts its attempts wrote.
struct Fx {
    tmp: tempfile::TempDir,
    store: RunStore,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        std::fs::create_dir_all(tmp.path().join("runs").join(RUN_ID)).unwrap();
        std::fs::create_dir_all(tmp.path().join("transcripts")).unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "alpha\nbeta\n").unwrap();
        Self { tmp, store }
    }

    fn transcript(&self, name: &str) -> PathBuf {
        self.tmp
            .path()
            .join("transcripts")
            .join(format!("{name}.jsonl"))
    }

    fn opts(&self, provider: MockProvider, name: &str) -> AgentRunOpts {
        AgentRunOpts {
            seed_source: None,
            recovery: Default::default(),
            collectors: Vec::new(),
            agent_name: "worker".into(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider: Box::new(provider),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id: name.to_string(),
            workspace_id: "ws_recovery".into(),
            workspace_path: self.tmp.path().to_path_buf(),
            transcript_path: self.transcript(name),
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: ToolContext {
                workspace_path: self.tmp.path().to_path_buf(),
                ..Default::default()
            },
            user_message: "go".into(),
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: true,
            suppress_stream_stdout: true,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: None,
            step_id: String::new(),
            on_tool_call: None,
            on_stream_event: None,
            on_usage: None,
            concerns: None,
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            scope_name: None,
            surface_tag: None,
            pause: None,
            codename: None,
        }
    }

    /// A real two-turn run: turn 1 reads `notes.txt`, turn 2 answers `answer`.
    async fn finished(&self, name: &str, answer: &str) -> PathBuf {
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_1".into(),
                tool_name: "read_file".into(),
                tool_input: serde_json::json!({ "path": "notes.txt" }),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: answer.into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
        ]);
        run_agent(self.opts(provider, name)).await.unwrap();
        self.transcript(name)
    }

    /// The same run, killed during turn 2: cut back to just after the tool
    /// turn, with a started-but-unfinished second turn. No `run_complete`.
    async fn died_mid_turn(&self, name: &str) -> PathBuf {
        let path = self.finished(name, "never reached").await;
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let end = lines
            .iter()
            .position(|v| v["type"] == "turn_end")
            .expect("a finished two-turn run has a turn_end");
        let mut cut = lines[..=end].to_vec();
        cut.push(serde_json::json!({ "type": "turn_start", "data": { "turn_idx": 1 } }));
        let body: String = cut.iter().map(|v| format!("{v}\n")).collect();
        std::fs::write(&path, body).unwrap();
        path
    }

    /// A run whose provider fails: its transcript ends `run_complete: error`.
    async fn failed(&self, name: &str) -> PathBuf {
        let provider = MockProvider::new(vec![ScriptedTurn::ProviderError("boom".into())]);
        let _ = run_agent(self.opts(provider, name)).await;
        self.transcript(name)
    }

    fn ledger(&self, rows: &[Row]) {
        for r in rows {
            self.store
                .append_attempt(
                    RUN_ID,
                    &AttemptRecord {
                        v: 1,
                        step_id: r.step.into(),
                        unit_index: r.unit,
                        sub_id: r.sub_id.map(str::to_string),
                        agent_run_id: r.run_id.clone(),
                        transcript_path: r.path.clone(),
                        host: r.host.map(str::to_string),
                        continued_from: None,
                        started_at: Utc::now(),
                    },
                )
                .unwrap();
        }
    }

    /// The events the runner emits around the same attempts, in the same
    /// order — what a run that predates the ledger has to go on. Includes a
    /// tool-call ping (no transcript path) and a torn line, both of which
    /// discovery must ignore.
    fn events(&self, rows: &[Row]) {
        let sink = JsonlSink::create(&self.store.events_path(RUN_ID)).unwrap();
        for r in rows {
            if let Some(index) = r.unit {
                sink.emit(
                    RUN_ID,
                    &Event::UnitStarted {
                        run_id: RUN_ID.into(),
                        step_id: r.step.into(),
                        index,
                        unit_key: format!("item{index}"),
                        agent: Some("worker".into()),
                        transcript_path: r.path.clone(),
                        host: r.host.map(str::to_string),
                        codename: None,
                    },
                );
            } else {
                sink.emit(
                    RUN_ID,
                    &Event::StepWorking {
                        run_id: RUN_ID.into(),
                        step_id: r.step.into(),
                        note: Some("tool call".into()),
                        transcript_path: None,
                    },
                );
                sink.emit(
                    RUN_ID,
                    &Event::StepWorking {
                        run_id: RUN_ID.into(),
                        step_id: r.step.into(),
                        note: None,
                        transcript_path: Some(r.path.clone()),
                    },
                );
            }
            sink.emit(
                RUN_ID,
                &Event::AgentStarted {
                    run_id: RUN_ID.into(),
                    step_id: r.step.into(),
                    unit_index: r.unit,
                    codename: None,
                    agent: "worker".into(),
                    provider: Some("mock".into()),
                    model: Some("mock-1".into()),
                    agent_run_id: r.run_id.clone(),
                    transcript_path: r.path.clone(),
                },
            );
        }
        drop(sink);
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(self.store.events_path(RUN_ID))
            .unwrap();
        writeln!(f, "{{\"type\":\"unit_started\",\"run_id\":\"torn").unwrap();
    }

    fn discover(
        &self,
        wf: &Workflow,
        done: &[&str],
        settled: &[(&str, &[usize])],
    ) -> RecoveryPlans {
        let done: BTreeSet<String> = done.iter().map(|s| s.to_string()).collect();
        let settled: BTreeMap<String, BTreeMap<usize, ()>> = settled
            .iter()
            .map(|(step, idxs)| {
                (
                    step.to_string(),
                    idxs.iter().map(|i| (*i, ())).collect::<BTreeMap<_, _>>(),
                )
            })
            .collect();
        discover(&self.store, RUN_ID, wf, &done, &settled)
    }
}

/// One attempt, as the ledger / the events record it.
#[derive(Clone)]
struct Row {
    step: &'static str,
    unit: Option<usize>,
    sub_id: Option<&'static str>,
    host: Option<&'static str>,
    run_id: String,
    path: PathBuf,
}

fn row(step: &'static str, unit: Option<usize>, name: &str, path: &Path) -> Row {
    Row {
        step,
        unit,
        sub_id: None,
        host: None,
        run_id: name.to_string(),
        path: path.to_path_buf(),
    }
}

const WF: &str = r#"
name: recovery
steps:
  - id: prep
    agent: worker
    actions: []
    prompt: "prep"
  - id: each
    agent: worker
    actions: []
    for_each: "a\nb\nc\nd\ne"
    prompt: "do {{ item }}"
  - id: lin
    agent: worker
    actions: []
    prompt: "the linear step"
"#;

fn wf(yaml: &str) -> Workflow {
    Workflow::parse(yaml).unwrap()
}

fn unit(plans: &RecoveryPlans, step: &str, idx: usize) -> Option<AttemptPlan> {
    plans.0.get(step).and_then(|p| p.units.get(&idx)).cloned()
}

#[tokio::test]
async fn a_unit_that_died_mid_turn_continues_from_its_transcript() {
    let fx = Fx::new();
    let t = fx.died_mid_turn("each0").await;
    fx.ledger(&[row("each", Some(0), "each0", &t)]);

    let plans = fx.discover(&wf(WF), &[], &[]);
    match unit(&plans, "each", 0) {
        Some(AttemptPlan::Continue {
            transcript,
            from_agent_run_id,
        }) => {
            assert_eq!(transcript, t);
            assert_eq!(from_agent_run_id, "each0");
        }
        other => panic!("expected Continue, got {other:?}"),
    }
    assert!(plans.0["each"].linear.is_none());
}

#[tokio::test]
async fn a_unit_that_finished_is_recovered_with_its_output() {
    let fx = Fx::new();
    let t = fx.finished("each0", "unit zero output").await;
    fx.ledger(&[row("each", Some(0), "each0", &t)]);

    let plans = fx.discover(&wf(WF), &[], &[]);
    match unit(&plans, "each", 0) {
        Some(AttemptPlan::Recovered {
            output,
            agent_run_id,
            transcript,
        }) => {
            assert_eq!(output, "unit zero output");
            assert_eq!(agent_run_id, "each0");
            assert_eq!(transcript, t);
        }
        other => panic!("expected Recovered, got {other:?}"),
    }
}

#[tokio::test]
async fn a_unit_whose_attempt_failed_restarts() {
    let fx = Fx::new();
    let t = fx.failed("each0").await;
    fx.ledger(&[row("each", Some(0), "each0", &t)]);

    let plans = fx.discover(&wf(WF), &[], &[]);
    match unit(&plans, "each", 0) {
        Some(AttemptPlan::Restart { reason }) => {
            assert!(reason.contains("failed"), "reason: {reason}");
        }
        other => panic!("expected Restart, got {other:?}"),
    }
}

/// A ledger row can name a transcript that was never written (a placement or
/// pack failure before any agent started). That's a restart, not an error.
#[tokio::test]
async fn a_unit_whose_transcript_was_never_written_restarts() {
    let fx = Fx::new();
    fx.ledger(&[row(
        "each",
        Some(0),
        "each0",
        &fx.transcript("never_written"),
    )]);

    let plans = fx.discover(&wf(WF), &[], &[]);
    match unit(&plans, "each", 0) {
        Some(AttemptPlan::Restart { reason }) => {
            assert!(reason.contains("can't be read"), "reason: {reason}");
        }
        other => panic!("expected Restart, got {other:?}"),
    }
}

#[tokio::test]
async fn a_settled_unit_gets_no_plan() {
    let fx = Fx::new();
    // Interrupted-looking transcripts, but unit 1 is covered by a successful
    // checkpoint: it replays as done and discovery must leave it alone.
    let t0 = fx.died_mid_turn("each0").await;
    let t1 = fx.died_mid_turn("each1").await;
    fx.ledger(&[
        row("each", Some(0), "each0", &t0),
        row("each", Some(1), "each1", &t1),
    ]);

    let plans = fx.discover(&wf(WF), &[], &[("each", &[1])]);
    assert!(matches!(
        unit(&plans, "each", 0),
        Some(AttemptPlan::Continue { .. })
    ));
    assert!(unit(&plans, "each", 1).is_none());

    // Every unit settled: no plan at all, and no empty entry for the step.
    let plans = fx.discover(&wf(WF), &[], &[("each", &[0, 1])]);
    assert!(plans.is_empty());
    assert!(plans.0.is_empty());
}

#[tokio::test]
async fn a_linear_step_attempt_fills_the_linear_slot() {
    let fx = Fx::new();
    let died = fx.died_mid_turn("lin1").await;
    fx.ledger(&[row("lin", None, "lin1", &died)]);

    let plans = fx.discover(&wf(WF), &[], &[]);
    let step = &plans.0["lin"];
    assert!(step.units.is_empty());
    match &step.linear {
        Some(AttemptPlan::Continue {
            transcript,
            from_agent_run_id,
        }) => {
            assert_eq!(transcript, &died);
            assert_eq!(from_agent_run_id, "lin1");
        }
        other => panic!("expected Continue, got {other:?}"),
    }

    // A linear step that had in fact finished is recovered without a re-run.
    let fx = Fx::new();
    let done = fx.finished("lin1", "linear answer").await;
    fx.ledger(&[row("lin", None, "lin1", &done)]);
    let plans = fx.discover(&wf(WF), &[], &[]);
    match &plans.0["lin"].linear {
        Some(AttemptPlan::Recovered { output, .. }) => assert_eq!(output, "linear answer"),
        other => panic!("expected Recovered, got {other:?}"),
    }
}

#[tokio::test]
async fn the_latest_attempt_per_unit_wins() {
    let fx = Fx::new();
    // Unit 0's first attempt failed, then a retry died mid-turn: the retry is
    // the one to continue. Unit 1 is the reverse (mid-turn, then failed).
    let a0 = fx.failed("each0a").await;
    let b0 = fx.died_mid_turn("each0b").await;
    let a1 = fx.died_mid_turn("each1a").await;
    let b1 = fx.failed("each1b").await;
    fx.ledger(&[
        row("each", Some(0), "each0a", &a0),
        row("each", Some(1), "each1a", &a1),
        row("each", Some(0), "each0b", &b0),
        row("each", Some(1), "each1b", &b1),
    ]);

    let plans = fx.discover(&wf(WF), &[], &[]);
    assert!(matches!(
        unit(&plans, "each", 0),
        Some(AttemptPlan::Continue { from_agent_run_id, .. }) if from_agent_run_id == "each0b"
    ));
    assert!(matches!(
        unit(&plans, "each", 1),
        Some(AttemptPlan::Restart { .. })
    ));
}

const WF_SHAPES: &str = r#"
name: shapes
steps:
  - id: done_step
    agent: worker
    actions: []
    prompt: "already finished"
  - id: gen
    agent: worker
    actions: []
    prompt: "loop member"
  - id: test
    agent: worker
    actions: []
    prompt: "loop member"
    depends_on: [gen]
  - id: par
    actions: []
    parallel:
      - id: a
        agent: worker
        prompt: "a"
      - id: b
        agent: worker
        prompt: "b"
  - id: panel
    actions: []
    panel:
      panelists: [reviewer-a]
      subject: "review me"
  - id: plain
    agent: worker
    actions: []
    prompt: "plain linear control"
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 3
"#;

#[tokio::test]
async fn done_loop_parallel_panel_and_unknown_steps_get_no_plan() {
    let fx = Fx::new();
    let died = fx.died_mid_turn("t").await;
    // Every one of these is interrupted-looking; only `plain` may be planned.
    let mut sub = row("par", None, "t", &died);
    sub.sub_id = Some("a");
    fx.ledger(&[
        row("done_step", None, "t", &died),
        row("gen", None, "t", &died),
        row("test", None, "t", &died),
        row("par", None, "t", &died),
        sub,
        row("panel", None, "t", &died),
        row("panel", Some(0), "t", &died),
        row("ghost", None, "t", &died),
        row("plain", None, "t", &died),
    ]);

    let plans = fx.discover(&wf(WF_SHAPES), &["done_step"], &[]);
    assert_eq!(
        plans.0.keys().cloned().collect::<Vec<_>>(),
        vec!["plain".to_string()],
        "plans: {plans:?}"
    );
    assert!(matches!(
        plans.0["plain"].linear,
        Some(AttemptPlan::Continue { .. })
    ));
}

/// A for_each step's units are `Some(idx)` attempts and a linear step's is
/// `None`; an attempt of the wrong shape for its step is not planned.
#[tokio::test]
async fn an_attempt_of_the_wrong_shape_for_its_step_is_ignored() {
    let fx = Fx::new();
    let died = fx.died_mid_turn("t").await;
    fx.ledger(&[
        row("each", None, "t", &died),
        row("lin", Some(0), "t", &died),
    ]);
    assert!(fx.discover(&wf(WF), &[], &[]).is_empty());
}

#[tokio::test]
async fn placed_attempts_restart_rather_than_continue_locally() {
    // A placed attempt's transcript lives on its host; continuing a local
    // mirror of it would silently run it on the wrong host.
    let yaml = r#"
name: placed
steps:
  - id: each
    agent: worker
    actions: []
    for_each: "a\nb"
    prompt: "do {{ item }}"
    distribute:
      hosts: [h1, h2]
  - id: remote_lin
    agent: worker
    actions: []
    prompt: "remote linear"
    host: h1
"#;
    let fx = Fx::new();
    let died = fx.died_mid_turn("t").await;
    let mut placed = row("each", Some(0), "t", &died);
    placed.host = Some("h1");
    // No host on the row (e.g. rebuilt from events): the step's own
    // `distribute:` still marks it placed.
    let unhosted = row("each", Some(1), "t", &died);
    fx.ledger(&[placed, unhosted, row("remote_lin", None, "t", &died)]);

    let plans = fx.discover(&wf(yaml), &[], &[]);
    for plan in [
        unit(&plans, "each", 0),
        unit(&plans, "each", 1),
        plans.0["remote_lin"].linear.clone(),
    ] {
        match plan {
            Some(AttemptPlan::Restart { reason }) => {
                assert!(reason.contains("placed on"), "reason: {reason}");
            }
            other => panic!("expected Restart, got {other:?}"),
        }
    }
}

/// The attempts for the full scenario, in dispatch order.
async fn scenario(fx: &Fx) -> Vec<Row> {
    let prep = fx.died_mid_turn("prep1").await;
    let e0 = fx.finished("each0", "unit zero output").await;
    let e1a = fx.failed("each1a").await;
    let e1b = fx.died_mid_turn("each1b").await;
    let e2 = fx.failed("each2").await;
    let e3 = fx.died_mid_turn("each3").await;
    let lin = fx.died_mid_turn("lin1").await;
    vec![
        row("prep", None, "prep1", &prep),
        row("each", Some(0), "each0", &e0),
        row("each", Some(1), "each1a", &e1a),
        row("each", Some(2), "each2", &e2),
        row("each", Some(3), "each3", &e3),
        row("each", Some(1), "each1b", &e1b),
        row("each", Some(4), "each4", &fx.transcript("never_written")),
        row("lin", None, "lin1", &lin),
    ]
}

fn plans_for_scenario(fx: &Fx) -> RecoveryPlans {
    // `prep` already has a result; unit 3 is covered by a successful
    // checkpoint.
    fx.discover(&wf(WF), &["prep"], &[("each", &[3])])
}

#[tokio::test]
async fn scenario_plans_from_the_ledger() {
    let fx = Fx::new();
    let rows = scenario(&fx).await;
    fx.ledger(&rows);

    let plans = plans_for_scenario(&fx);
    assert!(!plans.is_empty());
    assert!(!plans.0.contains_key("prep"));
    assert!(matches!(
        unit(&plans, "each", 0),
        Some(AttemptPlan::Recovered { ref output, .. }) if output == "unit zero output"
    ));
    assert!(matches!(
        unit(&plans, "each", 1),
        Some(AttemptPlan::Continue { ref from_agent_run_id, .. }) if from_agent_run_id == "each1b"
    ));
    assert!(matches!(
        unit(&plans, "each", 2),
        Some(AttemptPlan::Restart { .. })
    ));
    assert!(unit(&plans, "each", 3).is_none());
    assert!(matches!(
        unit(&plans, "each", 4),
        Some(AttemptPlan::Restart { .. })
    ));
    assert!(matches!(
        plans.0["lin"].linear,
        Some(AttemptPlan::Continue { .. })
    ));
}

/// With `attempts.jsonl` absent (a run from before the ledger), the same
/// attempts are derived from `events.jsonl` and yield the same plans.
#[tokio::test]
async fn events_fallback_yields_the_same_plans_as_the_ledger() {
    let fx = Fx::new();
    let rows = scenario(&fx).await;
    fx.ledger(&rows);
    let from_ledger = plans_for_scenario(&fx);
    assert!(!from_ledger.is_empty());

    // Same transcripts, same attempts — but the ledger is gone and only the
    // events remain.
    let ledger_file = fx
        .store
        .events_path(RUN_ID)
        .with_file_name("attempts.jsonl");
    assert!(
        ledger_file.is_file(),
        "the ledger must exist for the first half"
    );
    std::fs::remove_file(&ledger_file).unwrap();
    fx.events(&rows);
    let from_events = plans_for_scenario(&fx);

    assert_eq!(
        format!("{from_ledger:?}"),
        format!("{from_events:?}"),
        "events.jsonl fallback must reproduce the ledger's plans"
    );
}

/// An unparseable events file or a missing one is "no attempts", not a panic.
#[tokio::test]
async fn no_ledger_and_no_usable_events_means_no_plans() {
    let fx = Fx::new();
    assert!(fx.discover(&wf(WF), &[], &[]).is_empty());
    std::fs::write(fx.store.events_path(RUN_ID), "not json\n\n{\"type\":\n").unwrap();
    assert!(fx.discover(&wf(WF), &[], &[]).is_empty());
}

/// The ledger is authoritative when both exist.
#[tokio::test]
async fn the_ledger_wins_over_events_when_both_exist() {
    let fx = Fx::new();
    let died = fx.died_mid_turn("each0").await;
    let finished = fx.finished("each0_other", "from events").await;
    fx.events(&[row("each", Some(0), "each0_other", &finished)]);
    fx.ledger(&[row("each", Some(0), "each0", &died)]);

    let plans = fx.discover(&wf(WF), &[], &[]);
    assert!(matches!(
        unit(&plans, "each", 0),
        Some(AttemptPlan::Continue { ref from_agent_run_id, .. }) if from_agent_run_id == "each0"
    ));
}

#[tokio::test]
async fn summary_counts_each_kind_per_step() {
    let fx = Fx::new();
    let rows = scenario(&fx).await;
    fx.ledger(&rows);
    let plans = plans_for_scenario(&fx);

    let summary = plans.summary();
    assert_eq!(
        summary.keys().map(String::as_str).collect::<Vec<_>>(),
        ["each", "lin"]
    );
    assert_eq!(
        summary["each"],
        PlanCounts {
            continued: 1,
            recovered: 1,
            restarted: 2,
        }
    );
    assert_eq!(summary["each"].total(), 4);
    assert_eq!(
        summary["each"].to_string(),
        "1 continued · 1 recovered · 2 restarted"
    );
    assert_eq!(
        summary["lin"],
        PlanCounts {
            continued: 1,
            ..Default::default()
        }
    );
    assert_eq!(summary["lin"].to_string(), "1 continued");
    assert_eq!(plans.step("each").map(|p| p.units.len()), Some(4));
    assert!(plans.step("prep").is_none());

    let empty = RecoveryPlans::default();
    assert!(empty.is_empty());
    assert!(empty.summary().is_empty());
}
