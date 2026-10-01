//! `AgentRunOpts::limits` — the run consumes resolved `ModelLimits`: the
//! output cap goes on the wire, the run announces what it resolved, and an
//! overflow error teaches the run the real input limit (spec 2026-09-30
//! §6.3, §6.6, §7).

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, run_agent_with_limits, AgentRunOpts, RunError};
use rupu_providers::model_limits::{Limit, LimitSource, ModelLimits};
use rupu_providers::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, Role, StopReason, StreamEvent, Usage,
};
use rupu_providers::{LlmProvider, ProviderError, ProviderId};
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn usage(input: u32, output: u32, cached: u32) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cached_tokens: cached,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    }
}

fn final_text_turn(u: Usage) -> ScriptedTurn {
    ScriptedTurn::AssistantTextWithUsage {
        text: "all done".into(),
        stop: StopReason::EndTurn,
        usage: u,
    }
}

fn dense_msg(role: Role, label: &str) -> Message {
    Message {
        role,
        content: vec![ContentBlock::Text {
            text: format!("{label}: {}", "x".repeat(1000)),
        }],
    }
}

/// The compaction summariser's scripted reply; the marker lets a test tell a
/// post-compaction history from the original one.
const SUMMARY: &str = "SUMMARY-MARKER: condensed earlier work";

fn summary_turn(u: Usage) -> ScriptedTurn {
    ScriptedTurn::AssistantTextWithUsage {
        text: SUMMARY.into(),
        stop: StopReason::EndTurn,
        usage: u,
    }
}

/// Four dense messages: enough for `partition_for_compaction` to find a
/// middle to summarise (mirrors `runner_usage_hook.rs`).
fn dense_seed() -> Vec<Message> {
    vec![
        dense_msg(Role::User, "task"),
        dense_msg(Role::Assistant, "assistant 0"),
        dense_msg(Role::User, "user 0"),
        dense_msg(Role::Assistant, "assistant 1"),
    ]
}

fn message_text(m: &Message) -> String {
    m.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn compaction_count(path: &std::path::Path) -> usize {
    JsonlReader::iter(path)
        .unwrap()
        .filter(|e| matches!(e, Ok(Event::Compaction { .. })))
        .count()
}

/// After an overflow-triggered compaction the captured requests are: the
/// failed turn, the summariser call, the retried turn. The retry must carry
/// the compacted history, not the original one.
fn assert_retried_turn_carries_compacted_history(captured: &[LlmRequest]) {
    assert_eq!(
        captured.len(),
        3,
        "failed turn, summariser call, retried turn"
    );
    let (failed, retried) = (&captured[0], &captured[2]);
    assert!(
        !failed
            .messages
            .iter()
            .any(|m| message_text(m).contains(SUMMARY)),
        "the failed request predates the summary"
    );
    assert!(
        retried.messages.len() < failed.messages.len(),
        "retry has {} messages, failed request had {}",
        retried.messages.len(),
        failed.messages.len()
    );
    assert!(
        message_text(&retried.messages[0]).contains(SUMMARY),
        "the retried request starts from the summary-bearing task message"
    );
}

/// `runner_usage_hook.rs`'s opts construction, parameterised on the
/// provider; limits start out unknown.
fn build_opts(
    provider: Box<dyn LlmProvider>,
    tmp: &tempfile::TempDir,
    transcript_path: std::path::PathBuf,
) -> AgentRunOpts {
    AgentRunOpts {
        codename: None,
        seed_source: None,
        agent_name: "noop".into(),
        agent_system_prompt: "You are a noop agent.".into(),
        agent_tools: None,
        provider,
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_model_limits".into(),
        workspace_id: "ws_test1".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_path,
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: tmp.path().to_path_buf(),
            ..Default::default()
        },
        user_message: "say hi".into(),
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
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        on_usage: None,
        concerns: None,
        limits: ModelLimits::unknown(),
        scope_name: None,
        surface_tag: None,
        pause: None,
    }
}

