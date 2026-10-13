//! F1b §5 test 3: the operator's messages reach an agentiflow lead through
//! its message space before its next model call, mid-round included, and an
//! urgent one also ends the round early (the old `send --now`) without
//! being lost. A real flow (`run_agentiflow`) on a scripted lead.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rupu_agent::{MockProvider, ScriptedTurn, StopReason as ModelStop};
use rupu_agentiflow::{
    agentiflow_dir, pid_is_running, run_agentiflow, AgentiflowDef, LeadInputs, LeadProvider,
    RunAgentiflowOpts,
};
use rupu_fleet::{Address, BusCaps, LogLine, OperatorMessage, OperatorVia};
use rupu_providers::types::{LlmRequest, LlmResponse, StreamEvent};
use rupu_providers::{LlmProvider, ProviderError, ProviderId};
use rupu_runtime::assembly::{AssemblyContext, RunAssembler};
use rupu_runtime::services::operator::MessageRoot;

/// What a scripted call does before it answers.
enum Before {
    Nothing,
    /// The operator sends this (urgent or not) while the model generates.
    Send {
        body: &'static str,
        urgent: bool,
    },
    /// The operator sends this, urgent, and the reply never comes.
    SendAndHang {
        body: &'static str,
    },
}

/// One round's lead: each call runs its `Before`, records the request the
/// lead made, then answers from the script.
struct ScriptedLead {
    global: PathBuf,
    id: String,
    befores: std::vec::IntoIter<Before>,
    replies: MockProvider,
    requests: Arc<Mutex<Vec<String>>>,
}

impl ScriptedLead {
    async fn call(&mut self, req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(&req.messages).unwrap());
        match self.befores.next().unwrap_or(Before::Nothing) {
            Before::Nothing => {}
            Before::Send { body, urgent } => send(&self.global, &self.id, body, urgent),
            Before::SendAndHang { body } => {
                send(&self.global, &self.id, body, true);
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }
        self.replies.send(req).await
    }
}

#[async_trait::async_trait]
impl LlmProvider for ScriptedLead {
    async fn send(&mut self, req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.call(req).await
    }
    async fn stream(
        &mut self,
        req: &LlmRequest,
        _on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.call(req).await
    }
    fn default_model(&self) -> &str {
        "mock-1"
    }
    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

/// `rupu agentiflow send` for the running flow `id`.
fn send(global: &Path, id: &str, body: &str, urgent: bool) {
    MessageRoot::resolve(global, id)
        .unwrap()
        .operator_send(
            global,
            OperatorMessage {
                to: Address::Lead,
                body: body.into(),
                urgent,
                via: OperatorVia::Cli,
                viewer: None,
            },
            BusCaps::default(),
            &pid_is_running,
        )
        .unwrap();
}

fn text(t: &str) -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: t.into(),
        stop: ModelStop::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

/// A turn that reads the board: something to do, so the round makes a
/// second model call.
fn read_the_board() -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: "call_board".into(),
        tool_name: "board.read".into(),
        tool_input: serde_json::json!({}),
        stop: ModelStop::ToolUse,
    }
}

struct Flow {
    _tmp: tempfile::TempDir,
    global: PathBuf,
    workspace: PathBuf,
    id: String,
}

impl Flow {
    fn new(id: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        Self {
            _tmp: tmp,
            global,
            workspace,
            id: id.into(),
        }
    }

    fn dir(&self) -> PathBuf {
        agentiflow_dir(&self.global).join(&self.id)
    }

    /// Run two rounds, round `n` scripted by `rounds[n]`; returns each
    /// round's requests, in order.
    fn run(&self, rounds: Vec<(Vec<Before>, Vec<ScriptedTurn>)>) -> Vec<Vec<String>> {
        let requests: Vec<Arc<Mutex<Vec<String>>>> =
            rounds.iter().map(|_| Arc::default()).collect();
        let mut leads: Vec<ScriptedLead> = rounds
            .into_iter()
            .zip(&requests)
            .map(|((befores, replies), requests)| ScriptedLead {
                global: self.global.clone(),
                id: self.id.clone(),
                befores: befores.into_iter(),
                replies: MockProvider::new(replies),
                requests: requests.clone(),
            })
            .collect();
        leads.reverse();
        let started = chrono::Utc::now();
        run_agentiflow(RunAgentiflowOpts {
            def: AgentiflowDef::parse_str(
                "name: steer\n\
                 lead: lead\n\
                 engagement_profiles: [network]\n\
                 goals:\n  \
                   - id: any-finding\n    \
                     objective: \"Record at least one finding.\"\n    \
                     target: { findings: {}, count_gte: 1 }\n\
                 scope: { authorized: true }\n\
                 pool: { agents: [lead] }\n\
                 round: { lead_max_turns: 4, ceiling: { rounds: 2 } }\n",
            )
            .unwrap(),
            workspace: self.workspace.clone(),
            global: self.global.clone(),
            active: rupu_coverage::builtin_registry()
                .unwrap()
                .active_set(&["network".to_string()])
                .unwrap(),
            started,
            now: Box::new(move || started),
            assembler: RunAssembler::new(AssemblyContext::minimal(self.global.clone())),
            workspace_id: "ws_test".into(),
            lead: LeadInputs {
                agent: rupu_agent::AgentSpec::parse(
                    "---\nname: lead\ntools: []\n---\nYou are the lead.\n",
                )
                .unwrap(),
                provider: LeadProvider::Injected {
                    provider: "mock".into(),
                    model: "mock-1".into(),
                    make: Box::new(move || {
                        Box::new(leads.pop().expect("a lead for this round"))
                            as Box<dyn LlmProvider>
                    }),
                },
            },
            run_id: self.id.clone(),
            process_backend: None,
            generation: None,
            pricing: Default::default(),
        })
        .unwrap();
        requests.iter().map(|r| r.lock().unwrap().clone()).collect()
    }

