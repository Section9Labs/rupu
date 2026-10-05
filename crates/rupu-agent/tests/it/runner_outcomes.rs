//! Response outcomes in the turn loop (spec 2026-10-01 §5): every reply is
//! classified, rung 0 recovers in place, a run only succeeds on a normal or
//! warning stop, and replay rebuilds exactly what the runner sent.

use rupu_agent::recovery::{malformed_note, EMPTY_REPLY_NOTE, TRUNCATION_NOTE};
use rupu_agent::runner::{BypassDecider, CapturingMockProvider, MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts, RunResult};
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, Role, Stop, StopReason, StreamEvent, Usage,
};
use rupu_providers::{LlmProvider, ProviderError, ProviderId};
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader, RecoveryAction, RunStatus, Severity};
use std::sync::{Arc, Mutex};

fn build_opts(
    provider: Box<dyn LlmProvider>,
    tmp: &tempfile::TempDir,
    transcript_path: std::path::PathBuf,
) -> AgentRunOpts {
    AgentRunOpts {
        codename: None,
        seed_source: None,
        collectors: Vec::new(),
        agent_name: "outcomes".into(),
        agent_system_prompt: "You are a test agent.".into(),
        agent_tools: None,
        provider,
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_outcomes".into(),
        workspace_id: "ws_outcomes".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_path,
        max_turns: 10,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: tmp.path().to_path_buf(),
            ..Default::default()
        },
        user_message: "write the answer".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
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
        step_id: "s1".into(),
        on_tool_call: None,
        on_stream_event: None,
        on_usage: None,
        concerns: None,
        limits: ModelLimits::unknown(),
        scope_name: None,
        surface_tag: None,
        pause: None,
        recovery: Default::default(),
    }
}

fn text(s: &str) -> ContentBlock {
    ContentBlock::Text {
        text: s.to_string(),
    }
}

fn reply(reason: StopReason, content: Vec<ContentBlock>) -> ScriptedTurn {
    ScriptedTurn::Reply {
        content,
        stop: Stop::synthetic(reason, "mock"),
        usage: Usage::default(),
    }
}

fn malformed_reply() -> ScriptedTurn {
    let mut stop = Stop::synthetic(StopReason::MalformedToolCall, "mock");
    stop.set_detail(
        "malformed_tool",
        serde_json::json!({"name": "read_file", "id": "t1", "error": "EOF"}),
    );
    ScriptedTurn::Reply {
        content: vec![text("reading the file")],
        stop,
        usage: Usage::default(),
    }
}

/// One scripted run: its result, the requests the provider saw, and the
/// transcript events.
struct Ran {
    result: RunResult,
    requests: Vec<LlmRequest>,
    events: Vec<Event>,
}

async fn run_script(turns: Vec<ScriptedTurn>, step_id: &str) -> Ran {
    let provider = CapturingMockProvider::new(turns);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.step_id = step_id.to_string();
    let result = run_agent(opts).await.expect("the loop itself completes");
    let requests = captured.lock().unwrap().clone();
    let events = JsonlReader::iter(&transcript)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    Ran {
        result,
        requests,
        events,
    }
}

fn last_text(m: &Message) -> Option<&str> {
    m.content.iter().rev().find_map(|b| match b {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    })
}

fn outcomes(events: &[Event]) -> Vec<rupu_transcript::OutcomeRecord> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Outcome { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .collect()
}

/// `(rung, action, continues_output, merge_into_previous)` per Recovery.
fn recoveries(events: &[Event]) -> Vec<(u8, RecoveryAction, bool, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Recovery {
                rung,
                action,
                continues_output,
                merge_into_previous,
                ..
            } => Some((*rung, *action, *continues_output, *merge_into_previous)),
            _ => None,
        })
        .collect()
}

fn assert_replay_lockstep(ran: &Ran) {
    let rebuilt = rupu_agent::replay::reconstruct_messages(&ran.events).expect("replay");
    assert_eq!(
        serde_json::to_value(&rebuilt).unwrap(),
        serde_json::to_value(&ran.result.final_messages).unwrap(),
        "replay must rebuild exactly what the runner sent"
    );
}

fn truncation_script() -> Vec<ScriptedTurn> {
    vec![
        reply(StopReason::MaxTokens, vec![text("part one")]),
        reply(StopReason::EndTurn, vec![text("part two")]),
    ]
}

fn empty_script() -> Vec<ScriptedTurn> {
    vec![
        reply(StopReason::EndTurn, vec![]),
        reply(StopReason::EndTurn, vec![text("done")]),
    ]
}

fn malformed_script() -> Vec<ScriptedTurn> {
    vec![
        malformed_reply(),
        reply(StopReason::EndTurn, vec![text("ok")]),
    ]
}

fn pause_script() -> Vec<ScriptedTurn> {
    vec![
        reply(StopReason::PauseTurn, vec![text("a")]),
        reply(StopReason::EndTurn, vec![text("b")]),
    ]
}

