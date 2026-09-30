//! E2e — the workflow orchestrator attaches a usage-ledger hook to every
//! local agent launch (linear / fan-out / parallel) and `UnitCompleted`
//! carries real token totals (spec 2026-09-29 §3.3–§3.4).
//!
//! Harness mirrors `tests/it/linear_runner.rs` (MockProvider-backed
//! `StepFactory`) with a tempdir `RunStore` + `JsonlSink` as in
//! `tests/it/action_step.rs`; the remote-unit case mirrors
//! `tests/it/distributed_fanout_e2e.rs`'s fake `UnitDispatcher`.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::executor::JsonlSink;
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher,
    UnitFailure, UnitOutcome,
};
use rupu_orchestrator::usage_ledger::LedgerRow;
use rupu_orchestrator::{RunStore, Workflow};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const WF: &str = r#"
name: ledger-e2e
steps:
  - id: plan
    agent: a
    prompt: "go"
  - id: fan
    agent: a
    for_each: "x\ny\nz"
    prompt: "{{ item }}"
    max_parallel: 3
  - id: par
    parallel:
      - id: p1
        agent: a
        prompt: "one"
      - id: p2
        agent: a
        prompt: "two"
"#;

/// Remote fan-out: every unit is placed on a (fake) fleet host.
const WF_REMOTE: &str = r#"
name: ledger-e2e-remote
steps:
  - id: fan
    agent: a
    for_each: "x\ny"
    prompt: "{{ item }}"
    max_parallel: 2
    distribute:
      hosts: [h1]
"#;

/// Gate panel: the panelist always reports a high finding, so the fixer runs
/// (mirrors `panel_gate_emits_panel_round_events` in `tests/it/runner_events.rs`).
const WF_PANEL: &str = r#"
name: ledger-e2e-panel
steps:
  - id: scan
    panel:
      subject: "check this"
      panelists:
        - reviewer-a
      gate:
        max_iterations: 2
        until_no_findings_at_severity_or_above: high
        fix_with: fixer-bot
"#;

/// One final-text turn per agent run, reporting 10 input / 1 output tokens.
/// Mirrors `FakeFactory` in `tests/it/linear_runner.rs`; a `reviewer-*` agent
/// answers with a high finding instead, like `GatePanelFactory` in
/// `tests/it/runner_events.rs`.
struct UsageFactory;

#[async_trait]
impl StepFactory for UsageFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        let text = if agent_name.starts_with("reviewer") {
            r#"{"findings":[{"severity":"high","title":"oops","body":"details"}]}"#.to_string()
        } else {
            format!("step {step_id} agent {agent_name} echo: {rendered_prompt}")
        };
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text,
            stop: StopReason::EndTurn,
            input_tokens: 10,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            codename: None,
            seed_source: None,
            collectors: Vec::new(),
            agent_name: format!("ag-{agent_name}"),
            agent_system_prompt: "echo".into(),
            agent_tools: None,
            provider: Box::new(provider),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: ToolContext::default(),
            user_message: rendered_prompt,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: false,
            suppress_stream_stdout: false,
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
            step_id: step_id.to_string(),
            on_tool_call,
            on_stream_event: None,
            on_usage: None,
            concerns: None,
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            scope_name: None,
            surface_tag: None,
            pause: None,
        }
    }
}

/// Panics if a unit is ever run locally. Mirrors `PanicFactory` in
/// `tests/it/distributed_fanout_e2e.rs`.
struct PanicFactory;

#[async_trait]
impl StepFactory for PanicFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        _agent_name: &str,
        _rendered_prompt: String,
        _run_id: String,
        _workspace_id: String,
        _workspace_path: std::path::PathBuf,
        _transcript_path: std::path::PathBuf,
        _on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        panic!("PanicFactory: build_opts_for_step must not be called for fully-distributed units");
    }
}

/// Fake fleet dispatcher whose units "run remotely" and whose transcripts
/// land at a coordinator-side mirror path known before dispatch — the shape
/// the SSH lazy mirror gives the real runner. Each mirrored transcript
/// carries two `Usage` events (7/3 and 5/2 tokens).
struct MirrorDispatcher {
    mirror_dir: PathBuf,
}

