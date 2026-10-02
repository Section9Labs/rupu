//! Response outcomes in the turn loop (spec 2026-10-01 §5): every reply is
//! classified, rung 0 recovers in place, a run only succeeds on a normal or
//! warning stop, and replay rebuilds exactly what the runner sent.

use rupu_agent::recovery::{malformed_note, EMPTY_REPLY_NOTE, TRUNCATION_NOTE};
use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
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
