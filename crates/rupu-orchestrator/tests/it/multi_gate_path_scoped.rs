//! Gate decisions are PATH-SCOPED (spec §7 of the Phase-2 scheduler design;
//! product rule 2026-10-01): approving or rejecting one gate of a run that
//! batch-parked several affects only that gate's own path. The approved path
//! runs now; a sibling gate stays parked and approvable; a rejected gate
//! prunes only what is reachable solely through it; one runner executes the
//! run at a time, and a decision recorded while it runs is handed to it.
//!
//! Every test drives a real disk-backed `RunStore` through the same calls the
//! CLI (`approve_gate`/`reject_gate` + a resume) and the CP
//! (`request_resume_approval`/`request_resume_rejection`) make.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::runner::{
    run_reject_cleanup, run_workflow, OrchestratorRunOpts, OrchestratorRunResult, ResumeState,
    StepFactory,
};
use rupu_orchestrator::{
    ApprovalError, GateVerdict, RunStatus, RunStore, StepKind, StepResult, Workflow,
};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Steps held in flight until their `Notify` fires.
type Holds = Arc<Mutex<Vec<(String, Arc<tokio::sync::Notify>)>>>;

/// Records every dispatched step id (shared across clones, so several
/// runners of one run count into the same list). A step named in `holds`
/// blocks until its `Notify` fires — how a test keeps one path in flight.
#[derive(Clone, Default)]
struct Factory {
    calls: Arc<Mutex<Vec<String>>>,
    holds: Holds,
}

impl Factory {
    fn hold(&self, step: &str) -> Arc<tokio::sync::Notify> {
        let n = Arc::new(tokio::sync::Notify::new());
        self.holds
            .lock()
            .unwrap()
            .push((step.to_string(), Arc::clone(&n)));
        n
    }

    fn count(&self, step: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|s| *s == step)
            .count()
    }

    async fn wait_dispatched(&self, step: &str) {
        for _ in 0..1000 {
            if self.count(step) > 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!(
            "`{step}` was never dispatched; calls: {:?}",
            self.calls.lock().unwrap()
        );
    }
}

#[async_trait]
impl StepFactory for Factory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        self.calls.lock().unwrap().push(step_id.to_string());
        let hold = self
            .holds
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| id == step_id)
            .map(|(_, n)| Arc::clone(n));
        if let Some(n) = hold {
            n.notified().await;
        }
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: format!("out-{step_id}"),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            seed_source: None,
            agent_name: agent_name.to_string(),
            agent_system_prompt: "test".into(),
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
            step_id: step_id.to_string(),
            on_tool_call,
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
}

/// Two independent gated paths, each with its own `on_reject` cleanup.
const TWO_GATES: &str = r#"
name: two-gates
steps:
  - id: fanout
    split: [gate_a, gate_b]
  - id: gate_a
    approval:
      prompt: "Approve A?"
      on_reject:
        - id: undo_a
          agent: worker
          prompt: "undo a: {{ steps.gate_a.decision }}"
    next: [a]
  - id: gate_b
    approval:
      prompt: "Approve B?"
      on_reject:
        - id: undo_b
          agent: worker
          prompt: "undo b: {{ steps.gate_b.decision }}"
    next: [b]
  - id: a
    agent: worker
    prompt: "do a"
  - id: b
    agent: worker
    prompt: "do b"
"#;

/// The two gated paths reconverge on a join and a report.
const TWO_GATES_JOIN: &str = r#"
name: two-gates-join
steps:
  - id: fanout
    split: [gate_a, gate_b]
  - id: gate_a
    approval:
      prompt: "Approve A?"
    next: [a]
  - id: gate_b
    approval:
      prompt: "Approve B?"
    next: [b]
  - id: a
    agent: worker
    prompt: "do a"
    next: [merge]
  - id: b
    agent: worker
    prompt: "do b"
    next: [merge]
  - id: merge
    join:
      wait: all
    next: [report]
  - id: report
    agent: worker
    prompt: "report on {{ steps.merge.results }}"
"#;

struct Harness {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: Arc<RunStore>,
    yaml: &'static str,
    factory: Factory,
}