#[async_trait]
impl UnitDispatcher for MirrorDispatcher {
    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        host: &str,
    ) -> Result<UnitOutcome, UnitFailure> {
        let path = self.unit_transcript_path(host, &unit.run_id).unwrap();
        write_mirrored_transcript(&path, &unit.run_id);
        Ok(UnitOutcome {
            output: format!("out-{}-on-{host}", unit.index),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::NotLaunched,
        })
    }

    fn unit_transcript_path(&self, host: &str, unit_run_id: &str) -> Option<PathBuf> {
        Some(
            self.mirror_dir
                .join(host)
                .join(format!("{unit_run_id}.jsonl")),
        )
    }
}

fn write_mirrored_transcript(path: &Path, run_id: &str) {
    use rupu_transcript::{Event, JsonlWriter, RunMode, RunStatus};
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut w = JsonlWriter::create(path).unwrap();
    w.write(&Event::RunStart {
        codename: None,
        run_id: run_id.to_string(),
        workspace_id: "ws_remote".into(),
        agent: "a".into(),
        provider: "mock".into(),
        model: "mock-1".into(),
        started_at: chrono::Utc::now(),
        mode: RunMode::Bypass,
        schema: None,
        system_prompt: None,
    })
    .unwrap();
    for (input, output) in [(7u32, 3u32), (5, 2)] {
        w.write(&Event::Usage {
            provider: "mock".into(),
            model: "mock-1".into(),
            served_model: None,
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            cache_write_tokens: 0,
            purpose: None,
        })
        .unwrap();
    }
    w.write(&Event::RunComplete {
        run_id: run_id.to_string(),
        status: RunStatus::Ok,
        total_tokens: 17,
        duration_ms: 1,
        error: None,
    })
    .unwrap();
    w.flush().unwrap();
}

fn opts_for(
    tmp: &Path,
    wf_yaml: &str,
    store: &Arc<RunStore>,
    sink: Option<Arc<JsonlSink>>,
    factory: Arc<dyn StepFactory>,
    dispatcher: Option<Arc<dyn UnitDispatcher>>,
) -> OrchestratorRunOpts {
    OrchestratorRunOpts {
        naming: None,
        run_step: Default::default(),
        workflow: Workflow::parse(wf_yaml).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_ledger".into(),
        workspace_path: tmp.to_path_buf(),
        transcript_dir: tmp.join("transcripts"),
        factory,
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(store)),
        workflow_yaml: Some(wf_yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: sink.map(|s| s as Arc<dyn rupu_orchestrator::executor::EventSink>),
        unit_dispatcher: dispatcher,
        action_dispatcher: None,
        pause: None,
    }
}

/// `unit_completed` events for `step`, as raw JSON (flat `type`-tagged lines).
fn unit_completed_for_step(events_path: &Path, step: &str) -> Vec<serde_json::Value> {
    let body = std::fs::read_to_string(events_path).unwrap_or_default();
    body.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("unit_completed"))
        .filter(|v| v.get("step_id").and_then(|s| s.as_str()) == Some(step))
        .collect()
}

#[tokio::test]
async fn every_local_agent_launch_writes_ledger_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = opts_for(tmp.path(), WF, &store, None, Arc::new(UsageFactory), None);

    let res = run_workflow(opts).await.expect("run completes");
    let run_id = res.run_id.clone();
    assert!(!run_id.is_empty());

    let ledger = store.usage_ledger_path(&run_id);
    let rows: Vec<LedgerRow> = std::fs::read_to_string(&ledger)
        .unwrap_or_else(|e| panic!("ledger at {ledger:?} must exist: {e}"))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    // 1 (plan) + 3 (fan units) + 2 (parallel subs) agent runs × 1 LLM call each
    assert_eq!(rows.len(), 6, "{rows:#?}");
    let by_step = |s: &str| {
        rows.iter()
            .filter(|r| r.step_id.as_deref() == Some(s))
            .count()
    };
    assert_eq!(by_step("plan"), 1);
    assert_eq!(by_step("fan"), 3);
    assert_eq!(by_step("par"), 2);
    let mut idx: Vec<usize> = rows
        .iter()
        .filter(|r| r.step_id.as_deref() == Some("fan"))
        .filter_map(|r| r.unit_index)
        .collect();
    idx.sort();
    assert_eq!(idx, vec![0, 1, 2]);
    let mut par_keys: Vec<&str> = rows
        .iter()
        .filter(|r| r.step_id.as_deref() == Some("par"))
        .filter_map(|r| r.unit_key.as_deref())
        .collect();
    par_keys.sort();
    assert_eq!(par_keys, vec!["p1", "p2"]);
    for r in &rows {
        assert_eq!((r.input_tokens, r.output_tokens), (10, 1), "{r:?}");
        assert_eq!(r.agent, "a", "{r:?}");
    }
    // Every row's transcript is a real transcript containing a Usage event.
    for r in &rows {
        assert!(r.transcript.exists(), "{:?}", r.transcript);
        let totals = rupu_transcript::aggregate(&[&r.transcript], Default::default());
        assert_eq!(
            totals.iter().map(|t| t.input_tokens).sum::<u64>(),
            10,
            "{:?}",
            r.transcript
        );
    }
}