#[tokio::test]
async fn truncation_continues_with_a_note_and_joins_the_answer() {
    let ran = run_script(truncation_script(), "s1").await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    assert_eq!(ran.result.turns, 2);
    assert!(ran.result.outcome.is_none());

    assert_eq!(ran.requests.len(), 2);
    let last = ran.requests[1].messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(last_text(last), Some(TRUNCATION_NOTE));

    assert_eq!(
        rupu_transcript::final_turn_text(ran.events.clone()).as_deref(),
        Some("part one\n\npart two")
    );
    let oc = outcomes(&ran.events);
    assert_eq!(oc.len(), 1);
    assert_eq!(oc[0].class, "max_tokens");
    assert_eq!(oc[0].id, "oc_1");
    assert_eq!(
        recoveries(&ran.events),
        vec![(0, RecoveryAction::Continued, true, false)]
    );
    assert!(ran.events.iter().any(|e| matches!(
        e,
        Event::UserMessage { content } if content == TRUNCATION_NOTE
    )));
    // The truncated turn's TurnEnd carries its stop.
    let stops: Vec<String> = ran
        .events
        .iter()
        .filter_map(|e| match e {
            Event::TurnEnd {
                stop: Some(s),
                discarded: false,
                ..
            } => Some(s.reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(stops, vec!["max_tokens", "end_turn"]);
}

#[tokio::test]
async fn truncation_budget_exhausted_fails_with_an_outcome() {
    let turns = (0..4)
        .map(|i| reply(StopReason::MaxTokens, vec![text(&format!("piece {i}"))]))
        .collect();
    let ran = run_script(turns, "s1").await;
    assert_eq!(ran.result.status, RunStatus::Error);
    assert_eq!(
        ran.requests.len(),
        4,
        "three continuations, then the ladder"
    );
    assert_eq!(ran.result.turns, 4, "the failing turn counts");
    let outcome = ran.result.outcome.as_ref().expect("a typed outcome");
    assert_eq!(outcome.class, "max_tokens");

    let complete = ran
        .events
        .iter()
        .find_map(|e| match e {
            Event::RunComplete { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .expect("run_complete");
    assert_eq!(
        complete.as_ref().map(|o| o.class.as_str()),
        Some("max_tokens")
    );

    let rec = recoveries(&ran.events);
    assert_eq!(rec.len(), 4);
    assert_eq!(rec.last().unwrap().0, 3);
    assert_eq!(rec.last().unwrap().1, RecoveryAction::Failed);

    let err = ran.result.error.as_deref().unwrap();
    assert!(err.contains("truncated · output limit"), "{err}");
    assert!(err.contains("no recovery left"), "{err}");
    assert!(err.contains("[recovery].fallbacks"), "{err}");

    let terminal = ran.result.terminal_error().expect("a failed run");
    assert_eq!(
        terminal.outcome().map(|o| o.class.as_str()),
        Some("max_tokens")
    );
}

/// A session turn's rung 3 is the operator's next message (spec §7.3), not
/// a `rupu run --continue` its transcripts could never satisfy.
#[tokio::test]
async fn a_session_turn_hints_at_sending_another_message() {
    let provider = CapturingMockProvider::new(
        (0..4)
            .map(|_| reply(StopReason::MaxTokens, vec![text("piece")]))
            .collect(),
    );
    let tmp = tempfile::tempdir().unwrap();
    let mut opts = build_opts(Box::new(provider), &tmp, tmp.path().join("run.jsonl"));
    opts.step_id = String::new();
    opts.surface_tag = Some("session".into());
    let result = run_agent(opts).await.expect("the loop itself completes");
    let err = result.error.as_deref().unwrap();
    assert!(
        err.ends_with(rupu_agent::runner::SESSION_NO_RECOVERY_HINT),
        "{err}"
    );
    assert!(!err.contains("rupu run"), "{err}");
}

#[tokio::test]
async fn a_standalone_run_hints_at_continue() {
    let turns = (0..4)
        .map(|_| reply(StopReason::MaxTokens, vec![text("piece")]))
        .collect();
    let ran = run_script(turns, "").await;
    let err = ran.result.error.as_deref().unwrap();
    assert!(
        err.contains("rupu run outcomes --continue run_outcomes --model <model>"),
        "{err}"
    );
}

#[tokio::test]
async fn failure_reason_is_the_outcome_without_the_hint() {
    let ran = run_script(vec![refusal_reply()], "").await;
    assert_eq!(ran.result.status, RunStatus::Error);
    let recorded = ran.result.error.clone().unwrap();
    assert!(recorded.contains("no recovery left"), "{recorded}");
    let reason = ran.result.failure_reason().unwrap();
    assert!(reason.starts_with("refused"), "{reason}");
    assert!(!reason.contains("no recovery left"), "{reason}");
    assert!(!reason.contains("--continue"), "{reason}");

    let ok = run_script(vec![reply(StopReason::EndTurn, vec![text("done")])], "").await;
    assert_eq!(ok.result.failure_reason(), None);
}

#[tokio::test]
async fn refusal_with_no_chain_fails_and_discards_the_partial() {
    let mut stop = Stop::synthetic(StopReason::Refusal, "mock");
    stop.refusal = None;
    let ran = run_script(
        vec![ScriptedTurn::Reply {
            content: vec![text("partial")],
            stop,
            usage: Usage::default(),
        }],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Error);
    assert_eq!(ran.result.turns, 1, "the refused turn counts");
    assert_eq!(
        ran.result.outcome.as_ref().map(|o| o.class.as_str()),
        Some("refusal")
    );
    assert!(ran.events.iter().any(|e| matches!(
        e,
        Event::TurnEnd {
            discarded: true,
            ..
        }
    )));
    assert!(!ran.events.iter().any(|e| matches!(
        e,
        Event::AssistantMessage { content, .. } if content == "partial"
    )));
    assert!(!ran
        .result
        .final_messages
        .iter()
        .any(|m| m.role == Role::Assistant && last_text(m) == Some("partial")));
    assert_eq!(
        recoveries(&ran.events),
        vec![(3, RecoveryAction::Failed, false, false)]
    );
    assert_replay_lockstep(&ran);
}

#[tokio::test]
async fn empty_reply_is_nudged_once() {
    let ran = run_script(empty_script(), "s1").await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    assert_eq!(ran.requests.len(), 2);
    let last = ran.requests[1].messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(last_text(last), Some(EMPTY_REPLY_NOTE));
    assert!(
        !ran.requests[1]
            .messages
            .iter()
            .any(|m| m.role == Role::Assistant),
        "the empty reply is never sent back"
    );
    assert_eq!(outcomes(&ran.events)[0].class, "empty_reply");
    assert!(ran.events.iter().any(|e| matches!(
        e,
        Event::TurnEnd {
            discarded: true,
            ..
        }
    )));
}

#[tokio::test]
async fn a_second_empty_reply_fails() {
    let ran = run_script(
        vec![
            reply(StopReason::EndTurn, vec![]),
            reply(StopReason::EndTurn, vec![]),
        ],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Error);
    assert_eq!(
        ran.result.outcome.as_ref().map(|o| o.class.as_str()),
        Some("empty_reply")
    );
}

#[tokio::test]
async fn malformed_tool_call_gets_a_correction() {
    let ran = run_script(malformed_script(), "s1").await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    let note = malformed_note("read_file", "EOF");
    let last = ran.requests[1].messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(last_text(last), Some(note.as_str()));
    assert!(ran.events.iter().any(|e| matches!(
        e,
        Event::UserMessage { content } if *content == note
    )));
    assert_eq!(outcomes(&ran.events)[0].class, "malformed_tool_call");
}

#[tokio::test]
async fn pause_turn_merges_into_the_same_assistant_message() {
    let ran = run_script(pause_script(), "s1").await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    let second = &ran.requests[1].messages;
    let last = second.last().unwrap();
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(last.content, vec![text("a")]);

    let assistants: Vec<&Message> = ran
        .result
        .final_messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .collect();
    assert_eq!(assistants.len(), 1);
    assert_eq!(assistants[0].content, vec![text("a"), text("b")]);
    let oc = outcomes(&ran.events);
    assert_eq!(oc[0].class, "pause_turn");
    assert_eq!(oc[0].severity, Severity::Info);
    assert!(recoveries(&ran.events)
        .iter()
        .any(|r| r.1 == RecoveryAction::Continued && r.3));
}

#[tokio::test]
async fn unrecognized_stop_is_a_warning_and_finishes_by_content() {
    let ran = run_script(
        vec![ScriptedTurn::Reply {
            content: vec![text("the answer")],
            stop: Stop::from_wire(StopReason::Unrecognized, "anthropic", Some("brand_new")),
            usage: Usage::default(),
        }],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    assert_eq!(ran.result.turns, 1);
    let oc = outcomes(&ran.events);
    assert_eq!(oc.len(), 1);
    assert_eq!(oc[0].severity, Severity::Warning);
    assert!(recoveries(&ran.events).is_empty());
}

#[tokio::test]
async fn replay_lockstep_for_every_rung0_action() {
    for script in [
        truncation_script(),
        empty_script(),
        malformed_script(),
        pause_script(),
    ] {
        let ran = run_script(script, "s1").await;
        assert_eq!(ran.result.status, RunStatus::Ok);
        assert_replay_lockstep(&ran);
    }
}

/// Fails the first call with `FallbackUnavailable`, then answers.
struct FallbackRefusedOnce {
    calls: Arc<Mutex<u32>>,
}

#[async_trait::async_trait]
impl LlmProvider for FallbackRefusedOnce {
    async fn send(&mut self, _req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let n = {
            let mut c = self.calls.lock().unwrap();
            *c += 1;
            *c
        };
        if n == 1 {
            return Err(ProviderError::FallbackUnavailable {
                message: "fallbacks: not enabled".into(),
            });
        }
        Ok(LlmResponse {
            id: "r".into(),
            model: "mock-1".into(),
            content: vec![text("answered")],
            stop: Stop::synthetic(StopReason::EndTurn, "anthropic"),
            usage: Usage::default(),
        })
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        _on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.send(req).await
    }

    fn default_model(&self) -> &str {
        "mock-1"
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

#[tokio::test]
async fn fallback_unavailable_writes_a_notice_and_retries_once() {
    let calls = Arc::new(Mutex::new(0));
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let opts = build_opts(
        Box::new(FallbackRefusedOnce {
            calls: calls.clone(),
        }),
        &tmp,
        transcript.clone(),
    );
    let result = run_agent(opts).await.expect("retried after the notice");
    assert_eq!(result.status, RunStatus::Ok);
    assert_eq!(*calls.lock().unwrap(), 2);
    let notices: Vec<String> = JsonlReader::iter(&transcript)
        .unwrap()
        .filter_map(|e| match e.ok()? {
            Event::Notice { kind, .. } if kind == "server_side_fallback_disabled" => Some(kind),
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 1);
}

/// `(rung, action, reason)` per Recovery.
fn recovery_reasons(events: &[Event]) -> Vec<(u8, RecoveryAction, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Recovery {
                rung,
                action,
                reason,
                ..
            } => Some((*rung, *action, reason.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_incomplete_reply_is_retried_once_without_its_partial() {
    let ran = run_script(
        vec![
            reply(StopReason::Incomplete, vec![text("half")]),
            reply(StopReason::EndTurn, vec![text("whole")]),
        ],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    assert_eq!(ran.requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&ran.requests[1].messages).unwrap(),
        serde_json::to_value(&ran.requests[0].messages).unwrap(),
        "the retry re-sends the same conversation"
    );
    assert_eq!(
        recovery_reasons(&ran.events),
        vec![(0, RecoveryAction::Retried, None)]
    );
    assert_replay_lockstep(&ran);
}

#[tokio::test]
async fn a_second_incomplete_reply_fails() {
    let ran = run_script(
        vec![
            reply(StopReason::Incomplete, vec![text("half")]),
            reply(StopReason::Incomplete, vec![text("half again")]),
        ],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Error);
    assert_eq!(
        ran.result.outcome.as_ref().map(|o| o.class.as_str()),
        Some("incomplete")
    );
}

#[tokio::test]
async fn pause_turn_past_its_budget_fails_as_incomplete() {
    let turns = (0..6)
        .map(|i| reply(StopReason::PauseTurn, vec![text(&format!("p{i}"))]))
        .collect();
    let ran = run_script(turns, "s1").await;
    assert_eq!(ran.result.status, RunStatus::Error);
    assert_eq!(ran.requests.len(), 6, "five continuations, then the ladder");
    let oc = outcomes(&ran.events);
    assert_eq!(
        oc.len(),
        7,
        "six pause_turn outcomes and the incomplete one"
    );
    assert_eq!(oc.last().unwrap().class, "incomplete");
    assert_eq!(
        ran.result.outcome.as_ref().map(|o| o.class.as_str()),
        Some("incomplete")
    );
    let assistants = ran
        .result
        .final_messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count();
    assert_eq!(assistants, 1, "every continuation joined one message");
    assert_replay_lockstep(&ran);
}

#[tokio::test]
async fn truncation_with_tool_calls_continues_through_the_tool_results() {
    let ran = run_script(
        vec![
            reply(
                StopReason::MaxTokens,
                vec![
                    text("checking"),
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "read_file".into(),
                        input: serde_json::json!({"path": "missing.txt"}),
                    },
                ],
            ),
            reply(StopReason::EndTurn, vec![text("done")]),
        ],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    assert_eq!(
        recovery_reasons(&ran.events),
        vec![(
            0,
            RecoveryAction::Continued,
            Some("tool calls ran".to_string())
        )]
    );
    assert!(!ran.events.iter().any(|e| matches!(
        e,
        Event::UserMessage { content } if content == TRUNCATION_NOTE
    )));
    let last = ran.requests[1].messages.last().unwrap();
    assert!(matches!(
        last.content.last(),
        Some(ContentBlock::ToolResult { .. })
    ));
    assert_replay_lockstep(&ran);
}

/// Anthropic's `input + max_tokens > window` error (numbers invented).
const INPUT_PLUS_MAX_TOKENS: &str = "API error 400: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"input length and `max_tokens` exceed context limit: 183500 + 20000 > 201000, decrease input length or `max_tokens` and try again\"}}";

fn truncated_tool_reply() -> ScriptedTurn {
    let mut stop = Stop::synthetic(StopReason::MaxTokens, "mock");
    stop.set_detail(
        "truncated_tool",
        serde_json::json!({"name": "write_file", "id": "t9"}),
    );
    ScriptedTurn::Reply {
        content: vec![text("writing")],
        stop,
        usage: Usage::default(),
    }
}

fn truncated_tool_reply_with_input(input_tokens: u32) -> ScriptedTurn {
    match truncated_tool_reply() {
        ScriptedTurn::Reply { content, stop, .. } => ScriptedTurn::Reply {
            content,
            stop,
            usage: Usage {
                input_tokens,
                ..Default::default()
            },
        },
        other => other,
    }
}

fn compaction_count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::Compaction { .. }))
        .count()
}

fn read_events(path: &std::path::Path) -> Vec<Event> {
    JsonlReader::iter(path)
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

/// The cap was lowered because input + max_tokens overflowed the window, so
/// the retry compacts first and then sends the compacted history at the
/// model's full output cap.
#[tokio::test]
async fn a_truncated_tool_call_under_a_lowered_cap_compacts_then_retries_at_the_full_cap() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::ProviderError(INPUT_PLUS_MAX_TOKENS.into()),
        truncated_tool_reply_with_input(900),
        // The compaction summariser's reply.
        reply(StopReason::EndTurn, vec![text("SUMMARY of earlier work")]),
        reply(StopReason::EndTurn, vec![text("written")]),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = vec![
        dense_msg(Role::User, "task"),
        dense_msg(Role::Assistant, "assistant 0"),
        dense_msg(Role::User, "user 0"),
        dense_msg(Role::Assistant, "assistant 1"),
    ];
    opts.limits = ModelLimits::unknown()
        .with_input(2000)
        .with_percent(90)
        .with_output(20_000);
    let result = run_agent(opts).await.unwrap();
    assert_eq!(result.status, RunStatus::Ok);

    let reqs = captured.lock().unwrap().clone();
    assert_eq!(
        reqs.len(),
        4,
        "overflow, truncated reply, summariser, retry"
    );
    assert_eq!(reqs[0].max_tokens, Some(20_000));
    assert_eq!(reqs[1].max_tokens, Some(16_500), "the lowered cap");
    let retry = &reqs[3];
    assert_eq!(retry.max_tokens, Some(20_000), "the model's full cap");
    assert!(
        retry.messages.len() < reqs[1].messages.len(),
        "the retry carries the compacted history"
    );
    assert!(retry
        .messages
        .iter()
        .any(|m| m.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("SUMMARY of earlier work")
        ))));

    let events = read_events(&transcript);
    assert_eq!(compaction_count(&events), 1);
    assert_eq!(
        recovery_reasons(&events),
        vec![
            (0, RecoveryAction::Compacted, None),
            (
                0,
                RecoveryAction::Retried,
                Some("output cap raised to the model maximum after compaction".to_string())
            ),
        ]
    );
    assert!(!result
        .final_messages
        .iter()
        .any(|m| m.role == Role::Assistant && last_text(m) == Some("writing")));
    let rebuilt = rupu_agent::replay::reconstruct_messages(&events).unwrap();
    assert_eq!(
        serde_json::to_value(&rebuilt).unwrap(),
        serde_json::to_value(&result.final_messages).unwrap()
    );
}

/// Nothing to compact (the history is far below the threshold): retrying at
/// the full cap would overflow again, so rung 0 does nothing and the outcome
/// goes to the ladder.
#[tokio::test]
async fn a_truncated_tool_call_under_a_lowered_cap_with_nothing_to_compact_goes_to_the_ladder() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::ProviderError(INPUT_PLUS_MAX_TOKENS.into()),
        truncated_tool_reply(),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.limits = ModelLimits::fixed(1_000_000, 20_000);
    let result = run_agent(opts).await.unwrap();
    assert_eq!(result.status, RunStatus::Error);
    assert_eq!(result.turns, 1);
    assert_eq!(captured.lock().unwrap().len(), 2, "no retry was sent");
    assert_eq!(
        result.outcome.as_ref().map(|o| o.class.as_str()),
        Some("max_tokens")
    );
    let events = read_events(&transcript);
    assert_eq!(compaction_count(&events), 0);
    assert_eq!(
        recovery_reasons(&events)
            .into_iter()
            .map(|(rung, action, _)| (rung, action))
            .collect::<Vec<_>>(),
        vec![(3, RecoveryAction::Failed)]
    );
}

#[tokio::test]
async fn a_truncated_tool_call_with_no_higher_cap_fails() {
    let ran = run_script(vec![truncated_tool_reply()], "s1").await;
    assert_eq!(ran.result.status, RunStatus::Error);
    assert_eq!(ran.requests.len(), 1);
    assert_eq!(
        ran.result.outcome.as_ref().map(|o| o.class.as_str()),
        Some("max_tokens")
    );
    assert_eq!(
        recovery_reasons(&ran.events)
            .into_iter()
            .map(|(rung, action, _)| (rung, action))
            .collect::<Vec<_>>(),
        vec![(3, RecoveryAction::Failed)]
    );
}

fn dense_msg(role: Role, label: &str) -> Message {
    Message {
        role,
        content: vec![text(&format!("{label}: {}", "x".repeat(1000)))],
    }
}

#[tokio::test]
async fn context_window_exceeded_compacts_then_continues() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::Reply {
            content: vec![text("partial")],
            stop: Stop::synthetic(StopReason::ContextWindowExceeded, "mock"),
            usage: Usage {
                input_tokens: 900,
                ..Default::default()
            },
        },
        // The compaction summariser's reply.
        reply(StopReason::EndTurn, vec![text("SUMMARY of earlier work")]),
        reply(StopReason::EndTurn, vec![text("finished")]),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = vec![
        dense_msg(Role::User, "task"),
        dense_msg(Role::Assistant, "assistant 0"),
        dense_msg(Role::User, "user 0"),
        dense_msg(Role::Assistant, "assistant 1"),
    ];
    // Unknown output cap; a 2,000-token input limit compacting at 90%.
    opts.limits = ModelLimits::unknown().with_input(2000).with_percent(90);
    let result = run_agent(opts).await.unwrap();
    assert_eq!(result.status, RunStatus::Ok);
    assert_eq!(captured.lock().unwrap().len(), 3);
    let events: Vec<Event> = JsonlReader::iter(&transcript)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    let actions: Vec<RecoveryAction> = recovery_reasons(&events)
        .into_iter()
        .map(|(_, action, _)| action)
        .collect();
    assert_eq!(
        actions,
        vec![RecoveryAction::Compacted, RecoveryAction::Continued]
    );
    let last = captured.lock().unwrap()[2].messages.last().unwrap().clone();
    assert_eq!(last_text(&last), Some(TRUNCATION_NOTE));
    let rebuilt = rupu_agent::replay::reconstruct_messages(&events).unwrap();
    assert_eq!(
        serde_json::to_value(&rebuilt).unwrap(),
        serde_json::to_value(&result.final_messages).unwrap()
    );
}

/// Over the proactive threshold AND stopped on a full context window: the
/// proactive compaction already ran this turn, so rung 0 does not summarise
/// the history it just produced a second time.
#[tokio::test]
async fn context_window_exceeded_after_a_proactive_compaction_compacts_once() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::Reply {
            content: vec![text("partial")],
            stop: Stop::synthetic(StopReason::ContextWindowExceeded, "mock"),
            usage: Usage {
                input_tokens: 1900,
                ..Default::default()
            },
        },
        // The one summariser call.
        reply(StopReason::EndTurn, vec![text("SUMMARY of earlier work")]),
        reply(StopReason::EndTurn, vec![text("finished")]),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = vec![
        dense_msg(Role::User, "task"),
        dense_msg(Role::Assistant, "assistant 0"),
        dense_msg(Role::User, "user 0"),
        dense_msg(Role::Assistant, "assistant 1"),
    ];
    // Threshold 1,800: the reply's 1,900 input tokens trigger the proactive
    // compaction.
    opts.limits = ModelLimits::unknown().with_input(2000).with_percent(90);
    let result = run_agent(opts).await.unwrap();
    assert_eq!(result.status, RunStatus::Ok);
    assert_eq!(
        captured.lock().unwrap().len(),
        3,
        "the turn, ONE summariser call, the continuation"
    );
    let events = read_events(&transcript);
    assert_eq!(compaction_count(&events), 1);
    let actions: Vec<RecoveryAction> = recovery_reasons(&events)
        .into_iter()
        .map(|(_, action, _)| action)
        .collect();
    assert_eq!(
        actions,
        vec![RecoveryAction::Compacted, RecoveryAction::Continued]
    );
}

/// truncation → empty reply (nudged) → the rest: the final answer still
/// joins the earlier piece, because the discarded turn is transparent to
/// the continuation chain.
#[tokio::test]
async fn a_nudged_empty_turn_inside_a_truncation_chain_keeps_the_first_piece() {
    let ran = run_script(
        vec![
            reply(StopReason::MaxTokens, vec![text("part one")]),
            reply(StopReason::EndTurn, vec![]),
            reply(StopReason::EndTurn, vec![text("part two")]),
        ],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    assert_eq!(ran.result.turns, 3);
    assert_eq!(
        rupu_transcript::final_turn_text(ran.events.clone()).as_deref(),
        Some("part one\n\npart two")
    );
    assert_replay_lockstep(&ran);
}

// ---------------------------------------------------------------------------
// Rungs 1–2: fallback hops (spec 2026-10-01 §5.2, §6.2).
// ---------------------------------------------------------------------------

/// A test [`HopBuilder`]: `(provider, model)` → a prebuilt provider (handed
/// out once), anything else → `Err("no credentials")`. Records every call.
struct TestHops {
    hops: Mutex<std::collections::HashMap<(String, String), Box<dyn LlmProvider>>>,
    calls: Arc<Mutex<Vec<(String, String)>>>,
}

impl TestHops {
    fn new(hops: Vec<(&str, &str, Box<dyn LlmProvider>)>) -> Self {
        Self {
            hops: Mutex::new(
                hops.into_iter()
                    .map(|(p, m, prov)| ((p.to_string(), m.to_string()), prov))
                    .collect(),
            ),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl rupu_agent::HopBuilder for TestHops {
    async fn build(&self, provider: &str, model: &str) -> Result<rupu_agent::Hop, String> {
        let key = (provider.to_string(), model.to_string());
        self.calls.lock().unwrap().push(key.clone());
        match self.hops.lock().unwrap().remove(&key) {
            Some(p) => Ok(rupu_agent::Hop {
                provider: p,
                provider_name: provider.to_string(),
                model: model.to_string(),
                limits: ModelLimits::unknown(),
            }),
            None => Err("no credentials".to_string()),
        }
    }
}

fn fallback(provider: Option<&str>, model: &str) -> rupu_config::FallbackEntry {
    rupu_config::FallbackEntry {
        provider: provider.map(str::to_string),
        model: model.to_string(),
    }
}

/// A run with a fallback chain and (optionally) a hop builder.
struct HopRun {
    result: Result<RunResult, rupu_agent::runner::RunError>,
    events: Vec<Event>,
    builds: Vec<(String, String)>,
}

async fn run_with_hops(
    primary: Box<dyn LlmProvider>,
    origin: (&str, &str),
    chain: Vec<rupu_config::FallbackEntry>,
    hops: Option<TestHops>,
    tmp: &tempfile::TempDir,
) -> HopRun {
    run_with_hops_and(primary, origin, chain, hops, tmp, |_| {}).await
}

/// [`run_with_hops`], with `edit` applied to the run's options first.
async fn run_with_hops_and(
    primary: Box<dyn LlmProvider>,
    (provider_name, model): (&str, &str),
    chain: Vec<rupu_config::FallbackEntry>,
    hops: Option<TestHops>,
    tmp: &tempfile::TempDir,
    edit: impl FnOnce(&mut AgentRunOpts),
) -> HopRun {
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(primary, tmp, transcript.clone());
    opts.provider_name = provider_name.into();
    opts.model = model.into();
    edit(&mut opts);
    let builds = hops.as_ref().map(|h| h.calls.clone()).unwrap_or_default();
    opts.recovery = rupu_agent::RecoveryOpts {
        chain,
        hop_builder: hops.map(|h| Arc::new(h) as Arc<dyn rupu_agent::HopBuilder>),
    };
    let result = run_agent(opts).await;
    let events = read_events(&transcript);
    let builds = builds.lock().unwrap().clone();
    HopRun {
        result,
        events,
        builds,
    }
}

/// `(rung, action, provider, model, reason)` per Recovery.
type RecoveryRow = (
    u8,
    RecoveryAction,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn recovery_rows(events: &[Event]) -> Vec<RecoveryRow> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Recovery {
                rung,
                action,
                provider,
                model,
                reason,
                ..
            } => Some((
                *rung,
                *action,
                provider.clone(),
                model.clone(),
                reason.clone(),
            )),
            _ => None,
        })
        .collect()
}

fn refusal_reply() -> ScriptedTurn {
    ScriptedTurn::Reply {
        content: vec![text("I can't help with that")],
        stop: Stop::synthetic(StopReason::Refusal, "anthropic"),
        usage: Usage::default(),
    }
}

fn overloaded() -> ScriptedTurn {
    ScriptedTurn::ReplyError {
        body: rupu_providers::reply_error::parse_error_body(
            "anthropic",
            rupu_providers::reply_error::ErrorOrigin::Http { status: 529 },
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            None,
            None,
        ),
    }
}

/// The served-by-a-hop runs' replay check: hops never change the messages.
fn assert_hop_lockstep(run: &HopRun) {
    let result = run.result.as_ref().expect("an Ok run");
    let rebuilt = rupu_agent::replay::reconstruct_messages(&run.events).expect("replay");
    assert_eq!(
        serde_json::to_value(&rebuilt).unwrap(),
        serde_json::to_value(&result.final_messages).unwrap(),
        "replay must rebuild exactly what the runner sent"
    );
}

#[tokio::test]
async fn refusal_falls_back_on_the_same_provider_and_sticks() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("notes.txt"), "the notes").unwrap();
    // One turn only: a second call to the primary would fail with "mock
    // script exhausted", so completing proves the hop is sticky.
    let primary = MockProvider::new(vec![refusal_reply()]);
    let hop = CapturingMockProvider::new(vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "c1".into(),
            tool_name: "read_file".into(),
            tool_input: serde_json::json!({ "path": "notes.txt" }),
            stop: StopReason::ToolUse,
        },
        reply(StopReason::EndTurn, vec![text("answer")]),
    ]);
    let hop_requests = hop.captured.clone();
    let run = run_with_hops(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(None, "claude-opus-4-8")],
        Some(TestHops::new(vec![(
            "anthropic",
            "claude-opus-4-8",
            Box::new(hop),
        )])),
        &tmp,
    )
    .await;
    let result = run.result.as_ref().expect("the hop answered");
    assert_eq!(result.status, RunStatus::Ok);
    assert!(recovery_rows(&run.events).contains(&(
        1,
        RecoveryAction::FellBack,
        Some("anthropic".into()),
        Some("claude-opus-4-8".into()),
        None
    )));
    let hop_requests = hop_requests.lock().unwrap();
    assert_eq!(hop_requests.len(), 2, "both later turns went to the hop");
    assert!(hop_requests.iter().all(|r| r.model == "claude-opus-4-8"));
    // The hop resolves its own limits and says so.
    assert_eq!(
        run.events
            .iter()
            .filter(|e| matches!(e, Event::Notice { kind, .. } if kind == "model_limits"))
            .count(),
        2,
        "the run's notice, then the hop's"
    );
    // Usage after the hop is attributed to it.
    let usage: Vec<String> = run
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Usage { model, .. } => Some(model.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        usage,
        vec!["claude-opus-5-5", "claude-opus-4-8", "claude-opus-4-8"]
    );
    assert!(!result
        .final_messages
        .iter()
        .any(|m| last_text(m) == Some("I can't help with that")));
    assert_hop_lockstep(&run);
}

#[tokio::test]
async fn skipped_hop_then_cross_provider_hop() {
    let tmp = tempfile::tempdir().unwrap();
    let run = run_with_hops(
        Box::new(MockProvider::new(vec![refusal_reply()])),
        ("anthropic", "claude-opus-5-5"),
        vec![
            fallback(None, "missing-model"),
            fallback(Some("openai-codex"), "gpt-test"),
        ],
        Some(TestHops::new(vec![(
            "openai-codex",
            "gpt-test",
            Box::new(
                MockProvider::new(vec![reply(StopReason::EndTurn, vec![text("done")])])
                    .with_provider_id(ProviderId::OpenaiCodex),
            ),
        )])),
        &tmp,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    assert_eq!(
        run.builds,
        vec![
            ("anthropic".to_string(), "missing-model".to_string()),
            ("openai-codex".to_string(), "gpt-test".to_string()),
        ]
    );
    assert_eq!(
        recovery_rows(&run.events),
        vec![
            (
                1,
                RecoveryAction::Skipped,
                Some("anthropic".into()),
                Some("missing-model".into()),
                Some("no credentials".into())
            ),
            (
                2,
                RecoveryAction::FellBack,
                Some("openai-codex".into()),
                Some("gpt-test".into()),
                None
            ),
        ]
    );
    assert_hop_lockstep(&run);
}

#[tokio::test(start_paused = true)]
async fn overloaded_after_retries_goes_cross_provider() {
    let tmp = tempfile::tempdir().unwrap();
    // The first attempt plus MAX_HTTP_RETRIES (10) retries.
    let primary = MockProvider::new((0..11).map(|_| overloaded()).collect());
    let hop = CapturingMockProvider::new(vec![reply(StopReason::EndTurn, vec![text("done")])]);
    let hop_requests = hop.captured.clone();
    let run = run_with_hops(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![
            fallback(None, "claude-opus-4-8"),
            fallback(Some("openai-codex"), "gpt-test"),
        ],
        Some(TestHops::new(vec![(
            "openai-codex",
            "gpt-test",
            Box::new(hop),
        )])),
        &tmp,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    // Every transient retry ran before the ladder took over.
    let fell_back_at = run
        .events
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::Recovery {
                    action: RecoveryAction::FellBack,
                    ..
                }
            )
        })
        .expect("a FellBack row");
    let retries = run.events[..fell_back_at]
        .iter()
        .filter(|e| matches!(e, Event::Notice { kind, .. } if kind == "provider_retry"))
        .count();
    assert_eq!(retries, 10, "MAX_HTTP_RETRIES retries, then the hop");
    assert!(!run.events[fell_back_at..]
        .iter()
        .any(|e| matches!(e, Event::Notice { kind, .. } if kind == "provider_retry")));
    let hop_requests = hop_requests.lock().unwrap();
    assert_eq!(hop_requests.len(), 1);
    assert_eq!(
        hop_requests[0].model, "gpt-test",
        "the request is rebuilt for the hop"
    );
    let outs = outcomes(&run.events);
    assert_eq!(outs.len(), 1, "{outs:?}");
    assert_eq!(outs[0].class, "provider_error");
    assert_eq!(outs[0].error_class.as_deref(), Some("overloaded"));
    // Overloaded skips rung 1: the same provider is the one overloaded.
    assert_eq!(
        run.builds,
        vec![("openai-codex".to_string(), "gpt-test".to_string())]
    );
    assert_eq!(
        recovery_rows(&run.events),
        vec![(
            2,
            RecoveryAction::FellBack,
            Some("openai-codex".into()),
            Some("gpt-test".into()),
            None
        )]
    );
    assert_hop_lockstep(&run);
}

#[tokio::test]
async fn auth_error_does_not_hop() {
    let tmp = tempfile::tempdir().unwrap();
    let primary = MockProvider::new(vec![ScriptedTurn::ReplyError {
        body: rupu_providers::reply_error::parse_error_body(
            "anthropic",
            rupu_providers::reply_error::ErrorOrigin::Http { status: 401 },
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
            None,
            None,
        ),
    }]);
    let run = run_with_hops(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![
            fallback(None, "claude-opus-4-8"),
            fallback(Some("openai-codex"), "gpt-test"),
        ],
        Some(TestHops::new(vec![(
            "anthropic",
            "claude-opus-4-8",
            Box::new(MockProvider::new(vec![reply(
                StopReason::EndTurn,
                vec![text("never")],
            )])),
        )])),
        &tmp,
    )
    .await;
    let Err(err) = run.result.as_ref() else {
        panic!("an auth error fails the run");
    };
    assert!(
        matches!(err, rupu_agent::runner::RunError::Outcome { .. }),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "provider: API error 401: invalid x-api-key",
        "the error text is unchanged"
    );
    assert!(
        err.hint()
            .is_some_and(|h| h.starts_with("no recovery left")),
        "the rung-3 hint travels with the error: {:?}",
        err.hint()
    );
    assert_eq!(
        err.outcome().and_then(|o| o.error_class.as_deref()),
        Some("auth")
    );
    assert!(run.builds.is_empty(), "no hop was built");
    let rows = recovery_rows(&run.events);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].0, rows[0].1), (3, RecoveryAction::Failed));
    let closing = run.events.iter().find_map(|e| match e {
        Event::RunComplete { error, outcome, .. } => Some((error.clone(), outcome.clone())),
        _ => None,
    });
    let (error, outcome) = closing.expect("RunComplete");
    assert_eq!(
        error.as_deref(),
        Some("provider: API error 401: invalid x-api-key")
    );
    assert_eq!(outcome.map(|o| o.class), Some("provider_error".to_string()));
}