fn notices(path: &std::path::Path) -> Vec<(String, String)> {
    JsonlReader::iter(path)
        .unwrap()
        .filter_map(|e| match e.ok()? {
            Event::Notice { kind, message } => Some((kind, message)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn resolved_output_cap_goes_on_the_wire_and_is_announced() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.limits = ModelLimits::fixed(1_000_000, 128_000);
    run_agent(opts).await.unwrap();
    assert_eq!(captured.lock().unwrap()[0].max_tokens, Some(128_000));
    let n = notices(&transcript);
    assert!(
        n.iter()
            .any(|(k, m)| k == "model_limits" && m.starts_with("input 1,000,000 · output 128,000")),
        "{n:?}"
    );
}

#[tokio::test]
async fn unknown_output_cap_is_not_sent() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let opts = build_opts(Box::new(provider), &tmp, tmp.path().join("run.jsonl"));
    run_agent(opts).await.unwrap();
    assert_eq!(captured.lock().unwrap()[0].max_tokens, None);
}

#[tokio::test]
async fn overflow_error_clamps_the_limit_and_compacts_instead_of_trimming() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::ProviderError("prompt is too long: 1500 tokens > 1000 maximum".into()),
        summary_turn(usage(100, 10, 0)), // the compaction summariser's send
        final_text_turn(usage(300, 6, 0)), // the retried turn
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = dense_seed();
    opts.limits = ModelLimits::unknown()
        .with_input(1_000_000)
        .with_percent(50);
    let result = run_agent(opts).await.unwrap();
    assert_eq!(result.final_limits.input.tokens, Some(1000));
    assert_eq!(result.final_limits.input.source, LimitSource::Observed);
    let n = notices(&transcript);
    assert!(
        n.iter()
            .any(|(k, m)| k == "model_limits_clamped" && m.contains("1,000,000 → 1,000")),
        "{n:?}"
    );
    assert!(
        !n.iter().any(|(k, _)| k == "context_trim"),
        "compaction, not trimming: {n:?}"
    );
    assert_eq!(compaction_count(&transcript), 1);
    assert_retried_turn_carries_compacted_history(&captured.lock().unwrap());
}

/// Spec §7: with the input limit known, an overflow whose message carries no
/// `max` still compacts with a summary (once) before the trim loop is
/// considered. The limit is not touched, so no clamp notice either.
///
/// The limit is 1000 rather than 1,000,000 so the seeded history has a middle
/// to summarise: compaction keeps roughly `threshold / 2` tokens verbatim,
/// which at 1M would cover the whole four-message seed.
#[tokio::test]
async fn overflow_without_a_parsed_max_still_compacts_when_the_limit_is_known() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::ProviderError("too many tokens in request".into()),
        summary_turn(usage(100, 10, 0)),
        final_text_turn(usage(300, 6, 0)),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = dense_seed();
    opts.limits = ModelLimits::unknown().with_input(1000).with_percent(50);
    let result = run_agent(opts).await.unwrap();

    assert_eq!(compaction_count(&transcript), 1);
    let n = notices(&transcript);
    assert!(
        !n.iter().any(|(k, _)| k == "context_trim"),
        "compaction, not trimming: {n:?}"
    );
    assert!(
        !n.iter().any(|(k, _)| k == "model_limits_clamped"),
        "no max parsed, so nothing to clamp: {n:?}"
    );
    assert_eq!(
        result.final_limits.input,
        Limit::new(1000, LimitSource::Agent),
        "the limit is unchanged"
    );
    assert_retried_turn_carries_compacted_history(&captured.lock().unwrap());
}

/// Same as above for a parsed `max` that is not below the current limit:
/// `clamp_input` leaves the limit alone, and the run still compacts.
#[tokio::test]
async fn overflow_with_a_max_that_does_not_lower_the_limit_still_compacts() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::ProviderError("prompt is too long: 1500 tokens > 2000 maximum".into()),
        summary_turn(usage(100, 10, 0)),
        final_text_turn(usage(300, 6, 0)),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = dense_seed();
    opts.limits = ModelLimits::unknown().with_input(1000).with_percent(50);
    let result = run_agent(opts).await.unwrap();

    assert_eq!(compaction_count(&transcript), 1);
    let n = notices(&transcript);
    assert!(
        !n.iter().any(|(k, _)| k == "context_trim"),
        "compaction, not trimming: {n:?}"
    );
    assert!(
        !n.iter().any(|(k, _)| k == "model_limits_clamped"),
        "2000 is above the current 1000, so the limit is not clamped: {n:?}"
    );
    assert_eq!(
        result.final_limits.input,
        Limit::new(1000, LimitSource::Agent)
    );
    assert_retried_turn_carries_compacted_history(&captured.lock().unwrap());
}