impl Harness {
    fn new(yaml: &'static str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        Self {
            store: Arc::new(RunStore::new(root.join("runs"))),
            root,
            _tmp: tmp,
            yaml,
            factory: Factory::default(),
        }
    }

    fn opts(&self, resume: Option<ResumeState>) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            run_step: Default::default(),
            workflow: Workflow::parse(self.yaml).unwrap(),
            inputs: BTreeMap::new(),
            workspace_id: "ws".into(),
            workspace_path: self.root.clone(),
            transcript_dir: self.root.join("transcripts"),
            factory: Arc::new(self.factory.clone()),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(Arc::clone(&self.store)),
            workflow_yaml: Some(self.yaml.to_string()),
            resume_from: resume,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
            naming: None,
        }
    }

    fn prior(&self, run_id: &str) -> Vec<StepResult> {
        self.store
            .read_step_results(run_id)
            .unwrap()
            .iter()
            .map(StepResult::from)
            .collect()
    }

    /// A fresh run: `fanout` parks both gates in one wave.
    async fn park_both(&self) -> String {
        let res = run_workflow(self.opts(None)).await.unwrap();
        let ids: Vec<String> = res
            .awaiting
            .expect("both gates park")
            .gates
            .iter()
            .map(|g| g.step_id.clone())
            .collect();
        assert_eq!(ids, ["gate_a", "gate_b"]);
        res.run_id
    }

    fn approve(&self, run_id: &str, gate: &str) {
        self.store
            .approve_gate(run_id, "op", chrono::Utc::now(), Some(gate))
            .unwrap_or_else(|e| panic!("approve {gate}: {e}"));
    }

    fn reject(&self, run_id: &str, gate: &str) {
        self.store
            .reject_gate(run_id, "op", "not this one", chrono::Utc::now(), Some(gate))
            .unwrap_or_else(|e| panic!("reject {gate}: {e}"));
    }

    /// What `rupu workflow approve --gate <gate>` runs after recording the
    /// approval.
    async fn resume_approved(&self, run_id: &str, gate: &str) -> OrchestratorRunResult {
        run_workflow(self.opts(Some(ResumeState::from_approval_with_actor(
            run_id.to_string(),
            self.prior(run_id),
            gate.to_string(),
            "op".into(),
            false,
        ))))
        .await
        .unwrap()
    }

    /// What `rupu workflow resume` (and the CP resume worker, through it)
    /// runs for a run whose gate decisions are recorded but not applied.
    async fn resume_decided(&self, run_id: &str) -> OrchestratorRunResult {
        run_workflow(self.opts(Some(ResumeState::from_decisions(
            run_id.to_string(),
            self.prior(run_id),
        ))))
        .await
        .unwrap()
    }

    fn gate_decisions_on_disk(&self, run_id: &str, gate: &str) -> Vec<String> {
        self.store
            .read_step_results(run_id)
            .unwrap()
            .iter()
            .filter(|r| r.step_id == gate)
            .map(|r| {
                let v: serde_json::Value = serde_json::from_str(&r.output).unwrap();
                v["decision"].as_str().unwrap_or_default().to_string()
            })
            .collect()
    }
}

#[tokio::test]
async fn approving_one_gate_runs_only_its_path_and_the_sibling_stays_parked_and_approvable() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;

    h.approve(&run_id, "gate_a");
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(
        rec.status,
        RunStatus::AwaitingApproval,
        "gate_b is still parked"
    );
    assert_eq!(rec.awaiting.len(), 1);
    assert_eq!(rec.awaiting[0].step_id, "gate_b");

    let res = h.resume_approved(&run_id, "gate_a").await;
    assert_eq!(res.handed_off_to, None, "nothing else was running the run");
    let parked: Vec<&str> = res
        .awaiting
        .as_ref()
        .expect("gate_b keeps the run paused")
        .gates
        .iter()
        .map(|g| g.step_id.as_str())
        .collect();
    assert_eq!(parked, ["gate_b"]);
    assert_eq!(h.factory.count("a"), 1, "gate_a's path ran");
    assert_eq!(h.factory.count("b"), 0, "gate_b's path waits for gate_b");

    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(rec.status, RunStatus::AwaitingApproval);
    assert_eq!(rec.awaiting.len(), 1);
    assert_eq!(rec.awaiting[0].step_id, "gate_b");
    assert!(
        rec.gate_decisions.is_empty(),
        "the runner consumed the approval it applied: {:?}",
        rec.gate_decisions
    );
    assert_eq!(rec.runner_pid, None, "a parked run has no runner");

    // gate_b is still approvable, and approving it runs only its own path.
    h.approve(&run_id, "gate_b");
    let res = h.resume_approved(&run_id, "gate_b").await;
    assert!(res.awaiting.is_none());
    assert_eq!(h.factory.count("a"), 1, "a never re-runs");
    assert_eq!(h.factory.count("b"), 1);
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Completed);
    assert!(rec.awaiting.is_empty() && rec.gate_decisions.is_empty());
    assert_eq!(h.gate_decisions_on_disk(&run_id, "gate_a"), ["approved"]);
    assert_eq!(h.gate_decisions_on_disk(&run_id, "gate_b"), ["approved"]);
}