#[tokio::test]
async fn served_by_fallback_is_recorded_and_the_answer_kept() {
    let mut stop = Stop::synthetic(StopReason::EndTurn, "anthropic");
    stop.served_by = Some(rupu_providers::ServedBy {
        model: "claude-opus-4-8".into(),
        hops: vec![rupu_providers::FallbackHop {
            from_model: "claude-opus-5-5".into(),
            to_model: "claude-opus-4-8".into(),
        }],
    });
    let ran = run_script(
        vec![ScriptedTurn::Reply {
            content: vec![text("the fallback's answer")],
            stop,
            usage: Usage::default(),
        }],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    assert_eq!(
        rupu_transcript::final_turn_text(ran.events.clone()).as_deref(),
        Some("the fallback's answer")
    );
    let outs = outcomes(&ran.events);
    assert_eq!(outs.len(), 1, "{outs:?}");
    assert_eq!(outs[0].class, "refusal");
    assert_eq!(outs[0].title, "refused · served by claude-opus-4-8");
    assert_eq!(outs[0].severity, Severity::Error);
    let rows = recovery_rows(&ran.events);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        (rows[0].0, rows[0].1, rows[0].3.as_deref()),
        (1, RecoveryAction::ServedByFallback, Some("claude-opus-4-8"))
    );
    let linked = ran
        .events
        .iter()
        .any(|e| matches!(e, Event::Recovery { outcome_id, .. } if *outcome_id == outs[0].id));
    assert!(linked, "the recovery points at the outcome");
    assert_replay_lockstep(&ran);
}