    /// Who saw each logged message (`seen` receipts), by message body.
    fn seen(&self) -> Vec<(String, String)> {
        let bus = rupu_fleet::Bus::open(&self.dir(), rupu_fleet::address::OPERATOR);
        let lines = bus.read_log(None, usize::MAX).unwrap().lines;
        let body = |id: &str| {
            lines.iter().find_map(|l| match l {
                LogLine::Msg(m) if m.id == id => Some(m.body.clone()),
                _ => None,
            })
        };
        lines
            .iter()
            .filter_map(|l| match l {
                LogLine::Seen { id, by, .. } => Some((body(id).unwrap(), by.clone())),
                LogLine::Msg(_) => None,
            })
            .collect()
    }

    fn round_events(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.dir().join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|e| e["kind"] == "round")
            .collect()
    }
}

const OPERATOR_FRAME: &str = "Message from your operator";

#[test]
fn lead_gets_operator_message_mid_round() {
    // (a) An ordinary message, sent while round 0's first call generates,
    // reaches the lead's next call in the same round.
    let flow = Flow::new("af_mid_round");
    let requests = flow.run(vec![
        (
            vec![Before::Send {
                body: "focus on auth",
                urgent: false,
            }],
            vec![read_the_board(), text("Looking at auth.")],
        ),
        (vec![], vec![text("Round done.")]),
    ]);
    let [round0, round1] = requests.as_slice() else {
        panic!("{requests:?}")
    };
    assert_eq!(round0.len(), 2, "round 0 made two model calls");
    assert!(!round0[0].contains(OPERATOR_FRAME), "{}", round0[0]);
    assert!(round0[1].contains(OPERATOR_FRAME), "{}", round0[1]);
    assert!(round0[1].contains("focus on auth"), "{}", round0[1]);
    assert!(
        !round0[1].contains("URGENT"),
        "an ordinary message isn't urgent"
    );
    // Round 1's prompt only recaps it.
    assert!(
        round1[0].contains("Operator messages this round:"),
        "{}",
        round1[0]
    );
    assert_eq!(
        flow.seen(),
        [("focus on auth".to_string(), "lead".to_string())]
    );
    let rounds = flow.round_events();
    assert_eq!(rounds[0]["outcome"], "yielded");
    assert_eq!(
        (
            rounds[0]["steering"].as_u64(),
            rounds[1]["steering"].as_u64()
        ),
        (Some(0), Some(1))
    );

    // (b) An urgent message ends the round early, and the lead still gets
    // it, on the call that follows: the next round's first.
    let flow = Flow::new("af_urgent");
    let began = Instant::now();
    let requests = flow.run(vec![
        (
            vec![Before::SendAndHang {
                body: "drop the web leg",
            }],
            vec![text("never sent")],
        ),
        (vec![], vec![text("Pivoting.")]),
    ]);
    assert!(
        began.elapsed() < Duration::from_secs(30),
        "round 0 waited out its hung call: {:?}",
        began.elapsed()
    );
    let [round0, round1] = requests.as_slice() else {
        panic!("{requests:?}")
    };
    assert_eq!(round0.len(), 1, "round 0 ended at its first call");
    assert!(round1[0].contains(OPERATOR_FRAME), "{}", round1[0]);
    assert!(
        round1[0].contains("URGENT: drop the web leg"),
        "{}",
        round1[0]
    );
    assert_eq!(
        flow.seen(),
        [("drop the web leg".to_string(), "lead".to_string())]
    );
    let r0: Vec<serde_json::Value> =
        std::fs::read_to_string(flow.dir().join("lead/transcript.r0.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    let complete = r0
        .iter()
        .find(|e| e["type"] == "run_complete")
        .expect("round 0 completed");
    assert_eq!(complete["data"]["error"], "paused", "{complete}");
    assert_eq!(flow.round_events()[1]["steering"], 1);
}