#[tokio::test]
async fn unit_completed_carries_real_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let events_path = tmp.path().join("events.jsonl");
    let sink = Arc::new(JsonlSink::create(&events_path).expect("create jsonl sink"));
    let opts = opts_for(
        tmp.path(),
        WF,
        &store,
        Some(sink),
        Arc::new(UsageFactory),
        None,
    );

    run_workflow(opts).await.expect("run completes");

    let completed = unit_completed_for_step(&events_path, "fan");
    assert_eq!(completed.len(), 3, "{completed:#?}");
    for ev in &completed {
        assert_eq!(ev["tokens_in"], 10, "{ev}");
        assert_eq!(ev["tokens_out"], 1, "{ev}");
    }
}

#[tokio::test]
async fn remote_unit_completed_folds_its_mirrored_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let events_path = tmp.path().join("events.jsonl");
    let sink = Arc::new(JsonlSink::create(&events_path).expect("create jsonl sink"));
    let dispatcher: Arc<dyn UnitDispatcher> = Arc::new(MirrorDispatcher {
        mirror_dir: tmp.path().join("mirror"),
    });
    let opts = opts_for(
        tmp.path(),
        WF_REMOTE,
        &store,
        Some(sink),
        Arc::new(PanicFactory),
        Some(dispatcher),
    );

    run_workflow(opts).await.expect("run completes");

    let completed = unit_completed_for_step(&events_path, "fan");
    assert_eq!(completed.len(), 2, "{completed:#?}");
    for ev in &completed {
        assert_eq!(ev["host"], "h1", "{ev}");
        // 7 + 5 in, 3 + 2 out — the mirrored transcript's Usage events.
        assert_eq!(ev["tokens_in"], 12, "{ev}");
        assert_eq!(ev["tokens_out"], 5, "{ev}");
    }
}

#[tokio::test]
async fn panel_and_fixer_units_are_ledgered_with_real_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let events_path = tmp.path().join("events.jsonl");
    let sink = Arc::new(JsonlSink::create(&events_path).expect("create jsonl sink"));
    let opts = opts_for(
        tmp.path(),
        WF_PANEL,
        &store,
        Some(sink),
        Arc::new(UsageFactory),
        None,
    );

    let res = run_workflow(opts).await.expect("run completes");

    let completed = unit_completed_for_step(&events_path, "scan");
    let keys: Vec<&str> = completed
        .iter()
        .filter_map(|ev| ev["unit_key"].as_str())
        .collect();
    assert!(keys.contains(&"iter1:reviewer-a"), "{keys:?}");
    assert!(
        keys.iter().any(|k| k.contains(":fix:fixer-bot")),
        "{keys:?}"
    );
    for ev in &completed {
        assert_eq!(ev["tokens_in"], 10, "{ev}");
        assert_eq!(ev["tokens_out"], 1, "{ev}");
    }

    // One ledger row per panelist/fixer agent run, tagged with the same
    // unit index + key its `UnitCompleted` carries.
    let rows: Vec<LedgerRow> = std::fs::read_to_string(store.usage_ledger_path(&res.run_id))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut ledgered: Vec<(usize, String)> = rows
        .iter()
        .filter(|r| r.step_id.as_deref() == Some("scan"))
        .map(|r| (r.unit_index.unwrap(), r.unit_key.clone().unwrap()))
        .collect();
    ledgered.sort();
    let mut emitted: Vec<(usize, String)> = completed
        .iter()
        .map(|ev| {
            (
                ev["index"].as_u64().unwrap() as usize,
                ev["unit_key"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    emitted.sort();
    assert_eq!(ledgered, emitted);
}
