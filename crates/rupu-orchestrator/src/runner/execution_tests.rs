//! Deterministic fixtures through the production runner and observation reader.
#![cfg(unix)]

use super::*;
use crate::execution_observation::{read_for_owner, ExecutionObservation, ExecutionValidity};
use rupu_agent::runner::{MockProvider, ScriptedTurn};
use rupu_providers::types::StopReason;
use std::fs::File;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

const RUN: &str = "run_execution_fixture";
const FORK: &str = r#"
name: executing-fork
steps:
  - id: fork
    split: [a, b]
  - id: a
    agent: worker
    prompt: a
  - id: b
    agent: worker
    prompt: b
"#;

struct BlockingFactory {
    entered: mpsc::UnboundedSender<String>,
    released: mpsc::UnboundedSender<String>,
    gates: BTreeMap<String, Arc<Notify>>,
}

#[async_trait]
impl StepFactory for BlockingFactory {
    async fn launch_for_step(&self, request: StepRequest) -> Result<StepLaunch, AssembleError> {
        let _ = self.entered.send(request.step_id.clone());
        if let Some(gate) = self.gates.get(&request.step_id) {
            gate.notified().await;
        }
        let _ = self.released.send(request.step_id.clone());
        if request.agent_name == "panic" {
            panic!("fixture child panic");
        }
        crate::testing::MockRun {
            assembler: Some(crate::testing::assembler_at(&request.workspace.path)),
            agent_name: request.agent_name.clone(),
            agent_system_prompt: "fixture".into(),
            provider: Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: if request.agent_name == "reviewer" {
                    r#"{"findings":[]}"#.into()
                } else {
                    "false".into()
                },
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }])),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            max_turns: 2,
            no_stream: true,
            suppress_stream_stdout: true,
            ..Default::default()
        }
        .launch(request)
    }
}

struct Harness {
    tmp: tempfile::TempDir,
    store: Arc<crate::runs::RunStore>,
    yaml: String,
    factory: Arc<BlockingFactory>,
    entered: mpsc::UnboundedReceiver<String>,
    released: mpsc::UnboundedReceiver<String>,
}

impl Harness {
    fn new(yaml: &str, blocked: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::runs::RunStore::new(tmp.path().join("runs")));
        let (entered_tx, entered) = mpsc::unbounded_channel();
        let (released_tx, released) = mpsc::unbounded_channel();
        let factory = Arc::new(BlockingFactory {
            entered: entered_tx,
            released: released_tx,
            gates: blocked
                .iter()
                .map(|id| (id.to_string(), Arc::new(Notify::new())))
                .collect(),
        });
        Self {
            tmp,
            store,
            yaml: yaml.to_string(),
            factory,
            entered,
            released,
        }
    }

    fn opts(&self) -> OrchestratorRunOpts {
        OrchestratorRunOpts {
            run_step: Default::default(),
            workflow: Workflow::parse(&self.yaml).unwrap(),
            inputs: BTreeMap::new(),
            workspace_id: "ws_execution_fixture".into(),
            workspace_path: self.tmp.path().to_path_buf(),
            transcript_dir: self.tmp.path().join("transcripts"),
            factory: self.factory.clone(),
            event: None,
            issue: None,
            issue_ref: None,
            run_store: Some(self.store.clone()),
            workflow_yaml: Some(self.yaml.clone()),
            resume_from: None,
            run_id_override: Some(RUN.into()),
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_services: None,
            pause: None,
            naming: None,
        }
    }

    async fn entered(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(5), self.entered.recv())
            .await
            .unwrap()
            .unwrap()
    }

    fn release(&self, step: &str) {
        self.factory.gates[step].notify_one();
    }

    fn snapshot(&self) -> ExecutionObservation {
        snapshot(&self.store)
    }

    async fn wait_members(&self, wanted: &[&str]) -> ExecutionObservation {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let value = self.snapshot();
                if ids(&value) == wanted {
                    return value;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }

    fn prior(&self) -> Vec<StepResult> {
        self.store
            .read_step_results(RUN)
            .unwrap()
            .iter()
            .map(StepResult::from)
            .collect()
    }
}

fn snapshot(store: &crate::runs::RunStore) -> ExecutionObservation {
    let record = store.load(RUN).unwrap();
    let dir = File::open(store.root.join(RUN)).unwrap();
    read_for_owner(&dir, &record, false).envelope
}