/// A provider that can only stream: `send` fails loudly, `stream` emits a few
/// events and returns a complete response. Every streamed request is captured.
struct StreamOnlyProvider {
    captured: Arc<Mutex<Vec<LlmRequest>>>,
}

#[async_trait::async_trait]
impl LlmProvider for StreamOnlyProvider {
    async fn send(&mut self, _req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        Err(ProviderError::Other(anyhow::anyhow!(
            "send() must not be used: requests stream on the wire"
        )))
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.captured.lock().unwrap().push(req.clone());
        on_event(StreamEvent::TextDelta("hel".into()));
        on_event(StreamEvent::TextDelta("lo".into()));
        on_event(StreamEvent::UsageSnapshot(usage(5, 2, 0)));
        Ok(LlmResponse {
            id: "stream-only".into(),
            model: "mock-1".into(),
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
            stop_reason: Some(StopReason::EndTurn),
            usage: usage(5, 2, 0),
        })
    }

    fn default_model(&self) -> &str {
        "mock-1"
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

fn discovered(input: u32, output: u32, src: LimitSource) -> ModelLimits {
    let mut l = ModelLimits::unknown();
    l.input = Limit::new(input, src.clone());
    l.output = Limit::new(output, src);
    l
}

fn assistant_delta_count(path: &std::path::Path) -> usize {
    JsonlReader::iter(path)
        .unwrap()
        .filter(|e| matches!(e, Ok(Event::AssistantDelta { .. })))
        .count()
}

/// Run one turn against a [`StreamOnlyProvider`]; returns the captured
/// requests, the transcript path, the forwarded-event count and the run's
/// `model_limits` notice.
async fn run_stream_only(
    limits: ModelLimits,
    no_stream: bool,
    tmp: &tempfile::TempDir,
) -> (Vec<LlmRequest>, std::path::PathBuf, usize, String) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = StreamOnlyProvider {
        captured: captured.clone(),
    };
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), tmp, transcript.clone());
    opts.no_stream = no_stream;
    opts.suppress_stream_stdout = true;
    opts.limits = limits;
    let forwarded = Arc::new(AtomicUsize::new(0));
    let counter = forwarded.clone();
    opts.on_stream_event = Some(Arc::new(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    run_agent(opts).await.expect("run completes");
    let notice = notices(&transcript)
        .into_iter()
        .find(|(k, _)| k == "model_limits")
        .map(|(_, m)| m)
        .expect("model_limits notice");
    let reqs = captured.lock().unwrap().clone();
    (reqs, transcript, forwarded.load(Ordering::SeqCst), notice)
}

/// `no_stream` only changes display: the request still streams on the wire (so
/// a long generation cannot hit the HTTP timeout) and carries the FULL
/// discovered output cap — nothing is capped to dodge a timeout. The
/// transcript keeps its non-streaming shape (no `AssistantDelta`), and the
/// stream-event callback still sees every event.
#[tokio::test]
async fn no_stream_streams_under_the_hood_and_sends_the_full_output_cap() {
    for src in [
        LimitSource::Config,
        LimitSource::Live {
            fetched_at: chrono::Utc::now(),
            stale: false,
        },
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (reqs, transcript, forwarded, notice) =
            run_stream_only(discovered(1_000_000, 128_000, src.clone()), true, &tmp).await;
        assert_eq!(reqs.len(), 1, "one streamed request, source {src:?}");
        assert_eq!(reqs[0].max_tokens, Some(128_000), "source {src:?}");
        assert_eq!(
            assistant_delta_count(&transcript),
            0,
            "a --no-stream transcript has no AssistantDelta events"
        );
        assert_eq!(forwarded, 3, "the quiet sink still forwards every event");
        assert!(
            !notice.contains("capped"),
            "nothing is capped for non-streaming runs: {notice}"
        );
        assert!(notice.contains("output 128,000"), "{notice}");
    }
}

/// Control for the test above: a streaming run over the same provider DOES
/// write `AssistantDelta` events, so the zero count above is meaningful.
#[tokio::test]
async fn streaming_run_writes_assistant_deltas() {
    let tmp = tempfile::tempdir().unwrap();
    let (reqs, transcript, forwarded, _) = run_stream_only(
        discovered(1_000_000, 128_000, LimitSource::Config),
        false,
        &tmp,
    )
    .await;
    assert_eq!(reqs[0].max_tokens, Some(128_000));
    assert_eq!(assistant_delta_count(&transcript), 2, "\"hel\" + \"lo\"");
    assert_eq!(forwarded, 3);
}

/// Spec §6.5: a limit learned from an overflow must survive a run that ends
/// in an error, or a session whose turn clamps and then fails never persists
/// it. One message is too short to compact or trim, so the clamp is followed
/// by `Err(ContextOverflow)`; the run's final limits still come back.
#[tokio::test]
async fn run_agent_with_limits_returns_the_learned_limit_on_an_error_exit() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::ProviderError(
        "prompt is too long: 250000 tokens > 200000 maximum".into(),
    )]);
    let tmp = tempfile::tempdir().unwrap();
    let mut opts = build_opts(Box::new(provider), &tmp, tmp.path().join("run.jsonl"));
    opts.limits = ModelLimits::unknown().with_input(1_000_000);
    let (result, limits) = run_agent_with_limits(opts).await;
    assert!(
        matches!(result, Err(RunError::ContextOverflow { .. })),
        "the run must end in an error for this test to mean anything"
    );
    assert_eq!(limits.input, Limit::new(200_000, LimitSource::Observed));
}