#[tokio::test]
async fn approving_a_sibling_while_another_path_executes_hands_it_to_the_live_runner() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;
    let release_a = h.factory.hold("a");

    h.approve(&run_id, "gate_a");
    let first = {
        let opts = h.opts(Some(ResumeState::from_approval(
            run_id.clone(),
            h.prior(&run_id),
            "gate_a".into(),
        )));
        tokio::spawn(run_workflow(opts))
    };
    h.factory.wait_dispatched("a").await;

    // `rupu workflow approve --gate gate_b` while gate_a's path is running.
    h.approve(&run_id, "gate_b");
    let second = h.resume_approved(&run_id, "gate_b").await;
    assert_eq!(
        second.handed_off_to,
        Some(std::process::id()),
        "a run already being executed must not get a second runner"
    );

    // The live runner picks gate_b up without waiting for gate_a's path.
    h.factory.wait_dispatched("b").await;
    assert_eq!(h.factory.count("a"), 1, "a is still in flight, not re-run");

    release_a.notify_one();
    let first = first.await.unwrap().unwrap();
    assert!(first.awaiting.is_none(), "both paths finished");
    assert_eq!(h.factory.count("a"), 1, "no double execution of a");
    assert_eq!(h.factory.count("b"), 1, "no double execution of b");
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Completed);
    assert!(rec.awaiting.is_empty() && rec.gate_decisions.is_empty());
    assert_eq!(h.gate_decisions_on_disk(&run_id, "gate_b"), ["approved"]);
}

#[tokio::test]
async fn rejecting_a_gate_prunes_only_its_path_and_runs_its_cleanup_once() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;

    h.reject(&run_id, "gate_b");
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(
        rec.status,
        RunStatus::AwaitingApproval,
        "gate_a is untouched by gate_b's rejection"
    );
    assert_eq!(rec.awaiting.len(), 1);
    assert_eq!(rec.awaiting[0].step_id, "gate_a");
    assert_eq!(rec.gate_decisions.len(), 1);
    assert_eq!(rec.gate_decisions[0].verdict, GateVerdict::Rejected);

    h.approve(&run_id, "gate_a");
    let res = h.resume_approved(&run_id, "gate_a").await;
    assert!(
        res.awaiting.is_none(),
        "gate_b was decided — nothing is parked"
    );
    assert_eq!(h.factory.count("a"), 1);
    assert_eq!(h.factory.count("b"), 0, "a rejected gate's path never runs");
    assert_eq!(h.factory.count("undo_b"), 1, "gate_b's on_reject ran once");
    assert_eq!(h.factory.count("undo_a"), 0);
    assert_eq!(h.gate_decisions_on_disk(&run_id, "gate_b"), ["rejected"]);

    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(
        rec.status,
        RunStatus::Completed,
        "gate_a's path completed — gate_b's rejection does not reject it"
    );
    assert!(rec.awaiting.is_empty() && rec.gate_decisions.is_empty());
}

#[tokio::test]
async fn rejecting_the_last_parked_gate_while_a_path_executes_does_not_end_the_run() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;
    let release_a = h.factory.hold("a");

    h.approve(&run_id, "gate_a");
    let first = {
        let opts = h.opts(Some(ResumeState::from_approval(
            run_id.clone(),
            h.prior(&run_id),
            "gate_a".into(),
        )));
        tokio::spawn(run_workflow(opts))
    };
    h.factory.wait_dispatched("a").await;

    h.reject(&run_id, "gate_b");
    let rec = h.store.load(&run_id).unwrap();
    assert!(
        !rec.status.is_terminal(),
        "gate_a's path is still executing; got {:?}",
        rec.status
    );
    let second = h.resume_decided(&run_id).await;
    assert_eq!(second.handed_off_to, Some(std::process::id()));

    // The live runner applies the rejection (its cleanup runs) while a is
    // still in flight.
    h.factory.wait_dispatched("undo_b").await;
    release_a.notify_one();
    first.await.unwrap().unwrap();

    assert_eq!(h.factory.count("a"), 1);
    assert_eq!(h.factory.count("b"), 0);
    assert_eq!(h.factory.count("undo_b"), 1);
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Completed);
    assert!(rec.awaiting.is_empty() && rec.gate_decisions.is_empty());
}