fn ids(value: &ExecutionObservation) -> Vec<&str> {
    let mut ids: Vec<_> = value
        .members
        .iter()
        .map(|member| member.step_id.as_str())
        .collect();
    ids.sort_unstable();
    ids
}

fn current(value: &ExecutionObservation) {
    assert_eq!(value.validity, ExecutionValidity::Current, "{value:?}");
    assert!(value.complete, "{value:?}");
}

async fn complete(
    task: tokio::task::JoinHandle<Result<OrchestratorRunResult, RunWorkflowError>>,
) -> OrchestratorRunResult {
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn one_permit_excludes_the_queued_step_and_exact_leave_admits_the_next() {
    let mut h = Harness::new(FORK, &["a", "b"]);
    let mut opts = h.opts();
    opts.workflow.max_concurrency = Some(1);
    let task = tokio::spawn(run_workflow(opts));
    assert_eq!(h.entered().await, "a");
    let first = h.snapshot();
    current(&first);
    assert_eq!(ids(&first), ["a"]);
    assert!(h.entered.try_recv().is_err(), "b must still be queued");
    h.release("a");
    assert_eq!(h.entered().await, "b");
    let second = h.snapshot();
    current(&second);
    assert_eq!(ids(&second), ["b"]);
    assert_eq!(first.execution_generation, second.execution_generation);
    assert_ne!(
        first.members[0].activation_id,
        second.members[0].activation_id
    );
    assert!(
        second.revision.as_ref().unwrap().parse::<u64>().unwrap()
            > first.revision.as_ref().unwrap().parse::<u64>().unwrap()
    );
    h.release("b");
    complete(task).await;
    let closed = h.snapshot();
    assert_eq!(closed.validity, ExecutionValidity::Stale);
    assert!(!closed.complete);
    assert!(closed.members.is_empty());
}

#[tokio::test]
async fn two_permits_keep_the_other_activation_after_one_leaves() {
    let mut h = Harness::new(FORK, &["a", "b"]);
    let mut opts = h.opts();
    opts.workflow.max_concurrency = Some(2);
    let task = tokio::spawn(run_workflow(opts));
    let mut entered = [h.entered().await, h.entered().await];
    entered.sort();
    assert_eq!(entered, ["a", "b"]);
    let both = h.snapshot();
    current(&both);
    assert_eq!(ids(&both), ["a", "b"]);
    let b = both
        .members
        .iter()
        .find(|m| m.step_id == "b")
        .unwrap()
        .activation_id
        .clone();
    h.release("a");
    let remaining = h.wait_members(&["b"]).await;
    current(&remaining);
    assert_eq!(remaining.members[0].activation_id, b);
    h.release("b");
    complete(task).await;
}

#[tokio::test]
async fn repeated_loop_member_has_actual_coordinates_and_distinct_activation() {
    let mut h = Harness::new(
        r#"
name: executing-loop
steps:
  - id: work
    agent: worker
    prompt: work
    next: [check]
  - id: check
    agent: worker
    prompt: check
loops:
  refine:
    nodes: [work, check]
    until: "false"
    max_iterations: 2
    on_max: proceed
"#,
        &["work"],
    );
    let task = tokio::spawn(run_workflow(h.opts()));
    assert_eq!(h.entered().await, "work");
    let first = h.snapshot();
    current(&first);
    assert_eq!(ids(&first), ["loop:refine", "work"]);
    let work = first.members.iter().find(|m| m.step_id == "work").unwrap();
    assert_eq!(
        work.iterations,
        vec![crate::execution_observation::ExecutionIteration {
            loop_id: "refine".into(),
            iteration: 0
        }]
    );
    let activation = work.activation_id.clone();
    assert!(first
        .members
        .iter()
        .find(|m| m.step_id == "loop:refine")
        .unwrap()
        .iterations
        .is_empty());
    h.release("work");
    assert_eq!(h.entered().await, "check");
    assert_eq!(h.entered().await, "work");
    let second = h.snapshot();
    current(&second);
    let work = second.members.iter().find(|m| m.step_id == "work").unwrap();
    assert_eq!(work.iterations[0].iteration, 1);
    assert_ne!(work.activation_id, activation);
    assert_eq!(first.execution_generation, second.execution_generation);
    h.release("work");
    complete(task).await;
}

#[tokio::test]
async fn parked_approval_is_absent_while_independent_work_executes() {
    let mut h = Harness::new(
        r#"
name: executing-gate
steps:
  - id: fork
    split: [gate, side]
  - id: gate
    approval: {prompt: approve}
  - id: side
    agent: worker
    prompt: side
"#,
        &["side"],
    );
    let task = tokio::spawn(run_workflow(h.opts()));
    assert_eq!(h.entered().await, "side");
    let running = h.snapshot();
    current(&running);
    assert_eq!(ids(&running), ["side"]);
    h.release("side");
    assert!(complete(task).await.awaiting.is_some());
    assert!(h.snapshot().members.is_empty());
}

#[tokio::test]
async fn manual_pause_request_keeps_entered_body_until_acknowledged() {
    let mut h = Harness::new(
        r#"
name: executing-pause
steps:
  - id: work
    agent: worker
    prompt: work
  - id: next
    agent: worker
    prompt: next
"#,
        &["work"],
    );
    let pause = CancellationToken::new();
    let mut opts = h.opts();
    opts.pause = Some(pause.clone());
    let task = tokio::spawn(run_workflow(opts));
    assert_eq!(h.entered().await, "work");
    pause.cancel();
    let requested = h.snapshot();
    current(&requested);
    assert_eq!(ids(&requested), ["work"]);
    h.release("work");
    assert_eq!(
        complete(task).await.awaiting.unwrap().reason,
        PauseReason::Manual
    );
    assert!(
        h.entered.try_recv().is_err(),
        "next must not enter after pause"
    );
    let paused = h.snapshot();
    assert_ne!(paused.validity, ExecutionValidity::Current);
    assert!(!paused.complete);
    assert!(paused.members.is_empty());
}

#[tokio::test]
async fn abort_of_fanout_invalidates_even_while_detached_child_finishes() {
    let mut h = Harness::new(
        r#"
name: executing-abort
steps:
  - id: work
    agent: worker
    for_each: '["item"]'
    prompt: work
"#,
        &["work"],
    );
    let task = tokio::spawn(run_workflow(h.opts()));
    assert_eq!(h.entered().await, "work");
    current(&h.snapshot());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let aborted = h.snapshot();
    assert_ne!(aborted.validity, ExecutionValidity::Current);
    assert!(!aborted.complete);
    // The existing Vec<JoinHandle> behavior is deliberately unchanged: dropping
    // the parent does not stop this factory future. Its late activity must not
    // restore current authority.
    h.release("work");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), h.released.recv())
            .await
            .unwrap()
            .unwrap(),
        "work"
    );
    assert_ne!(h.snapshot().validity, ExecutionValidity::Current);
}