/// The success path returns the same limits `RunResult.final_limits` carries.
#[tokio::test]
async fn run_agent_with_limits_returns_the_final_limits_on_success() {
    let provider = CapturingMockProvider::new(vec![final_text_turn(usage(1, 1, 0))]);
    let tmp = tempfile::tempdir().unwrap();
    let mut opts = build_opts(Box::new(provider), &tmp, tmp.path().join("run.jsonl"));
    opts.limits = ModelLimits::fixed(300_000, 4_000);
    let (result, limits) = run_agent_with_limits(opts).await;
    let result = result.expect("run completes");
    assert_eq!(limits, result.final_limits);
    assert_eq!(limits, ModelLimits::fixed(300_000, 4_000));
}

/// Sum of the text lengths across `messages` (the runner's `message_chars`
/// for text-only messages).
fn text_chars(messages: &[Message]) -> usize {
    messages.iter().map(|m| message_text(m).len()).sum()
}

/// A 15-message conversation for proactive-compaction sizing: a 19,998-char
/// task, 13 alternating 10,000-char messages, and the 2-char "go" prompt the
/// run appends — 150,000 chars, so a turn billed at 150,000 input tokens
/// calibrates to exactly one token per char.
fn sizing_seed() -> Vec<Message> {
    let mut seed = vec![Message::user(&"t".repeat(19_998))];
    for i in 0..13 {
        let role = if i % 2 == 0 {
            Role::Assistant
        } else {
            Role::User
        };
        seed.push(Message {
            role,
            content: vec![ContentBlock::Text {
                text: "x".repeat(10_000),
            }],
        });
    }
    seed
}

