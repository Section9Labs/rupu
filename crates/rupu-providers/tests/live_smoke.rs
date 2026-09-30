//! Live smoke tests. Skipped silently unless RUPU_LIVE_TESTS=1 AND
//! per-provider credentials are present in the env.
//!
//! Run via: `RUPU_LIVE_TESTS=1 RUPU_LIVE_ANTHROPIC_KEY=... cargo test -p rupu-providers --test live_smoke`
//!
//! These tests are NOT run in the regular `cargo test` flow. They live
//! behind an env gate so the per-PR CI workflow stays offline; the
//! nightly workflow at `.github/workflows/nightly-live-tests.yml` runs
//! them with secrets.

use rupu_providers::auth::AuthCredentials;
use rupu_providers::types::{ContentBlock, LlmRequest, Message, Role, ToolDefinition, Usage};

fn live_enabled() -> bool {
    std::env::var("RUPU_LIVE_TESTS").as_deref() == Ok("1")
}

fn minimal_request(model: &str) -> LlmRequest {
    LlmRequest {
        model: model.into(),
        system: None,
        messages: vec![Message::user("Say hi.")],
        max_tokens: Some(64),
        tools: vec![],
        cell_id: None,
        trace_id: None,
        thinking: None,
        context_window: None,
        task_type: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        disable_prompt_cache: false,
    }
}

#[tokio::test]
async fn anthropic_live_round_trip() {
    if !live_enabled() {
        return;
    }
    let key = match std::env::var("RUPU_LIVE_ANTHROPIC_KEY") {
        Ok(k) => k,
        Err(_) => return,
    };
    let mut client =
        rupu_providers::AnthropicClient::new(key, std::sync::Arc::new(rupu_netflow::NullSink));
    let resp = client
        .send(&minimal_request("claude-haiku-4-5"))
        .await
        .expect("anthropic round-trip");
    assert!(!resp.content.is_empty(), "anthropic returned empty content");
    assert!(resp.usage.input_tokens > 0, "anthropic input_tokens == 0");
}

#[tokio::test]
async fn openai_live_round_trip() {
    if !live_enabled() {
        return;
    }
    let key = match std::env::var("RUPU_LIVE_OPENAI_KEY") {
        Ok(k) => k,
        Err(_) => return,
    };
    let creds = AuthCredentials::ApiKey { key };
    let mut client = rupu_providers::OpenAiCodexClient::new(
        creds,
        None,
        std::sync::Arc::new(rupu_netflow::NullSink),
    )
    .expect("init");
    let resp = client
        .send(&minimal_request("gpt-4o-mini"))
        .await
        .expect("openai round-trip");
    assert!(resp.usage.output_tokens > 0, "openai output_tokens == 0");
}

#[tokio::test]
async fn copilot_live_round_trip() {
    if !live_enabled() {
        return;
    }
    let token = match std::env::var("RUPU_LIVE_COPILOT_TOKEN") {
        Ok(t) => t,
        Err(_) => return,
    };
    let creds = AuthCredentials::ApiKey { key: token };
    let mut client = rupu_providers::GithubCopilotClient::new(
        creds,
        None,
        std::sync::Arc::new(rupu_netflow::NullSink),
    )
    .expect("init");
    let resp = client
        .send(&minimal_request("gpt-4o-mini"))
        .await
        .expect("copilot round-trip");
    assert!(resp.usage.output_tokens > 0, "copilot output_tokens == 0");
}

// ---------------------------------------------------------------------
// Anthropic prompt-cache live checks (Plan 3, Task 5).
//
// Both tests are `#[ignore]`d on top of the usual env gate, so a plain
// `cargo test` (or even the nightly run without `--ignored`) never spends
// money. Run them only after the user has explicitly approved the spend:
//
//   RUPU_LIVE_TESTS=1 RUPU_LIVE_ANTHROPIC_KEY=... \
//     cargo test -p rupu-providers --test live_smoke -- --ignored --nocapture prompt_cache
//   RUPU_LIVE_TESTS=1 RUPU_LIVE_ANTHROPIC_KEY=... \
//     cargo test -p rupu-providers --test live_smoke -- --ignored --nocapture empty_tool_result
//
// `--nocapture` matters: the usage numbers are printed with `eprintln!`, and a
// missing env var skips (returns early, reported as "ok") rather than fails —
// the same behaviour as the round-trip tests above.

/// Model for the prompt-cache checks. `claude-haiku-4-5` (used by the plain
/// round-trip test) needs a 4096-token prefix to cache at all, which the ~3k
/// filler below would silently miss; Sonnet 4.6's minimum cacheable prefix is
/// 1024 tokens. Override with `RUPU_LIVE_ANTHROPIC_MODEL`, minding the
/// model's minimum cacheable prefix:
/// - 512: Opus 5.5 / Opus 5, Fable, Sonnet 5.5;
/// - 1024: Opus 4.8, Sonnet 5, Sonnet 4.6 / 4.5, Opus 4.1 / 4;
/// - 2048: Opus 4.7;
/// - 4096: Opus 4.6 / 4.5, Haiku 4.5 (the ~3k filler is too short for these).
fn cache_test_model() -> String {
    std::env::var("RUPU_LIVE_ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-sonnet-4-6".into())
}