#[tokio::test]
async fn child_panic_invalidates_instead_of_publishing_complete_empty() {
    let h = Harness::new(
        r#"
name: executing-panic
steps:
  - id: work
    agent: panic
    for_each: '["item"]'
    prompt: work
"#,
        &[],
    );
    let outcome = run_workflow(h.opts()).await;
    assert!(matches!(outcome, Err(RunWorkflowError::FanoutJoin { .. })));
    let failed = h.snapshot();
    assert_ne!(failed.validity, ExecutionValidity::Current);
    assert!(!failed.complete);
}

#[tokio::test]
async fn aggregate_families_observe_the_parent_body_without_invented_child_members() {
    for body in [
        "    agent: worker\n    for_each: '[\"item\"]'\n    prompt: work\n",
        "    parallel:\n      - id: declared-child\n        agent: worker\n        prompt: work\n",
        "    panel:\n      subject: work\n      panelists: [reviewer]\n",
    ] {
        let yaml = format!("name: executing-aggregate\nsteps:\n  - id: work\n{body}");
        let mut h = Harness::new(&yaml, &["work"]);
        let task = tokio::spawn(run_workflow(h.opts()));
        assert_eq!(h.entered().await, "work");
        let running = h.snapshot();
        current(&running);
        assert_eq!(ids(&running), ["work"]);
        assert!(running.members[0].iterations.is_empty());
        h.release("work");
        complete(task).await;
        assert!(h.snapshot().members.is_empty());
    }
}