#[tokio::test]
async fn max_recovery_actions_caps_the_ladder() {
    let tmp = tempfile::tempdir().unwrap();
    let chain = (0..25)
        .map(|i| fallback(None, &format!("unbuildable-{i}")))
        .collect();
    let run = run_with_hops(
        Box::new(MockProvider::new(vec![refusal_reply()])),
        ("anthropic", "claude-opus-5-5"),
        chain,
        Some(TestHops::new(Vec::new())),
        &tmp,
    )
    .await;
    let result = run.result.as_ref().unwrap();
    assert_eq!(result.status, RunStatus::Error);
    let rows = recovery_rows(&run.events);
    let skips = rows
        .iter()
        .filter(|r| r.1 == RecoveryAction::Skipped)
        .count();
    assert_eq!(
        skips,
        rupu_agent::recovery::MAX_RECOVERY_ACTIONS as usize,
        "skips count as actions"
    );
    assert_eq!(
        rows.last().map(|r| (r.0, r.1)),
        Some((3, RecoveryAction::Failed))
    );
}

#[tokio::test]
async fn a_chain_without_a_hop_builder_is_skipped_with_a_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let run = run_with_hops(
        Box::new(MockProvider::new(vec![refusal_reply()])),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(None, "claude-opus-4-8")],
        None,
        &tmp,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Error);
    assert_eq!(
        recovery_rows(&run.events)[0],
        (
            1,
            RecoveryAction::Skipped,
            Some("anthropic".into()),
            Some("claude-opus-4-8".into()),
            Some("no hop builder in this context".into())
        )
    );
}

