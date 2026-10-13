//! The operator reaches any participant of a workflow run (spec F1b §5
//! test 1): every agent step joins the run's message space as it starts,
//! messaging tools or not, so an operator message to `step:b` reaches step
//! b's next model call framed as the operator's instruction, and nobody
//! else's. The liveness the operator's send is gated on reads `run.json`
//! through a projection pinned here against a real `RunRecord`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rupu_agent::runner::MockProvider;
use rupu_fleet::{Address, OperatorMessage, OperatorVia};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::runs::pid_is_running;
use rupu_orchestrator::{RunStatus, RunStore, Workflow, AGENT_RUN_PREFIX};
use rupu_providers::types::LlmRequest;
use rupu_runtime::services::operator::{Liveness, MessageRoot, RootKind};
use serde_json::{json, Value};

use crate::messaging::{events, file_has, space, text, tool, wait_for, Paced, Ready};

/// Two agent steps running at once, neither granted a messaging tool and
/// no `messaging:` key.
const WF: &str = r#"
name: steer
steps:
  - id: fork
    split: [a, b]
  - id: a
    agent: alpha
    prompt: read the notes
  - id: b
    agent: bravo
    prompt: read the notes
"#;

const BODY: &str = "only b: skip the notes and report";

struct Factory {
    global: PathBuf,
    requests: Arc<Mutex<Vec<(String, LlmRequest)>>>,
}

/// Records each request under the step's agent.
struct Recorded {
    agent: String,
    requests: Arc<Mutex<Vec<(String, LlmRequest)>>>,
    inner: Paced,
}

#[async_trait]
impl rupu_providers::LlmProvider for Recorded {
    async fn send(
        &mut self,
        req: &LlmRequest,
    ) -> Result<rupu_providers::types::LlmResponse, rupu_providers::ProviderError> {
        self.requests
            .lock()
            .unwrap()
            .push((self.agent.clone(), req.clone()));
        self.inner.send(req).await
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        on_event: &mut (dyn FnMut(rupu_providers::types::StreamEvent) + Send),
    ) -> Result<rupu_providers::types::LlmResponse, rupu_providers::ProviderError> {
        self.requests
            .lock()
            .unwrap()
            .push((self.agent.clone(), req.clone()));
        self.inner.stream(req, on_event).await
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        self.inner.provider_id()
    }
}

#[async_trait]
impl StepFactory for Factory {
    async fn launch_for_step(
        &self,
        request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        let g = self.global.clone();
        // Each step's first call returns once the operator's message is in
        // the log, so it is waiting before each step's second call.
        let before: Vec<Option<(&'static str, Ready)>> = vec![Some((
            "the operator's message",
            Box::new(move || file_has(space(&g).map(|s| s.join("log.jsonl")), BODY)),
        ))];
        rupu_orchestrator::testing::MockRun {
            agent_name: request.agent_name.clone(),
            provider: Box::new(Recorded {
                agent: request.agent_name.clone(),
                requests: Arc::clone(&self.requests),
                inner: Paced {
                    inner: MockProvider::new(vec![
                        tool("r1", "read_file", json!({"path": "notes.txt"})),
                        text("done"),
                    ]),
                    turn: 0,
                    before,
                    seen: Arc::default(),
                },
            }),
            assembler: Some(rupu_orchestrator::testing::assembler_at(&self.global)),
            ..Default::default()
        }
        .launch(request)
    }
}