#[tokio::test]
async fn a_gate_rejected_on_disk_before_the_decision_queue_existed_stays_pruned() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;

    // An older binary's `rupu workflow reject --gate gate_b`: the gate's
    // rejected result + its cleanup land in `step_results.jsonl`, and the
    // record keeps no pending decision for it.
    h.reject(&run_id, "gate_b");
    let mut opts = h.opts(None);
    opts.resume_from = Some(ResumeState::from_rejection(
        run_id.clone(),
        h.prior(&run_id),
        "gate_b".into(),
        "not this one".into(),
    ));
    run_reject_cleanup(opts, "gate_b", "not this one", "human", Some("op"))
        .await
        .unwrap();
    let mut rec = h.store.load(&run_id).unwrap();
    rec.gate_decisions.clear();
    h.store.update(&rec).unwrap();
    assert_eq!(h.factory.count("undo_b"), 1);

    h.approve(&run_id, "gate_a");
    let res = h.resume_approved(&run_id, "gate_a").await;
    assert!(res.awaiting.is_none(), "gate_b must not be parked again");
    assert_eq!(h.factory.count("b"), 0, "gate_b's path stays pruned");
    assert_eq!(h.factory.count("undo_b"), 1, "the cleanup is not repeated");
    assert_eq!(h.gate_decisions_on_disk(&run_id, "gate_b"), ["rejected"]);
    assert_eq!(h.store.load(&run_id).unwrap().status, RunStatus::Completed);
}

#[tokio::test]
async fn rejecting_every_gate_ends_the_run_rejected() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;

    h.reject(&run_id, "gate_a");
    h.reject(&run_id, "gate_b");
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(
        rec.status,
        RunStatus::Running,
        "both decisions wait for a runner to apply them"
    );
    assert!(rec.awaiting.is_empty());

    let res = h.resume_decided(&run_id).await;
    assert!(res.awaiting.is_none());
    assert_eq!(h.factory.count("a") + h.factory.count("b"), 0);
    assert_eq!(h.factory.count("undo_a"), 1);
    assert_eq!(h.factory.count("undo_b"), 1);
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Rejected);
    assert!(rec.gate_decisions.is_empty());
}

async fn join_case(reject_first: bool) {
    let h = Harness::new(TWO_GATES_JOIN);
    let run_id = h.park_both().await;
    if reject_first {
        h.reject(&run_id, "gate_b");
        h.approve(&run_id, "gate_a");
        h.resume_approved(&run_id, "gate_a").await;
    } else {
        h.approve(&run_id, "gate_a");
        let res = h.resume_approved(&run_id, "gate_a").await;
        assert!(res.awaiting.is_some(), "the join waits on gate_b's path");
        assert_eq!(h.factory.count("report"), 0);
        h.reject(&run_id, "gate_b");
        h.resume_decided(&run_id).await;
    }
    assert_eq!(h.factory.count("a"), 1);
    assert_eq!(h.factory.count("b"), 0);
    assert_eq!(
        h.factory.count("report"),
        1,
        "the join continues on gate_a's path (reject_first: {reject_first})"
    );
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(
        rec.status,
        RunStatus::Completed,
        "reject_first: {reject_first}"
    );
    let merge = h
        .prior(&run_id)
        .into_iter()
        .find(|r| r.step_id == "merge")
        .unwrap();
    assert_eq!(merge.kind, StepKind::Join);
    assert_eq!(
        merge.items.len(),
        1,
        "only gate_a's path arrived at the join"
    );
}

#[tokio::test]
async fn a_join_after_both_gates_continues_on_the_approved_path_in_either_order() {
    join_case(true).await;
    join_case(false).await;
}