/// Truncation past its budget leaves the truncated text as the last
/// message: the hop gets the retry note as a user turn, so the request it
/// sees ends on a user message.
#[tokio::test]
async fn a_hop_after_kept_content_gets_the_retry_note() {
    let tmp = tempfile::tempdir().unwrap();
    let primary = MockProvider::new(
        (0..4)
            .map(|_| reply(StopReason::MaxTokens, vec![text("piece")]))
            .collect(),
    );
    let hop = CapturingMockProvider::new(vec![reply(StopReason::EndTurn, vec![text("done")])]);
    let hop_requests = hop.captured.clone();
    let run = run_with_hops(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(None, "claude-opus-4-8")],
        Some(TestHops::new(vec![(
            "anthropic",
            "claude-opus-4-8",
            Box::new(hop),
        )])),
        &tmp,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    let req = hop_requests.lock().unwrap()[0].clone();
    let last = req.messages.last().expect("messages");
    assert_eq!(last.role, Role::User);
    assert_eq!(
        last_text(last),
        Some(
            rupu_agent::recovery::recovery_retry_note(
                "truncated · output limit",
                "anthropic",
                "claude-opus-4-8"
            )
            .as_str()
        )
    );
    assert_hop_lockstep(&run);
}

/// A context overflow nothing can compact or trim climbs to rung 2.
#[tokio::test]
async fn context_overflow_exhaustion_hops_to_another_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let primary = MockProvider::new(vec![ScriptedTurn::ProviderError(
        "prompt is too long: 250000 tokens > 200000 maximum".into(),
    )]);
    let run = run_with_hops(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(Some("openai-codex"), "gpt-test")],
        Some(TestHops::new(vec![(
            "openai-codex",
            "gpt-test",
            Box::new(MockProvider::new(vec![reply(
                StopReason::EndTurn,
                vec![text("done")],
            )])),
        )])),
        &tmp,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    let outs = outcomes(&run.events);
    assert_eq!(outs[0].error_class.as_deref(), Some("context_overflow"));
    assert_eq!(outs[0].title, "provider error · context_overflow");
    assert!(recovery_rows(&run.events)
        .iter()
        .any(|r| (r.0, r.1) == (2, RecoveryAction::FellBack)));
}