#[tokio::test]
async fn standalone_rejection_cleanup_establishes_a_new_generation() {
    let mut h = Harness::new(
        r#"
name: executing-cleanup
steps:
  - id: gate
    approval:
      prompt: approve
      on_reject:
        - id: cleanup
          agent: worker
          prompt: cleanup
"#,
        &["cleanup"],
    );
    assert!(run_workflow(h.opts()).await.unwrap().awaiting.is_some());
    let prior_generation = h.snapshot().execution_generation;
    h.store
        .reject(RUN, "fixture", "rejected", chrono::Utc::now())
        .unwrap();
    let mut opts = h.opts();
    opts.resume_from = Some(ResumeState::from_rejection(
        RUN.into(),
        h.prior(),
        "gate".into(),
        "rejected".into(),
    ));
    let task =
        tokio::spawn(
            async move { run_reject_cleanup(opts, "gate", "rejected", "human", None).await },
        );
    assert_eq!(h.entered().await, "cleanup");
    let cleanup = h.snapshot();
    current(&cleanup);
    assert_eq!(ids(&cleanup), ["cleanup"]);
    assert_ne!(cleanup.execution_generation, prior_generation);
    h.release("cleanup");
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        h.store.load(RUN).unwrap().status,
        crate::runs::RunStatus::Rejected
    );
    assert!(h.snapshot().members.is_empty());
}