/// Deterministic invented filler of roughly 3,000 tokens (~34 numbered
/// ledger entries of ~90 tokens each). Numbering each entry keeps the text
/// from being one trivially repeated string; the same input always yields
/// byte-identical output so both requests share a cacheable prefix.
fn cacheable_system_prompt() -> String {
    let entry = "The lighthouse keeper at Marrowgate logged the tides each dusk: three \
        barrels of salt for the herring boats, one lantern wick trimmed, and a note about \
        the gull that had learned to open the tin biscuit box. Nothing about the harbour \
        ledger was urgent, but every entry was copied twice, once in ink for the office \
        and once in pencil for the wall.";
    let mut out = String::from(
        "You are a terse assistant. Below is a reference ledger; ignore it and answer \
         the user in one short word.\n\n",
    );
    for i in 1..=34 {
        out.push_str(&format!("Ledger entry {i}: {entry}\n"));
    }
    out
}

/// Builds a caching-ON (default) client from the live env pattern, or `None`
/// (skip) when the env gate / key is absent.
fn live_anthropic_client() -> Option<rupu_providers::AnthropicClient> {
    if !live_enabled() {
        eprintln!("SKIPPED: RUPU_LIVE_TESTS != 1");
        return None;
    }
    let Ok(key) = std::env::var("RUPU_LIVE_ANTHROPIC_KEY") else {
        eprintln!("SKIPPED: RUPU_LIVE_ANTHROPIC_KEY not set");
        return None;
    };
    // `AnthropicClient::new` leaves prompt caching at its default: ON.
    Some(rupu_providers::AnthropicClient::new(
        key,
        std::sync::Arc::new(rupu_netflow::NullSink),
    ))
}

fn assert_whole_prompt_usage(label: &str, usage: &Usage) {
    assert!(
        usage.input_tokens >= usage.cached_tokens + usage.cache_write_tokens,
        "{label}: normalized input_tokens ({}) must cover cache reads ({}) + cache writes ({})",
        usage.input_tokens,
        usage.cached_tokens,
        usage.cache_write_tokens,
    );
}

/// Live: the SAME streaming request twice must write (or already hit) the
/// cache the first time and read it the second.
///
/// Cost: two tiny requests (~3k input tokens each, `max_tokens = 16`) on
/// Sonnet 4.6 — a few cents. Requires explicit user approval before running;
/// see the run command in the block comment above.
#[tokio::test]
#[ignore = "live API: spends money; run with --ignored after approval"]
async fn live_anthropic_prompt_cache_reads_on_second_request() {
    let Some(mut client) = live_anthropic_client() else {
        return;
    };
    let mut req = minimal_request(&cache_test_model());
    req.system = Some(cacheable_system_prompt());
    req.messages = vec![Message::user("Say hi.")];
    req.max_tokens = Some(16);

    let first = client
        .stream(&req, |_| {})
        .await
        .expect("first streamed request");
    eprintln!("prompt_cache first  usage: {:?}", first.usage);
    let second = client
        .stream(&req, |_| {})
        .await
        .expect("second streamed request");
    eprintln!("prompt_cache second usage: {:?}", second.usage);

    // A warm cache from a run within the last 5 minutes is fine: the first
    // call then reads instead of writes.
    assert!(
        first.usage.cache_write_tokens > 0 || first.usage.cached_tokens > 0,
        "first request neither wrote nor read the cache (input_tokens = {}; is the prefix \
         below the model's minimum cacheable length?): {:?}",
        first.usage.input_tokens,
        first.usage,
    );
    assert!(
        second.usage.cached_tokens > 0,
        "second identical request did not read the cache: {:?}",
        second.usage,
    );
    assert_whole_prompt_usage("first", &first.usage);
    assert_whole_prompt_usage("second", &second.usage);
}

/// Live: a turn whose final message is an EMPTY `tool_result` (what every
/// silent `bash` call produces) succeeds with caching on. The request builder
/// treats an empty `tool_result` as uncacheable and walks the rolling
/// breakpoint back to the preceding assistant's `tool_use`, so this checks
/// that placement end to end — whether the API would accept `cache_control`
/// on the empty `tool_result` itself is left unverified by design.
///
/// Cost: one tiny streaming request (`max_tokens = 32`) — well under a cent.
/// Requires explicit user approval before running; see the run command in the
/// block comment above.
#[tokio::test]
#[ignore = "live API: spends money; run with --ignored after approval"]
async fn live_anthropic_turn_ending_in_empty_tool_result_succeeds_with_caching() {
    let Some(mut client) = live_anthropic_client() else {
        return;
    };
    let tool_use_id = "toolu_01cachechecknoop000001";
    let mut req = minimal_request(&cache_test_model());
    req.system = Some("You are a terse test harness. Reply with one short sentence.".into());
    req.tools = vec![ToolDefinition {
        name: "noop".into(),
        description: "Does nothing and returns nothing.".into(),
        input_schema: serde_json::json!({ "type": "object", "properties": {} }),
    }];
    req.messages = vec![
        Message::user("Call the noop tool once, then tell me it finished."),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: tool_use_id.into(),
                name: "noop".into(),
                input: serde_json::json!({}),
            }],
        },
        // Final user message ends with an EMPTY tool_result — not a marker
        // target, so the rolling breakpoint walks back onto the tool_use.
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: String::new(),
                is_error: false,
            }],
        },
    ];
    req.max_tokens = Some(32);

    let resp = client
        .stream(&req, |_| {})
        .await
        .expect("a turn ending in an empty tool_result must succeed with caching on");
    eprintln!("empty_tool_result usage: {:?}", resp.usage);
    assert_whole_prompt_usage("empty tool_result", &resp.usage);
}

// Gemini live test deferred until AI Studio API-key path is wired
// (see TODO.md). The Vertex/CLI OAuth path requires a project_id +
// service-account-style credential which doesn't fit the simple env-
// var-keyed pattern used here.