#[tokio::test]
async fn two_web_approvals_are_both_recorded_and_applied_by_one_runner() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;
    let now = chrono::Utc::now();
    h.store
        .request_resume_approval(&run_id, "web", None, now, Some("gate_a"))
        .unwrap();
    h.store
        .request_resume_approval(&run_id, "web", None, now, Some("gate_b"))
        .unwrap();
    let rec = h.store.load(&run_id).unwrap();
    let decided: Vec<(&str, GateVerdict)> = rec
        .gate_decisions
        .iter()
        .map(|d| (d.step_id.as_str(), d.verdict))
        .collect();
    assert_eq!(
        decided,
        [
            ("gate_a", GateVerdict::Approved),
            ("gate_b", GateVerdict::Approved)
        ]
    );
    assert!(
        rec.resume_requested_at.is_some(),
        "the resume worker is asked for a runner"
    );
    assert_eq!(rec.gate_decisions[0].approver.as_deref(), Some("web"));

    h.resume_decided(&run_id).await;
    assert_eq!(h.factory.count("a"), 1);
    assert_eq!(h.factory.count("b"), 1);
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Completed);
    assert!(rec.gate_decisions.is_empty());
    assert_eq!(
        rec.resume_requested_at, None,
        "the runner consumed the request"
    );
}

#[tokio::test]
async fn a_web_rejection_asks_for_a_runner_and_prunes_only_its_path() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;
    h.store
        .request_resume_rejection(&run_id, "web", "no", chrono::Utc::now(), Some("gate_b"))
        .unwrap();
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(rec.status, RunStatus::AwaitingApproval);
    assert!(rec.resume_requested_at.is_some());

    // The resume worker's runner applies the rejection now; gate_a is
    // still parked and still approvable.
    let res = h.resume_decided(&run_id).await;
    let parked: Vec<&str> = res
        .awaiting
        .as_ref()
        .unwrap()
        .gates
        .iter()
        .map(|g| g.step_id.as_str())
        .collect();
    assert_eq!(parked, ["gate_a"]);
    assert_eq!(h.factory.count("undo_b"), 1);
    assert_eq!(h.factory.count("b"), 0);

    h.approve(&run_id, "gate_a");
    h.resume_approved(&run_id, "gate_a").await;
    assert_eq!(h.factory.count("a"), 1);
    assert_eq!(h.factory.count("b"), 0);
    assert_eq!(h.factory.count("undo_b"), 1);
    assert_eq!(h.store.load(&run_id).unwrap().status, RunStatus::Completed);
}

#[tokio::test]
async fn a_decided_gate_cannot_be_decided_again() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;
    h.reject(&run_id, "gate_b");
    // The first decision stands — the other way, or the same way again (a
    // stale view, a retried request): refused, never applied twice.
    for verdict_tried in ["approve", "reject"] {
        let err = if verdict_tried == "approve" {
            h.store
                .approve_gate(&run_id, "op", chrono::Utc::now(), Some("gate_b"))
                .unwrap_err()
        } else {
            h.store
                .reject_gate(&run_id, "op", "again", chrono::Utc::now(), Some("gate_b"))
                .unwrap_err()
        };
        assert!(
            matches!(err, ApprovalError::GateAlreadyDecided { ref step_id, verdict: GateVerdict::Rejected, .. } if step_id == "gate_b"),
            "{verdict_tried}: {err:?}"
        );
    }
    assert_eq!(h.store.load(&run_id).unwrap().gate_decisions.len(), 1);
}

#[tokio::test]
async fn cancelling_a_run_with_two_parked_gates_cancels_it() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;
    h.store
        .cancel(&run_id, "op", "stop", chrono::Utc::now())
        .expect("a multi-gate run is cancellable");
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Cancelled);
    assert!(rec.awaiting.is_empty() && rec.gate_decisions.is_empty());
}

#[tokio::test]
async fn cancelling_while_an_approved_path_executes_cancels_the_run() {
    let h = Harness::new(TWO_GATES);
    let run_id = h.park_both().await;
    let release_a = h.factory.hold("a");
    h.approve(&run_id, "gate_a");
    let first = {
        let opts = h.opts(Some(ResumeState::from_approval(
            run_id.clone(),
            h.prior(&run_id),
            "gate_a".into(),
        )));
        tokio::spawn(run_workflow(opts))
    };
    h.factory.wait_dispatched("a").await;

    h.store
        .cancel(&run_id, "op", "stop", chrono::Utc::now())
        .unwrap();
    release_a.notify_one();
    let _ = first.await.unwrap();
    let rec = h.store.load(&run_id).unwrap();
    assert_eq!(
        rec.status,
        RunStatus::Cancelled,
        "the runner keeps the cancel"
    );
    assert!(rec.awaiting.is_empty());
}