#[tokio::test]
async fn conflicting_cleanup_owner_fails_before_any_second_body_enters() {
    let mut h = Harness::new(
        r#"
name: executing-owner-conflict
steps:
  - id: gate
    approval:
      prompt: approve
      on_reject:
        - id: cleanup
          agent: worker
          prompt: cleanup
"#,
        &["cleanup"],
    );
    run_workflow(h.opts()).await.unwrap();
    h.store
        .reject(RUN, "fixture", "rejected", chrono::Utc::now())
        .unwrap();
    let mut opts = h.opts();
    opts.resume_from = Some(ResumeState::from_rejection(
        RUN.into(),
        h.prior(),
        "gate".into(),
        "rejected".into(),
    ));
    let duplicate = opts.clone();
    let task =
        tokio::spawn(
            async move { run_reject_cleanup(opts, "gate", "rejected", "human", None).await },
        );
    assert_eq!(h.entered().await, "cleanup");
    let first = h.snapshot();
    let rejected = run_reject_cleanup(duplicate, "gate", "rejected", "human", None).await;
    assert!(matches!(rejected, Err(RunWorkflowError::Io(_))));
    assert!(h.entered.try_recv().is_err());
    let after = h.snapshot();
    current(&after);
    assert_eq!(first.execution_generation, after.execution_generation);
    assert_eq!(first.members, after.members);
    h.release("cleanup");
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn scheduler_spawned_rejection_cleanup_inherits_the_owner() {
    let mut h = Harness::new(
        r#"
name: executing-concurrent-cleanup
steps:
  - id: fork
    split: [gate, waiting]
  - id: gate
    approval:
      prompt: approve
      on_reject:
        - id: cleanup
          agent: worker
          prompt: cleanup
  - id: waiting
    approval: {prompt: wait}
"#,
        &["cleanup"],
    );
    assert!(run_workflow(h.opts()).await.unwrap().awaiting.is_some());
    h.store
        .reject_gate(RUN, "fixture", "rejected", chrono::Utc::now(), Some("gate"))
        .unwrap();
    let mut opts = h.opts();
    opts.resume_from = Some(ResumeState::from_decisions(RUN.into(), h.prior()));
    let task = tokio::spawn(run_workflow(opts));
    assert_eq!(h.entered().await, "cleanup");
    let cleanup = h.snapshot();
    current(&cleanup);
    assert_eq!(ids(&cleanup), ["cleanup"]);
    h.release("cleanup");
    assert!(complete(task).await.awaiting.is_some());
}

struct CapturingSink {
    store: Arc<crate::runs::RunStore>,
    captures: Mutex<Vec<(String, ExecutionObservation)>>,
}

impl crate::executor::EventSink for CapturingSink {
    fn emit(&self, _run: &str, event: &crate::executor::Event) {
        if let crate::executor::Event::StepStarted { step_id, .. } = event {
            self.captures
                .lock()
                .unwrap()
                .push((step_id.clone(), snapshot(&self.store)));
        }
    }
}

#[tokio::test]
async fn synchronous_control_bodies_have_real_activations() {
    let h = Harness::new(
        r#"
name: executing-controls
steps:
  - id: fork
    split: [left, right]
  - id: left
    approval: {auto_approve: "true"}
    next: [joined]
  - id: right
    approval: {auto_approve: "true"}
    next: [joined]
  - id: joined
    join: {wait: all}
"#,
        &[],
    );
    let sink = Arc::new(CapturingSink {
        store: h.store.clone(),
        captures: Mutex::new(Vec::new()),
    });
    let mut opts = h.opts();
    opts.event_sink = Some(sink.clone());
    run_workflow(opts).await.unwrap();
    let captures = sink.captures.lock().unwrap();
    for step in ["fork", "left", "right", "joined"] {
        let capture = &captures.iter().find(|(id, _)| id == step).unwrap().1;
        current(capture);
        assert!(
            capture.members.iter().any(|m| m.step_id == step),
            "{step}: {capture:?}"
        );
    }
}

struct NotifyConnector {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl rupu_scm::RepoConnector for NotifyConnector {
    fn platform(&self) -> rupu_scm::Platform {
        rupu_scm::Platform::Github
    }
    async fn list_repos(&self) -> Result<Vec<rupu_scm::Repo>, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn get_repo(&self, _: &rupu_scm::RepoRef) -> Result<rupu_scm::Repo, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn list_branches(
        &self,
        _: &rupu_scm::RepoRef,
    ) -> Result<Vec<rupu_scm::Branch>, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn create_branch(
        &self,
        _: &rupu_scm::RepoRef,
        _: &str,
        _: &str,
    ) -> Result<rupu_scm::Branch, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn read_file(
        &self,
        _: &rupu_scm::RepoRef,
        _: &str,
        _: Option<&str>,
    ) -> Result<rupu_scm::FileContent, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn list_prs(
        &self,
        _: &rupu_scm::RepoRef,
        _: rupu_scm::PrFilter,
    ) -> Result<Vec<rupu_scm::Pr>, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn get_pr(&self, _: &rupu_scm::PrRef) -> Result<rupu_scm::Pr, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn diff_pr(&self, _: &rupu_scm::PrRef) -> Result<rupu_scm::Diff, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn comment_pr(
        &self,
        _: &rupu_scm::PrRef,
        body: &str,
    ) -> Result<rupu_scm::Comment, rupu_scm::ScmError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(rupu_scm::Comment {
            id: "fixture".into(),
            author: "fixture".into(),
            body: body.into(),
            created_at: chrono::Utc::now(),
            author_association: None,
        })
    }
    async fn create_pr(
        &self,
        _: &rupu_scm::RepoRef,
        _: rupu_scm::CreatePr,
    ) -> Result<rupu_scm::Pr, rupu_scm::ScmError> {
        unimplemented!()
    }
    async fn clone_to(&self, _: &rupu_scm::RepoRef, _: &Path) -> Result<(), rupu_scm::ScmError> {
        unimplemented!()
    }
}

#[tokio::test]
async fn notify_is_covered_by_its_gate_without_colliding_with_a_declared_step() {
    let h = Harness::new(
        r#"
name: executing-notify
steps:
  - id: gate
    approval:
      prompt: approve
      notify:
        - action: scm.prs.comment
          with: {platform: github, owner: fixture, repo: fixture, number: 1, body: notify}
  - id: gate.notify
    agent: worker
    prompt: not admitted
"#,
        &[],
    );
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut registry = rupu_scm::Registry::empty();
    registry.insert_repo_connector(
        rupu_scm::Platform::Github,
        Arc::new(NotifyConnector {
            entered: entered.clone(),
            release: release.clone(),
        }),
    );
    let mut context = rupu_tools::ToolContext::default();
    context.services.scm = Some(Arc::new(registry));
    let mut opts = h.opts();
    opts.action_services = Some(ActionServices {
        context,
        mode: rupu_tools::PermissionMode::Bypass,
    });
    let task = tokio::spawn(run_workflow(opts));
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let notifying = h.snapshot();
    current(&notifying);
    assert_eq!(ids(&notifying), ["gate"]);
    release.notify_one();
    assert!(complete(task).await.awaiting.is_some());
    assert!(h.snapshot().members.is_empty());
}
