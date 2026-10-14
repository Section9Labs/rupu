//! A workflow's `messaging:` key (spec F1a §7, §9 test 3): its agent steps
//! share the run's message space, so two parallel branches can talk — what
//! one sends reaches the other before its next model call, both transcripts
//! record it, and the space's log has the message and its receipt.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rupu_agent::runner::{MockProvider, ScriptedTurn};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::{RunStore, Workflow};
use rupu_providers::types::{LlmRequest, LlmResponse, StopReason, StreamEvent};
use rupu_providers::LlmProvider;
use serde_json::{json, Value};

const WF: &str = r#"
name: talk
messaging: true
steps:
  - id: pair
    max_parallel: 2
    parallel:
      - id: a
        agent: sender
        prompt: tell the listener
      - id: b
        agent: listener
        prompt: wait to hear
"#;

/// Wait (without blocking the task, which the parallel branches share)
/// until `ready` holds: the other branch's progress. Bounded.
pub(crate) async fn wait_for(what: &str, ready: &(dyn Fn() -> bool + Send + Sync)) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The run's message space: the one `messages/` dir under the run store.
pub(crate) fn space(global: &Path) -> Option<PathBuf> {
    std::fs::read_dir(global.join("runs"))
        .ok()?
        .flatten()
        .map(|e| e.path().join("messages"))
        .find(|p| p.exists())
}

pub(crate) fn file_has(path: Option<PathBuf>, needle: &str) -> bool {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|t| t.contains(needle))
}

pub(crate) type Ready = Box<dyn Fn() -> bool + Send + Sync>;

/// A scripted provider whose turns may each wait for the other branch
/// first (`(what, ready)`), recording every request.
pub(crate) struct Paced {
    pub(crate) inner: MockProvider,
    pub(crate) turn: usize,
    pub(crate) before: Vec<Option<(&'static str, Ready)>>,
    pub(crate) seen: Arc<Mutex<Vec<LlmRequest>>>,
}

impl Paced {
    async fn before_turn(&mut self, req: &LlmRequest) {
        self.seen.lock().unwrap().push(req.clone());
        if let Some(Some((what, ready))) = self.before.get(self.turn) {
            wait_for(what, ready.as_ref()).await;
        }
        self.turn += 1;
    }
}

#[async_trait]
impl LlmProvider for Paced {
    async fn send(
        &mut self,
        req: &LlmRequest,
    ) -> Result<LlmResponse, rupu_providers::ProviderError> {
        self.before_turn(req).await;
        self.inner.send(req).await
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, rupu_providers::ProviderError> {
        self.before_turn(req).await;
        self.inner.stream(req, on_event).await
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        self.inner.provider_id()
    }
}

pub(crate) fn tool(id: &str, name: &str, input: Value) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: name.into(),
        tool_input: input,
        stop: StopReason::ToolUse,
    }
}

pub(crate) fn text(t: &str) -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: t.into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

struct Factory {
    global: PathBuf,
    listener_requests: Arc<Mutex<Vec<LlmRequest>>>,
}

