//! `AgentRunOpts::on_usage` — the per-LLM-call usage hook (live usage ledger
//! spec 2026-09-29 §3.3). The hook must see exactly what the transcript's
//! `Usage` events record, for normal turns AND the compaction summariser.

use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts, UsageKind, UsageTurn};
use rupu_providers::types::{ContentBlock, Message, Role, StopReason, Usage};
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader};
use std::path::Path;
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

/// A scripted assistant turn that calls `read_file` on `path`, carrying `u`.
fn read_file_turn(path: &Path, u: Usage) -> ScriptedTurn {
    ScriptedTurn::AssistantBlocksWithUsage {
        content: vec![ContentBlock::ToolUse {
            id: "call_read_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({ "path": path.to_str().unwrap() }),
        }],
        stop: StopReason::ToolUse,
        usage: u,
    }
}

fn final_text_turn(u: Usage) -> ScriptedTurn {
    ScriptedTurn::AssistantTextWithUsage {
        text: "all done".into(),
        stop: StopReason::EndTurn,
        usage: u,
    }
}

/// `runner_basic.rs`'s opts construction, parameterised on the provider.
fn build_opts(
    provider: MockProvider,
    tmp: &assert_fs::TempDir,
    transcript_path: std::path::PathBuf,
) -> AgentRunOpts {
    AgentRunOpts {
        seed_source: None,
        agent_name: "noop".into(),
        agent_system_prompt: "You are a noop agent.".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_usage_hook".into(),
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
        max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
        scope_name: None,
        surface_tag: None,
        context_window_tokens: None,
        compact_at_percent: None,
        pause: None,
    }
}

/// `(input, output, cached, purpose)` of every transcript `Usage` event, in order.
fn transcript_usages(path: &Path) -> Vec<(u32, u32, u32, Option<String>)> {
    JsonlReader::iter(path)
        .unwrap()
        .flatten()
        .filter_map(|e| match e {
            Event::Usage {
                input_tokens,
                output_tokens,
                cached_tokens,
                purpose,
                ..
            } => Some((input_tokens, output_tokens, cached_tokens, purpose)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn on_usage_fires_once_per_turn_with_transcript_values() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let target = tmp.path().join("notes.txt");
    std::fs::write(&target, "invented file contents").unwrap();
    let transcript = tmp.path().join("run.jsonl");

    // A tool_use turn, then a final text turn.
    let provider = MockProvider::new(vec![
        read_file_turn(&target, usage(100, 10, 5)),
        final_text_turn(usage(200, 20, 7)),
    ]);

    let seen: Arc<Mutex<Vec<UsageTurn>>> = Default::default();
    let seen2 = seen.clone();
    let mut opts = build_opts(provider, &tmp, transcript.clone());
    opts.on_usage = Some(Arc::new(move |u: &UsageTurn| {
        seen2.lock().unwrap().push(u.clone());
    }));
    let rr = run_agent(opts).await.unwrap();

    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|u| u.kind == UsageKind::Turn));
    assert!(got
        .iter()
        .all(|u| u.provider == "mock" && u.model == "mock-1"));
    assert_eq!(got[0].input_tokens, 100);
    assert_eq!(got[0].cached_tokens, 5);
    assert_eq!(got[1].output_tokens, 20);
    assert_eq!(rr.total_tokens_cached, 12);
    assert_eq!(rr.total_tokens_in, 300);
    assert_eq!(rr.total_tokens_out, 30);

    // Hook values == the transcript's Usage events, in order.
    assert_eq!(
        transcript_usages(&transcript),
        vec![(100, 10, 5, None), (200, 20, 7, None)]
    );
}

#[tokio::test]
async fn on_usage_reports_billable_output_including_reasoning() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let provider = MockProvider::new(vec![ScriptedTurn::AssistantTextWithUsage {
        text: "thought about it".into(),
        stop: StopReason::EndTurn,
        usage: Usage {
            input_tokens: 40,
            output_tokens: 6,
            cached_tokens: 3,
            cache_write_tokens: 0,
            reasoning_tokens: 9,
        },
    }]);

    let seen: Arc<Mutex<Vec<UsageTurn>>> = Default::default();
    let seen2 = seen.clone();
    let mut opts = build_opts(provider, &tmp, transcript.clone());
    opts.on_usage = Some(Arc::new(move |u: &UsageTurn| {
        seen2.lock().unwrap().push(u.clone());
    }));
    run_agent(opts).await.unwrap();

    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    // output (6) + reasoning (9): identical to what the transcript records.
    assert_eq!(got[0].output_tokens, 15);
    assert_eq!(transcript_usages(&transcript), vec![(40, 15, 3, None)]);
}

fn dense_msg(role: Role, label: &str) -> Message {
    Message {
        role,
        content: vec![ContentBlock::Text {
            text: format!("{label}: {}", "x".repeat(1000)),
        }],
    }
}

