//! `AgentRunOpts::limits` — the run consumes resolved `ModelLimits`: the
//! output cap goes on the wire, the run announces what it resolved, and an
//! overflow error teaches the run the real input limit (spec 2026-09-30
//! §6.3, §6.6, §7).

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_providers::model_limits::{Limit, LimitSource, ModelLimits};
use rupu_providers::types::{ContentBlock, LlmRequest, Message, Role, StopReason, Usage};
use rupu_providers::LlmProvider;
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader};
use std::sync::Arc;

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