#[async_trait]
impl StepFactory for Factory {
    async fn launch_for_step(
        &self,
        request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        let g = self.global.clone();
        let (script, before): (Vec<ScriptedTurn>, Vec<Option<(&'static str, Ready)>>) =
            if request.agent_name == "sender" {
                (
                    vec![
                        tool(
                            "a1",
                            "msg.send",
                            json!({"to": "role:listener", "body": "ping from A"}),
                        ),
                        text("sent"),
                    ],
                    // Send once the listener has joined the space.
                    vec![Some((
                        "the listener to join",
                        Box::new(move || {
                            file_has(
                                space(&g).map(|s| s.join("participants.jsonl")),
                                "\"listener\"",
                            )
                        }),
                    ))],
                )
            } else {
                (
                    vec![tool("b1", "board.read", json!({})), text("heard")],
                    // Its first turn ends once the sender has sent, so the
                    // message is waiting before its second model call.
                    vec![Some((
                        "the sender's message",
                        Box::new(move || {
                            file_has(space(&g).map(|s| s.join("log.jsonl")), "ping from A")
                        }),
                    ))],
                )
            };
        let seen = if request.agent_name == "listener" {
            Arc::clone(&self.listener_requests)
        } else {
            Arc::default()
        };
        rupu_orchestrator::testing::MockRun {
            agent_name: request.agent_name.clone(),
            provider: Box::new(Paced {
                inner: MockProvider::new(script),
                turn: 0,
                before,
                seen,
            }),
            assembler: Some(rupu_orchestrator::testing::assembler_at(&self.global)),
            ..Default::default()
        }
        .launch(request)
    }
}

const WF_OFF: &str = r#"
name: hush
messaging: false
steps:
  - id: a
    agent: talker
    prompt: say hi
"#;

/// Every step runs `talker`, whose `tools:` names `msg.send`, on a provider
/// that records its requests.
struct OffFactory {
    global: PathBuf,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

#[async_trait]
impl StepFactory for OffFactory {
    async fn launch_for_step(
        &self,
        request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        rupu_orchestrator::testing::MockRun {
            agent_name: request.agent_name.clone(),
            agent_tools: Some(vec!["read_file".into(), "msg.send".into()]),
            provider: Box::new(Paced {
                inner: MockProvider::new(vec![text("hi")]),
                turn: 0,
                before: Vec::new(),
                seen: Arc::clone(&self.requests),
            }),
            assembler: Some(rupu_orchestrator::testing::assembler_at(&self.global)),
            ..Default::default()
        }
        .launch(request)
    }
}

/// Spec F1d D2, D5: a workflow's `messaging: false` leaves its steps no
/// messaging tool, and one the agent names is reported through a
/// `tool_unavailable` notice, never dropped silently.
#[tokio::test]
async fn workflow_messaging_false_turns_its_steps_off() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let requests: Arc<Mutex<Vec<LlmRequest>>> = Arc::default();
    let wf = Workflow::parse(WF_OFF).unwrap();
    let store = Arc::new(RunStore::new(global.join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: Default::default(),
        workspace_id: "ws_hush".into(),
        workspace_path: workspace.clone(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(OffFactory {
            global: global.clone(),
            requests: Arc::clone(&requests),
        }),
        event: None,
        run_store: Some(store),
        workflow_yaml: Some(WF_OFF.into()),
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
    assert!(res.step_results.iter().all(|s| s.success), "{:#?}", res.step_results);

    let requests = requests.lock().unwrap().clone();
    let tools: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(tools.contains(&"read_file"), "{tools:?}");
    assert!(
        !tools
            .iter()
            .any(|t| t.starts_with("board.") || t.starts_with("msg.")),
        "{tools:?}"
    );
    let notices: Vec<String> = res
        .step_results
        .iter()
        .flat_map(|s| events(&s.transcript_path))
        .filter_map(|e| match e {
            rupu_transcript::Event::Notice { kind, message } if kind == "tool_unavailable" => {
                Some(message)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        ["tool \"msg.send\" is not available in this run: it needs message_bus, which this run does not provide"]
    );
    assert!(
        file_has(
            space(&global).map(|s| s.join("participants.jsonl")),
            "\"talker\""
        ),
        "the step joined the space all the same (F1b)"
    );
}

pub(crate) fn events(path: &Path) -> Vec<rupu_transcript::Event> {
    rupu_transcript::JsonlReader::iter(path)
        .unwrap()
        .flatten()
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_parallel_branches_talk() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let listener_requests: Arc<Mutex<Vec<LlmRequest>>> = Arc::default();
    let wf = Workflow::parse(WF).unwrap();
    let store = Arc::new(RunStore::new(global.join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: Default::default(),
        workspace_id: "ws_talk".into(),
        workspace_path: workspace.clone(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(Factory {
            global: global.clone(),
            listener_requests: Arc::clone(&listener_requests),
        }),
        event: None,
        run_store: Some(store),
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

    // The listener's second model call carried the message.
    let requests = listener_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let second = serde_json::to_string(&requests[1].messages).unwrap();
    assert!(second.contains("ping from A"), "{second}");

    // The log has the message, resolved to the listener, and its receipt.
    let space = space(&global).expect("the run's message space");
    let log: Vec<Value> = std::fs::read_to_string(space.join("log.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let msg = log.iter().find(|l| l["t"] == "msg").unwrap();
    assert_eq!(msg["kind"], "direct");
    assert_eq!(msg["to"], "role:listener");
    assert_eq!(msg["delivery"]["resolved"].as_array().unwrap().len(), 1);
    let listener = msg["delivery"]["resolved"][0].as_str().unwrap().to_string();
    let seen = log.iter().find(|l| l["t"] == "seen").expect("a receipt");
    assert_eq!(seen["id"], msg["id"]);
    assert_eq!(seen["by"], listener.as_str());

    // Both transcripts record it, from either side.
    let id = msg["id"].as_str().unwrap();
    let directions: Vec<String> = res
        .step_results
        .iter()
        .flat_map(|s| s.items.iter())
        .flat_map(|item| events(&item.transcript_path))
        .filter_map(|e| match e {
            rupu_transcript::Event::Message {
                id: mid, direction, ..
            } if mid == id => Some(format!("{direction:?}")),
            _ => None,
        })
        .collect();
    let mut sorted = directions.clone();
    sorted.sort();
    assert_eq!(sorted, ["Received", "Sent"], "{directions:?}");
}