#[tokio::test]
async fn compaction_call_emits_usage_with_purpose_and_hook_kind() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let target = tmp.path().join("notes.txt");
    std::fs::write(&target, "invented file contents").unwrap();
    let transcript = tmp.path().join("run.jsonl");

    // Script order: turn 1 (input 900 > the 500-token compaction threshold),
    // then the compaction summariser's own `send`, then turn 2.
    let provider = MockProvider::new(vec![
        read_file_turn(&target, usage(900, 10, 0)),
        final_text_turn_summary(usage(111, 22, 3)),
        final_text_turn(usage(60, 6, 0)),
    ]);

    let seen: Arc<Mutex<Vec<UsageTurn>>> = Default::default();
    let seen2 = seen.clone();
    let mut opts = build_opts(provider, &tmp, transcript.clone());
    // Enough seeded history for `partition_for_compaction` to find a middle.
    opts.initial_messages = vec![
        dense_msg(Role::User, "task"),
        dense_msg(Role::Assistant, "assistant 0"),
        dense_msg(Role::User, "user 0"),
        dense_msg(Role::Assistant, "assistant 1"),
    ];
    opts.context_window_tokens = Some(1000);
    opts.compact_at_percent = Some(50);
    opts.on_usage = Some(Arc::new(move |u: &UsageTurn| {
        seen2.lock().unwrap().push(u.clone());
    }));
    let rr = run_agent(opts).await.unwrap();

    let got = seen.lock().unwrap().clone();
    let kinds: Vec<UsageKind> = got.iter().map(|u| u.kind).collect();
    assert_eq!(
        kinds,
        vec![UsageKind::Turn, UsageKind::Compaction, UsageKind::Turn]
    );
    let comp = &got[1];
    assert_eq!(
        (comp.input_tokens, comp.output_tokens, comp.cached_tokens),
        (111, 22, 3)
    );
    assert_eq!(
        (comp.provider.as_str(), comp.model.as_str()),
        ("mock", "mock-1")
    );

    // The compaction spend is in the transcript, tagged with its purpose.
    assert_eq!(
        transcript_usages(&transcript),
        vec![
            (900, 10, 0, None),
            (111, 22, 3, Some("compaction".to_string())),
            (60, 6, 0, None),
        ]
    );

    // Compaction tokens do NOT leak into the turn-scoped run totals.
    assert_eq!(rr.total_tokens_in, 960);
    assert_eq!(rr.total_tokens_out, 16);
    assert_eq!(rr.total_tokens_cached, 0);
}

/// The summariser's scripted reply (a plain text turn carrying `u`).
fn final_text_turn_summary(u: Usage) -> ScriptedTurn {
    ScriptedTurn::AssistantTextWithUsage {
        text: "Summary of prior work.".into(),
        stop: StopReason::EndTurn,
        usage: u,
    }
}

/// `cache_write_tokens` of every transcript `Usage` event, in order.
fn transcript_cache_writes(path: &Path) -> Vec<u32> {
    JsonlReader::iter(path)
        .unwrap()
        .flatten()
        .filter_map(|e| match e {
            Event::Usage {
                cache_write_tokens, ..
            } => Some(cache_write_tokens),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn cache_write_tokens_reach_the_hook_and_transcript_for_turns_and_compaction() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let target = tmp.path().join("notes.txt");
    std::fs::write(&target, "invented file contents").unwrap();
    let transcript = tmp.path().join("run.jsonl");

    let with_write = |input: u32, write: u32| Usage {
        cache_write_tokens: write,
        ..usage(input, 5, 0)
    };
    // Turn 1 (900 > the 500-token threshold), the compaction summariser's
    // call, then turn 2 — each writing a different number of cache tokens.
    let provider = MockProvider::new(vec![
        read_file_turn(&target, with_write(900, 30)),
        final_text_turn_summary(with_write(111, 11)),
        final_text_turn(with_write(60, 0)),
    ]);

    let seen: Arc<Mutex<Vec<UsageTurn>>> = Default::default();
    let seen2 = seen.clone();
    let mut opts = build_opts(provider, &tmp, transcript.clone());
    opts.initial_messages = vec![
        dense_msg(Role::User, "task"),
        dense_msg(Role::Assistant, "assistant 0"),
        dense_msg(Role::User, "user 0"),
        dense_msg(Role::Assistant, "assistant 1"),
    ];
    opts.context_window_tokens = Some(1000);
    opts.compact_at_percent = Some(50);
    opts.on_usage = Some(Arc::new(move |u: &UsageTurn| {
        seen2.lock().unwrap().push(u.clone());
    }));
    run_agent(opts).await.unwrap();

    let got: Vec<(UsageKind, u64)> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|u| (u.kind, u.cache_write_tokens))
        .collect();
    assert_eq!(
        got,
        vec![
            (UsageKind::Turn, 30),
            (UsageKind::Compaction, 11),
            (UsageKind::Turn, 0),
        ]
    );
    assert_eq!(transcript_cache_writes(&transcript), vec![30, 11, 0]);
}