/// Run one proactively-compacting turn over [`sizing_seed`] with `limits`;
/// returns the captured requests and the run's final messages.
async fn run_sizing_turn(limits: ModelLimits) -> (Vec<LlmRequest>, Vec<Message>) {
    let provider = CapturingMockProvider::new(vec![
        final_text_turn(usage(150_000, 8, 0)),
        summary_turn(usage(100, 10, 0)),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let mut opts = build_opts(Box::new(provider), &tmp, tmp.path().join("run.jsonl"));
    opts.initial_messages = sizing_seed();
    opts.user_message = "go".into();
    opts.limits = limits;
    let result = run_agent(opts).await.expect("run completes");
    let reqs = captured.lock().unwrap().clone();
    assert_eq!(
        text_chars(&reqs[0].messages),
        150_000,
        "fixture calibration"
    );
    (reqs, result.final_messages)
}

/// Spec §6.4: compaction sizes the verbatim recent history from the SAME
/// threshold that triggered it — `min(input × pct, input − output)` — not
/// from `input × pct` alone. 200,000 input / 64,000 output → threshold
/// 136,000: the recent budget is half of it net of the 19,998-token task and
/// the 8,192-token summary cap, 68,000 − 28,190 = 39,810 tokens (= chars
/// here). The last four messages (30,002 chars) stay verbatim and eleven are
/// summarised. A 160,000-based budget (51,810 net) would keep five and
/// summarise ten.
#[tokio::test]
async fn compaction_budget_follows_the_headroom_threshold() {
    let limits = discovered(
        200_000,
        64_000,
        LimitSource::Live {
            fetched_at: chrono::Utc::now(),
            stale: false,
        },
    );
    assert_eq!(limits.compact_threshold(), Some(136_000));
    let (reqs, _) = run_sizing_turn(limits).await;
    assert_eq!(reqs.len(), 2, "the turn, then the summariser call");
    assert_eq!(
        reqs[1].messages.len(),
        11,
        "task + ten messages summarised; four recent kept verbatim"
    );
}

/// The point of sizing from the threshold: the compacted history must land
/// under it, or the next turn compacts again (and again). 200,000 input /
/// 150,000 output → threshold 50,000. A budget from `input × pct` (80,000)
/// would keep ~70,000 tokens of recent history verbatim — over the threshold
/// before the next turn even starts.
#[tokio::test]
async fn compacted_history_lands_under_the_threshold() {
    let limits = discovered(
        200_000,
        150_000,
        LimitSource::Live {
            fetched_at: chrono::Utc::now(),
            stale: false,
        },
    );
    let threshold = limits.compact_threshold().unwrap();
    assert_eq!(threshold, 50_000);
    let (_, final_messages) = run_sizing_turn(limits).await;
    assert!(
        message_text(&final_messages[0]).contains(SUMMARY),
        "the history was compacted"
    );
    // One token per char (the fixture's calibration).
    let estimated = text_chars(&final_messages) as u64;
    assert!(
        estimated < threshold,
        "post-compaction history ({estimated} tokens) must stay under the {threshold} threshold"
    );
}

/// Anthropic pre-4.5 validation error (wording verified against real API
/// responses quoted in anthropics/claude-code#42 and #228; numbers invented).
const INPUT_PLUS_MAX_TOKENS: &str = "API error 400: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"input length and `max_tokens` exceed context limit: 183500 + 20000 > 201000, decrease input length or `max_tokens` and try again\"}}";

/// `input + max_tokens > window`: the input fits, the output reservation does
/// not. The turn is retried ONCE with `max_tokens = window − input − 1000`;
/// the input limit is not clamped (it never overflowed), and nothing is
/// compacted or trimmed.
#[tokio::test]
async fn input_plus_max_tokens_overflow_retries_with_a_lowered_output_cap() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::ProviderError(INPUT_PLUS_MAX_TOKENS.into()),
        final_text_turn(usage(300, 6, 0)),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.limits = ModelLimits::fixed(1_000_000, 20_000);
    let result = run_agent(opts).await.expect("the retry succeeds");

    let reqs = captured.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2, "the failed turn and its one retry");
    assert_eq!(reqs[0].max_tokens, Some(20_000));
    assert_eq!(
        reqs[1].max_tokens,
        Some(201_000 - 183_500 - 1_000),
        "the retried request carries the lowered cap"
    );
    assert_eq!(
        serde_json::to_value(&reqs[1].messages).unwrap(),
        serde_json::to_value(&reqs[0].messages).unwrap(),
        "same history otherwise"
    );
    assert_eq!(
        result.final_limits,
        ModelLimits::fixed(1_000_000, 20_000),
        "neither limit is clamped: the lowering is for this request only"
    );
    let n = notices(&transcript);
    assert!(
        n.iter().any(|(k, m)| k == "model_limits_clamped"
            && m.contains("output 20,000 → 16,500")
            && m.contains("this request")),
        "{n:?}"
    );
    assert!(!n.iter().any(|(k, _)| k == "context_trim"), "{n:?}");
    assert_eq!(compaction_count(&transcript), 0);
}