/// Still paused past the pause budget: the reply is `Incomplete`, and it
/// climbs under `Incomplete`'s policy (rungs 1 and 2), the incomplete
/// outcome being the one the hop answers.
#[tokio::test]
async fn pause_turn_past_its_budget_hops_as_incomplete() {
    let tmp = tempfile::tempdir().unwrap();
    let primary = MockProvider::new(
        (0..6)
            .map(|i| reply(StopReason::PauseTurn, vec![text(&format!("p{i}"))]))
            .collect(),
    );
    let run = run_with_hops(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(None, "claude-opus-4-8")],
        Some(TestHops::new(vec![(
            "anthropic",
            "claude-opus-4-8",
            Box::new(MockProvider::new(vec![reply(
                StopReason::EndTurn,
                vec![text("done")],
            )])),
        )])),
        &tmp,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    let incomplete = outcomes(&run.events)
        .into_iter()
        .find(|o| o.class == "incomplete")
        .expect("the incomplete outcome");
    let fell_back = run.events.iter().any(|e| {
        matches!(
            e,
            Event::Recovery { outcome_id, rung: 1, action: RecoveryAction::FellBack, .. }
                if *outcome_id == incomplete.id
        )
    });
    assert!(fell_back, "rung 1 answers the incomplete outcome");
    assert_hop_lockstep(&run);
}