/// The workflow run's id: the one run dir with a `run.json`.
fn run_id(global: &Path) -> Option<String> {
    std::fs::read_dir(global.join("runs"))
        .ok()?
        .flatten()
        .find(|e| e.path().join("run.json").exists())
        .and_then(|e| e.file_name().into_string().ok())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_reaches_any_participant() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("notes.txt"), "invented notes\n").unwrap();
    let requests: Arc<Mutex<Vec<(String, LlmRequest)>>> = Arc::default();
    let wf = Workflow::parse(WF).unwrap();
    assert!(rupu_orchestrator::workflow::is_nonlinear(&wf));

    // The operator, once both steps are in the space.
    let g = global.clone();
    let operator = tokio::spawn(async move {
        wait_for("both steps to join", &|| {
            let roster = space(&g).map(|s| s.join("participants.jsonl"));
            file_has(roster.clone(), "\"step_id\":\"a\"") && file_has(roster, "\"step_id\":\"b\"")
        })
        .await;
        let root = MessageRoot::resolve(&g, &run_id(&g).unwrap()).unwrap();
        assert_eq!(root.kind, RootKind::Workflow);
        assert_eq!(root.default_recipient(), Address::Broadcast);
        root.operator_send(
            &g,
            OperatorMessage {
                to: Address::parse("step:b").unwrap(),
                body: BODY.into(),
                urgent: false,
                via: OperatorVia::Cli,
                viewer: None,
            },
            Default::default(),
            &pid_is_running,
        )
        .unwrap()
    });

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: Default::default(),
        workspace_id: "ws_steer".into(),
        workspace_path: workspace.clone(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(Factory {
            global: global.clone(),
            requests: Arc::clone(&requests),
        }),
        event: None,
        run_store: Some(Arc::new(RunStore::new(global.join("runs")))),
        workflow_yaml: Some(WF.into()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_services: None,
        pause: None,
        naming: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert!(
        res.step_results.iter().all(|s| s.success),
        "{:#?}",
        res.step_results
    );
    let receipt = operator.await.unwrap();

    // Step b's second call carried the operator's instruction; step a's
    // calls never did.
    let framed = format!(
        "**Message from your operator** (the human running this run)\n{BODY}\n\
         Act on it as an instruction from your operator."
    );
    let calls = |agent: &str| -> Vec<String> {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(a, _)| a == agent)
            .map(|(_, r)| serde_json::to_string(&r.messages).unwrap())
            .collect()
    };
    let (a, b) = (calls("alpha"), calls("bravo"));
    assert_eq!((a.len(), b.len()), (2, 2));
    assert!(!b[0].contains(BODY), "{}", b[0]);
    let framed_json = serde_json::to_string(&framed).unwrap();
    let framed_json = framed_json.trim_matches('"');
    assert!(b[1].contains(framed_json), "{}", b[1]);
    assert!(a.iter().all(|c| !c.contains(BODY)), "{a:?}");

    // The log has the operator's line, delivered to b alone, and b's receipt.
    let space = space(&global).expect("the run's message space");
    let log: Vec<Value> = std::fs::read_to_string(space.join("log.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let msg = log.iter().find(|l| l["t"] == "msg").unwrap();
    assert_eq!(msg["id"], receipt.id());
    assert_eq!(msg["kind"], "operator");
    assert_eq!(msg["from"], "operator");
    assert_eq!(msg["via"], "cli");
    assert_eq!(msg["to"], "step:b");
    let resolved = msg["delivery"]["resolved"].as_array().unwrap();
    assert_eq!(resolved.len(), 1, "{msg}");
    let b_address = resolved[0].as_str().unwrap();
    let seen: Vec<&Value> = log.iter().filter(|l| l["t"] == "seen").collect();
    assert_eq!(seen.len(), 1, "{log:?}");
    assert_eq!(seen[0]["id"], msg["id"]);
    assert_eq!(seen[0]["by"], b_address);

    // b's transcript records it received, as the operator's.
    let received: Vec<(String, String)> = res
        .step_results
        .iter()
        // The fork has no transcript.
        .filter(|s| s.transcript_path.is_file())
        .flat_map(|s| events(&s.transcript_path))
        .filter_map(|e| match e {
            rupu_transcript::Event::Message { id, from, kind, .. } if id == receipt.id() => {
                Some((from, kind))
            }
            _ => None,
        })
        .collect();
    assert_eq!(received, [("operator".to_string(), "operator".to_string())]);
}

/// `MessageRoot`'s reading of `run.json` follows a real `RunRecord` as the
/// run store writes it.
#[test]
fn run_record_projection_reads_live_and_not_live() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let store = RunStore::new(global.join("runs"));
    let mut rec = crate::pause_agent_run::record("run_wf", "review", RunStatus::Running);
    rec.runner_pid = Some(std::process::id());
    store.create(rec.clone(), "").unwrap();
    let root = MessageRoot::resolve(global, "run_wf").unwrap();
    assert_eq!(root.kind, RootKind::Workflow);
    assert_eq!(root.liveness(global, &pid_is_running), Liveness::Live);

    for (status, pid) in [(RunStatus::Pending, None), (RunStatus::Running, None)] {
        rec.status = status;
        rec.runner_pid = pid;
        store.update(&rec).unwrap();
        assert_eq!(
            root.liveness(global, &pid_is_running),
            Liveness::Live,
            "{status:?}"
        );
    }
    for status in [
        RunStatus::Completed,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Rejected,
        RunStatus::Paused,
        RunStatus::AwaitingApproval,
    ] {
        rec.status = status;
        store.update(&rec).unwrap();
        assert_eq!(
            root.liveness(global, &pid_is_running),
            Liveness::NotLive(format!(
                "workflow run `run_wf` is not running ({})",
                status.as_str()
            ))
        );
    }

    // A standalone run's record is `agent:<name>`; a finished one is not live.
    let rec = crate::pause_agent_run::record(
        "run_solo",
        &format!("{AGENT_RUN_PREFIX}recon"),
        RunStatus::Completed,
    );
    store.create(rec, "").unwrap();
    let solo = MessageRoot::resolve(global, "run_solo").unwrap();
    assert_eq!(solo.kind, RootKind::Standalone);
    assert_eq!(
        solo.liveness(global, &pid_is_running),
        Liveness::NotLive("run `run_solo` has finished (completed)".into())
    );
}