/// When `window − input − 1000` is under the 1024 floor there is no useful
/// cap to lower to: the error falls through to compaction (the input is what
/// has to shrink). The input limit is still not clamped — the window in this
/// error counts output too.
#[tokio::test]
async fn input_plus_max_tokens_below_the_floor_compacts_instead() {
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::ProviderError(
            "API error 400: input length and `max_tokens` exceed context limit: 200000 + 8192 > 201000"
                .into(),
        ),
        summary_turn(usage(100, 10, 0)),
        final_text_turn(usage(300, 6, 0)),
    ]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = dense_seed();
    opts.limits = ModelLimits::unknown().with_input(1000).with_percent(50);
    let result = run_agent(opts).await.expect("compaction recovers");

    assert_eq!(compaction_count(&transcript), 1);
    let n = notices(&transcript);
    assert!(
        !n.iter().any(|(k, _)| k == "model_limits_clamped"),
        "nothing lowered, nothing clamped: {n:?}"
    );
    assert_eq!(
        result.final_limits.input,
        Limit::new(1000, LimitSource::Agent)
    );
    assert_retried_turn_carries_compacted_history(&captured.lock().unwrap());
}

/// A provider that fails its first streamed call with `err`, then delegates.
struct FailFirst {
    err: Option<ProviderError>,
    inner: CapturingMockProvider,
}

#[async_trait::async_trait]
impl LlmProvider for FailFirst {
    async fn send(&mut self, req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.inner.send(req).await
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        match self.err.take() {
            Some(e) => Err(e),
            None => self.inner.stream(req, on_event).await,
        }
    }