/// Unnamed chain entries mean the provider the attempt started on, even
/// after a hop to another provider (spec §6.1): anthropic overloaded →
/// rung 2 to codex; codex refuses → rung 1 is `(anthropic, A)`, not
/// `(codex, A)`; that fails to build → gemini.
#[tokio::test(start_paused = true)]
async fn unnamed_entries_stay_on_the_origin_provider_after_a_hop() {
    let tmp = tempfile::tempdir().unwrap();
    let primary = MockProvider::new((0..11).map(|_| overloaded()).collect());
    let codex = MockProvider::new(vec![ScriptedTurn::Reply {
        content: vec![text("no")],
        stop: Stop::synthetic(StopReason::Refusal, "openai-codex"),
        usage: Usage::default(),
    }])
    .with_provider_id(ProviderId::OpenaiCodex);
    let run = run_with_hops(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![
            fallback(None, "claude-a"),
            fallback(Some("openai-codex"), "gpt-x"),
            fallback(Some("gemini"), "gemini-y"),
        ],
        Some(TestHops::new(vec![
            ("openai-codex", "gpt-x", Box::new(codex)),
            (
                "gemini",
                "gemini-y",
                Box::new(MockProvider::new(vec![reply(
                    StopReason::EndTurn,
                    vec![text("done")],
                )])),
            ),
        ])),
        &tmp,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    let pair = |p: &str, m: &str| (p.to_string(), m.to_string());
    assert_eq!(
        run.builds,
        vec![
            pair("openai-codex", "gpt-x"),
            pair("anthropic", "claude-a"),
            pair("gemini", "gemini-y"),
        ]
    );
    let rows: Vec<(u8, RecoveryAction, Option<String>, Option<String>)> =
        recovery_rows(&run.events)
            .into_iter()
            .map(|(rung, action, provider, model, _)| (rung, action, provider, model))
            .collect();
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        rows,
        vec![
            (
                2,
                RecoveryAction::FellBack,
                some("openai-codex"),
                some("gpt-x")
            ),
            (
                1,
                RecoveryAction::Skipped,
                some("anthropic"),
                some("claude-a")
            ),
            (
                2,
                RecoveryAction::FellBack,
                some("gemini"),
                some("gemini-y")
            ),
        ]
    );
}

/// The origin model's `contextWindow: 1m`, `anthropicSpeed: fast`,
/// `anthropicTaskBudget` and `anthropicContextManagement`, plus a
/// provider-generic `effort`.
fn pin_origin_model(opts: &mut AgentRunOpts) {
    opts.context_window = Some(rupu_providers::model_tier::ContextWindow::OneMillion);
    opts.anthropic_speed = Some(rupu_providers::types::Speed::Fast);
    opts.anthropic_task_budget = Some(40_000);
    opts.anthropic_context_management = Some(ORIGIN_CONTEXT_MANAGEMENT);
    opts.effort = Some(rupu_providers::model_tier::ThinkingLevel::High);
}

const ORIGIN_CONTEXT_MANAGEMENT: rupu_providers::types::ContextManagement =
    rupu_providers::types::ContextManagement::ToolClearing;

/// `(context_window, anthropic_speed)` per request, after asserting the
/// task budget and context management follow the same rule (both set or
/// both cleared with them) and `effort` is always kept.
fn pins(
    requests: &Arc<Mutex<Vec<LlmRequest>>>,
) -> Vec<(
    Option<rupu_providers::model_tier::ContextWindow>,
    Option<rupu_providers::types::Speed>,
)> {
    requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| {
            let pinned = r.anthropic_speed.is_some();
            assert_eq!(r.anthropic_task_budget, pinned.then_some(40_000));
            assert_eq!(
                r.anthropic_context_management,
                pinned.then_some(ORIGIN_CONTEXT_MANAGEMENT)
            );
            assert_eq!(
                r.thinking,
                Some(rupu_providers::model_tier::ThinkingLevel::High),
                "effort is provider-generic: every hop keeps it"
            );
            (r.context_window, r.anthropic_speed)
        })
        .collect()
}

/// A hop to another model drops the origin model's request pins: the hop's
/// model may have no 1M window, and `speed: fast` errors on a model without
/// fast mode (the rule `rupu run --model` applies).
#[tokio::test]
async fn a_hop_to_another_model_drops_the_origin_model_pins() {
    let tmp = tempfile::tempdir().unwrap();
    let primary = CapturingMockProvider::new(vec![refusal_reply()]);
    let primary_requests = primary.captured.clone();
    let hop = CapturingMockProvider::new(vec![reply(StopReason::EndTurn, vec![text("answer")])]);
    let hop_requests = hop.captured.clone();
    let run = run_with_hops_and(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(None, "claude-sonnet-4-6")],
        Some(TestHops::new(vec![(
            "anthropic",
            "claude-sonnet-4-6",
            Box::new(hop),
        )])),
        &tmp,
        pin_origin_model,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    use rupu_providers::model_tier::ContextWindow;
    use rupu_providers::types::Speed;
    assert_eq!(
        pins(&primary_requests),
        vec![(Some(ContextWindow::OneMillion), Some(Speed::Fast))],
        "the origin model sends its pins"
    );
    assert_eq!(pins(&hop_requests), vec![(None, None)]);
}

/// The error path retries the request it already built: the pins come off
/// that request too.
#[tokio::test]
async fn an_error_path_hop_to_another_model_drops_the_origin_model_pins() {
    let tmp = tempfile::tempdir().unwrap();
    let primary = MockProvider::new(vec![ScriptedTurn::ReplyError {
        body: rupu_providers::reply_error::parse_error_body(
            "anthropic",
            rupu_providers::reply_error::ErrorOrigin::Http { status: 404 },
            r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-opus-5-5"}}"#,
            None,
            None,
        ),
    }]);
    let hop = CapturingMockProvider::new(vec![reply(StopReason::EndTurn, vec![text("answer")])]);
    let hop_requests = hop.captured.clone();
    let run = run_with_hops_and(
        Box::new(primary),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(None, "claude-sonnet-4-6")],
        Some(TestHops::new(vec![(
            "anthropic",
            "claude-sonnet-4-6",
            Box::new(hop),
        )])),
        &tmp,
        pin_origin_model,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    assert!(recovery_rows(&run.events)
        .iter()
        .any(|r| r.1 == RecoveryAction::FellBack));
    assert_eq!(pins(&hop_requests), vec![(None, None)]);
}

/// A hop that keeps the origin's model string (another provider serving the
/// same model) keeps its pins.
#[tokio::test]
async fn a_hop_that_keeps_the_origin_model_keeps_its_pins() {
    let tmp = tempfile::tempdir().unwrap();
    let hop = CapturingMockProvider::new(vec![reply(StopReason::EndTurn, vec![text("answer")])]);
    let hop_requests = hop.captured.clone();
    let run = run_with_hops_and(
        Box::new(MockProvider::new(vec![refusal_reply()])),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(Some("other-gateway"), "claude-opus-5-5")],
        Some(TestHops::new(vec![(
            "other-gateway",
            "claude-opus-5-5",
            Box::new(hop),
        )])),
        &tmp,
        pin_origin_model,
    )
    .await;
    assert_eq!(run.result.as_ref().unwrap().status, RunStatus::Ok);
    use rupu_providers::model_tier::ContextWindow;
    use rupu_providers::types::Speed;
    assert_eq!(
        pins(&hop_requests),
        vec![(Some(ContextWindow::OneMillion), Some(Speed::Fast))]
    );
}

/// A provider that fails every call with `ProviderError::Terminating`.
struct TerminatingError;

#[async_trait::async_trait]
impl LlmProvider for TerminatingError {
    async fn send(&mut self, _req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        Err(ProviderError::Terminating)
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        _on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.send(req).await
    }