    fn default_model(&self) -> &str {
        "mock-1"
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

/// The client's `LongContextUnavailable`: a request carrying the 1M beta was
/// refused with Anthropic's extra-usage 429, so this account has no
/// entitlement for 1M context, and the client has stopped sending the beta.
fn long_context_unavailable() -> ProviderError {
    ProviderError::LongContextUnavailable {
        message: "Extra usage is required for long context requests".into(),
    }
}

fn live_1m_limits() -> ModelLimits {
    discovered(
        1_000_000,
        128_000,
        LimitSource::Live {
            fetched_at: chrono::Utc::now(),
            stale: false,
        },
    )
    .with_percent(50)
}

/// On `LongContextUnavailable` the run falls back to the standard window:
/// it clamps the input limit to 200,000 (`Observed`), says why, and retries
/// the turn once — the retry goes out without the beta. It is neither
/// treated as a rate limit nor compacted for (nothing overflowed).
#[tokio::test]
async fn long_context_unavailable_clamps_to_200k_and_retries_once() {
    let inner = CapturingMockProvider::new(vec![final_text_turn(usage(300, 6, 0))]);
    let captured = inner.captured.clone();
    let provider = FailFirst {
        err: Some(long_context_unavailable()),
        inner,
    };
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.limits = live_1m_limits();
    let result = run_agent(opts).await.expect("the retry succeeds");

    assert_eq!(
        result.final_limits.input,
        Limit::new(200_000, LimitSource::Observed)
    );
    let n = notices(&transcript);
    assert!(
        n.iter().any(|(k, m)| k == "model_limits_clamped"
            && m.contains("no extra-usage entitlement for 1M context")
            && m.contains("1M beta disabled")
            && m.contains("1,000,000 → 200,000")
            && m.contains(LONG_CONTEXT_HINT)),
        "{n:?}"
    );
    assert!(
        !n.iter().any(|(k, _)| k == "provider_retry"),
        "not retried as a rate limit: {n:?}"
    );
    assert_eq!(compaction_count(&transcript), 0);
    assert_eq!(captured.lock().unwrap().len(), 1, "exactly one retry");
}

/// How to stop paying the refused request every run.
const LONG_CONTEXT_HINT: &str =
    "remove `[1m]` from the model / `contextWindow: 1m` from the agent to skip this";

/// A session's later turns already carry the clamped 200K input limit (it
/// was persisted), so their notice has no "input X → Y" part — but every
/// run still sends one refused request, and the notice must still say how
/// to stop it.
#[tokio::test]
async fn long_context_unavailable_notice_keeps_its_hint_when_already_clamped() {
    let inner = CapturingMockProvider::new(vec![final_text_turn(usage(300, 6, 0))]);
    let provider = FailFirst {
        err: Some(long_context_unavailable()),
        inner,
    };
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    let mut limits = live_1m_limits();
    limits.input = Limit::new(200_000, LimitSource::Observed);
    opts.limits = limits;
    run_agent(opts).await.expect("the retry succeeds");
    let n = notices(&transcript);
    let clamped: Vec<&String> = n
        .iter()
        .filter(|(k, _)| k == "model_limits_clamped")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(clamped.len(), 1, "{n:?}");
    assert!(
        !clamped[0].contains(" → "),
        "nothing newly clamped: {}",
        clamped[0]
    );
    assert!(clamped[0].contains(LONG_CONTEXT_HINT), "{}", clamped[0]);
}

/// If the retried turn (now without the beta) still overflows the 200K
/// window, the server answers `prompt is too long` and the existing overflow
/// handling compacts.
#[tokio::test]
async fn long_context_unavailable_then_overflow_reaches_compaction() {
    let big = |role: Role, label: &str| Message {
        role,
        content: vec![ContentBlock::Text {
            text: format!("{label}: {}", "x".repeat(100_000)),
        }],
    };
    let inner = CapturingMockProvider::new(vec![
        ScriptedTurn::ProviderError("prompt is too long: 250000 tokens > 200000 maximum".into()),
        summary_turn(usage(100, 10, 0)),
        final_text_turn(usage(300, 6, 0)),
    ]);
    let captured = inner.captured.clone();
    let provider = FailFirst {
        err: Some(long_context_unavailable()),
        inner,
    };
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = vec![
        big(Role::User, "task"),
        big(Role::Assistant, "a0"),
        big(Role::User, "u0"),
        big(Role::Assistant, "a1"),
    ];
    opts.limits = live_1m_limits();
    let result = run_agent(opts).await.expect("compaction recovers");

    assert_eq!(
        result.final_limits.input,
        Limit::new(200_000, LimitSource::Observed)
    );
    assert_eq!(compaction_count(&transcript), 1);
    assert!(!notices(&transcript)
        .iter()
        .any(|(k, _)| k == "context_trim"));
    assert_retried_turn_carries_compacted_history(&captured.lock().unwrap());
}

/// `Compaction.seq` numbers the compactions that actually ran, with no gaps:
/// an attempt that does not compact (here the summariser call fails) must not
/// consume a number.
#[tokio::test]
async fn compaction_seq_counts_only_compactions_that_ran() {
    let provider = CapturingMockProvider::new(vec![
        // Turn 1: over the 500-token threshold, and a tool call so the run
        // continues.
        ScriptedTurn::AssistantBlocksWithUsage {
            content: vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "no_such_tool".into(),
                input: serde_json::json!({}),
            }],
            stop: StopReason::ToolUse,
            usage: usage(600, 5, 0),
        },
        // Its compaction attempt: the summariser fails, nothing compacts.
        ScriptedTurn::ProviderError("summariser unavailable".into()),
        // Turn 2: over the threshold again; this compaction runs.
        final_text_turn(usage(600, 5, 0)),
        summary_turn(usage(100, 10, 0)),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = dense_seed();
    opts.limits = ModelLimits::unknown().with_input(1000).with_percent(50);
    run_agent(opts).await.expect("run completes");

    let seqs: Vec<u32> = JsonlReader::iter(&transcript)
        .unwrap()
        .filter_map(|e| match e.ok()? {
            Event::Compaction { seq, .. } => Some(seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, vec![1], "the first compaction that ran is #1");
}

/// Anthropic's extra-usage 429 on a request WITHOUT the 1M beta (so the
/// client surfaces the plain 429, not `LongContextUnavailable`) is refused
/// the same way on every attempt: the runner must not burn its transient
/// retries (with backoff) on it. One attempt, then the error.
#[tokio::test]
async fn a_plain_extra_usage_429_is_not_retried() {
    let inner = CapturingMockProvider::new(vec![final_text_turn(usage(1, 1, 0))]);
    let captured = inner.captured.clone();
    let provider = FailFirst {
        err: Some(ProviderError::Api {
            status: 429,
            message: r#"{"error":{"message":"Extra usage is required for long context requests"}}"#
                .into(),
        }),
        inner,
    };
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    let result = run_agent(opts).await;
    assert!(
        matches!(&result, Err(RunError::Provider(m)) if m.contains("Extra usage")),
        "surfaces as an error: {:?}",
        result.as_ref().err()
    );
    assert_eq!(captured.lock().unwrap().len(), 0, "never retried");
    assert!(!notices(&transcript)
        .iter()
        .any(|(k, _)| k == "provider_retry"));
}