    fn default_model(&self) -> &str {
        "mock-1"
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

/// `ProviderError::Terminating` closes the run as aborted: no outcome, no
/// ladder, no hop built.
#[tokio::test]
async fn a_terminating_provider_error_aborts_without_the_ladder() {
    let tmp = tempfile::tempdir().unwrap();
    let run = run_with_hops(
        Box::new(TerminatingError),
        ("anthropic", "claude-opus-5-5"),
        vec![fallback(Some("openai-codex"), "gpt-test")],
        Some(TestHops::new(vec![(
            "openai-codex",
            "gpt-test",
            Box::new(MockProvider::new(vec![reply(
                StopReason::EndTurn,
                vec![text("never")],
            )])),
        )])),
        &tmp,
    )
    .await;
    assert!(
        matches!(run.result, Err(rupu_agent::runner::RunError::Terminating)),
        "{:?}",
        run.result.as_ref().err()
    );
    assert!(run.builds.is_empty(), "no hop was built");
    assert!(outcomes(&run.events).is_empty());
    assert!(recovery_rows(&run.events).is_empty());
    assert!(run.events.iter().any(|e| matches!(
        e,
        Event::RunComplete {
            status: RunStatus::Aborted,
            ..
        }
    )));
}

// ---------------------------------------------------------------------------
// Blocks with no event of their own, and the mid-output fallback boundary
// (spec 2026-10-01 §4.5, §5.4, §7.1).
// ---------------------------------------------------------------------------

fn fallback_block() -> ContentBlock {
    ContentBlock::Fallback {
        from_model: "claude-opus-5-5".into(),
        to_model: "claude-opus-4-8".into(),
    }
}

fn served_by_stop(reason: StopReason) -> Stop {
    let mut stop = Stop::synthetic(reason, "anthropic");
    stop.served_by = Some(rupu_providers::ServedBy {
        model: "claude-opus-4-8".into(),
        hops: vec![rupu_providers::FallbackHop {
            from_model: "claude-opus-5-5".into(),
            to_model: "claude-opus-4-8".into(),
        }],
    });
    stop
}

/// `(block, abandoned)` per `AssistantBlock` event.
fn assistant_blocks(events: &[Event]) -> Vec<(serde_json::Value, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::AssistantBlock { block, abandoned } => Some((block.clone(), *abandoned)),
            _ => None,
        })
        .collect()
}

/// A mid-output server-side fallback: the tool call before the boundary
/// belongs to the swapped-out model. It is not dispatched, the next request
/// carries no `tool_result` for it, and the transcript keeps it as an
/// abandoned `assistant_block`. The call after the boundary runs.
#[tokio::test]
async fn a_tool_call_before_a_mid_output_fallback_is_abandoned() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("notes.txt"), "the notes").unwrap();
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::Reply {
            content: vec![
                text("Let me check the notes."),
                ContentBlock::ToolUse {
                    id: "abandoned_1".into(),
                    name: "write_file".into(),
                    input: serde_json::json!({ "path": "out.txt", "content": "x" }),
                },
                fallback_block(),
                text("Reading them now."),
                ContentBlock::ToolUse {
                    id: "kept_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "notes.txt" }),
                },
            ],
            stop: served_by_stop(StopReason::ToolUse),
            usage: Usage::default(),
        },
        reply(StopReason::EndTurn, vec![text("done")]),
    ]);
    let captured = provider.captured.clone();
    let transcript = tmp.path().join("run.jsonl");
    let opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    let result = run_agent(opts).await.expect("the loop completes");
    assert_eq!(result.status, RunStatus::Ok);
    let events = read_events(&transcript);

    assert!(
        !tmp.path().join("out.txt").exists(),
        "the abandoned write_file never ran"
    );
    let called: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::ToolCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(called, vec!["kept_1"]);
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::ToolResult { call_id, .. } if call_id == "abandoned_1")));

    let requests = captured.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let mentions_abandoned = requests[1].messages.iter().any(|m| {
        m.content.iter().any(|b| match b {
            ContentBlock::ToolUse { id, .. } => id == "abandoned_1",
            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id == "abandoned_1",
            _ => false,
        })
    });
    assert!(
        !mentions_abandoned,
        "no tool_use or tool_result for the abandoned call"
    );
    let assistant = requests[1]
        .messages
        .iter()
        .find(|m| m.role == Role::Assistant)
        .expect("the reply is in the next request");
    assert_eq!(
        assistant.content,
        vec![
            text("Let me check the notes."),
            fallback_block(),
            text("Reading them now."),
            ContentBlock::ToolUse {
                id: "kept_1".into(),
                name: "read_file".into(),
                input: serde_json::json!({ "path": "notes.txt" }),
            },
        ],
        "text before the boundary stays; the fallback is echoed in place"
    );

    let blocks = assistant_blocks(&events);
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[0].0["type"], "tool_use");
    assert_eq!(blocks[0].0["id"], "abandoned_1");
    assert!(blocks[0].1, "the pre-boundary call is abandoned");
    assert_eq!(blocks[1].0["type"], "fallback");
    assert!(!blocks[1].1);

    let rebuilt = rupu_agent::replay::reconstruct_messages(&events).expect("replay");
    assert_eq!(
        serde_json::to_value(&rebuilt).unwrap(),
        serde_json::to_value(&result.final_messages).unwrap(),
        "replay must rebuild exactly what the runner sent"
    );
}

/// A mid-output fallback with nothing after the boundary: the abandoned tool
/// call is not content, so the reply is an empty reply — nudged once at rung
/// 0, never dispatched, never sent back.
#[tokio::test]
async fn a_reply_with_only_an_abandoned_tool_call_is_an_empty_reply() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::Reply {
            content: vec![
                ContentBlock::ToolUse {
                    id: "abandoned_1".into(),
                    name: "write_file".into(),
                    input: serde_json::json!({ "path": "out.txt", "content": "x" }),
                },
                fallback_block(),
            ],
            stop: served_by_stop(StopReason::ToolUse),
            usage: Usage::default(),
        },
        reply(StopReason::EndTurn, vec![text("done")]),
    ]);
    let captured = provider.captured.clone();
    let transcript = tmp.path().join("run.jsonl");
    let opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    let result = run_agent(opts).await.expect("the loop completes");
    assert_eq!(result.status, RunStatus::Ok);
    let events = read_events(&transcript);

    assert!(!tmp.path().join("out.txt").exists());
    assert!(!events.iter().any(|e| matches!(e, Event::ToolCall { .. })));
    let classes: Vec<String> = outcomes(&events).into_iter().map(|o| o.class).collect();
    assert!(classes.contains(&"empty_reply".to_string()), "{classes:?}");
    assert!(
        recoveries(&events)
            .iter()
            .any(|(rung, action, ..)| *rung == 0 && *action == RecoveryAction::Continued),
        "the empty-reply nudge"
    );

    let requests = captured.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let last = requests[1].messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(last_text(last), Some(EMPTY_REPLY_NOTE));
    assert!(
        !requests[1]
            .messages
            .iter()
            .any(|m| m.role == Role::Assistant),
        "the empty reply is never sent back"
    );
    let rebuilt = rupu_agent::replay::reconstruct_messages(&events).expect("replay");
    assert_eq!(
        serde_json::to_value(&rebuilt).unwrap(),
        serde_json::to_value(&result.final_messages).unwrap()
    );
}

/// A turn a server-side fallback served from the start carries a leading
/// `fallback` block; an unrecognized block is kept verbatim. Both reach the
/// transcript in position, and replay rebuilds the conversation byte for
/// byte — the runner keeps both in its messages.
#[tokio::test]
async fn fallback_and_unknown_blocks_are_transcribed_and_replay_in_lockstep() {
    let unknown = ContentBlock::Unknown {
        provider: Some("anthropic".into()),
        raw: serde_json::json!({ "type": "lantern_note", "glow": 3 }),
    };
    let ran = run_script(
        vec![ScriptedTurn::Reply {
            content: vec![fallback_block(), unknown.clone(), text("the answer")],
            stop: served_by_stop(StopReason::EndTurn),
            usage: Usage::default(),
        }],
        "s1",
    )
    .await;
    assert_eq!(ran.result.status, RunStatus::Ok);
    let blocks = assistant_blocks(&ran.events);
    assert_eq!(
        blocks,
        vec![
            (serde_json::to_value(fallback_block()).unwrap(), false),
            (serde_json::to_value(&unknown).unwrap(), false),
        ]
    );
    // In position: both before the text.
    let first_text = ran
        .events
        .iter()
        .position(|e| matches!(e, Event::AssistantMessage { .. }))
        .unwrap();
    let last_block = ran
        .events
        .iter()
        .rposition(|e| matches!(e, Event::AssistantBlock { .. }))
        .unwrap();
    assert!(last_block < first_text);
    let last = ran.result.final_messages.last().unwrap();
    assert_eq!(
        last.content,
        vec![fallback_block(), unknown, text("the answer")]
    );
    assert_replay_lockstep(&ran);
}
