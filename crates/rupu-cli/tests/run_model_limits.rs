//! `rupu run` discovers the model's real limits and sends them (spec
//! 2026-09-30 §6.1). The httpmock server plays Anthropic: it serves the model
//! listing (with `max_tokens`) and accepts the messages request only when it
//! carries the expected cap. The runs here pass `--no-stream`, so a discovered
//! output cap above `NON_STREAMING_MAX_TOKENS` (16,384) goes out clamped.

use assert_cmd::Command;
use httpmock::prelude::*;

/// Run `rupu run probe --no-stream` against a mocked Anthropic that lists the
/// model with `discovered_max` output tokens and only accepts a messages
/// request carrying `wire_max_tokens`. Returns the run's transcript text.
fn run_probe(discovered_max: u32, wire_max_tokens: u32) -> String {
    let server = MockServer::start();
    let list = server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{
                "id": "claude-test-1",
                "type": "model",
                "max_input_tokens": 300000,
                "max_tokens": discovered_max
            }],
            "has_more": false
        }));
    });
    let msgs = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains(format!("\"max_tokens\":{wire_max_tokens}"));
        then.status(200).json_body(serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-test-1",
            "content": [{ "type": "text", "text": "ok" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 5, "output_tokens": 1 }
        }));
    });

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let agents = dir.path().join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("probe.md"),
        "---\nname: probe\nprovider: anthropic\nmodel: claude-test-1\n---\nSay ok.\n",
    )
    .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_HOME", &home)
        .env("RUPU_CACHE_DIR_OVERRIDE", home.join("cache/models"))
        .env(
            "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
            format!("{}/v1/messages", server.url("")),
        )
        .env("RUPU_ANTHROPIC_API_KEY", "sk-test")
        .args(["run", "probe", "--mode", "bypass", "--no-stream", "go"])
        .assert()
        .success();

    list.assert();
    msgs.assert();

    std::fs::read_dir(home.join("transcripts"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>()
}

#[test]
fn rupu_run_sends_the_discovered_output_cap_and_announces_limits() {
    // 12,000 is under the non-streaming ceiling, so it goes out as discovered.
    let transcripts = run_probe(12_000, 12_000);

    // The run-start notice states what was resolved and where it came from.
    assert!(
        transcripts.contains("\"kind\":\"model_limits\""),
        "no model_limits notice in the transcript: {transcripts}"
    );
    assert!(
        transcripts.contains("input 300,000 · output 12,000"),
        "notice should state the discovered limits: {transcripts}"
    );
    assert!(
        transcripts.contains("anthropic model list"),
        "notice should name the limit source: {transcripts}"
    );
    assert!(
        !transcripts.contains("for non-streaming requests"),
        "no cap applied, so the notice must not claim one: {transcripts}"
    );
}

#[test]
fn rupu_run_no_stream_clamps_a_large_discovered_output_cap() {
    // 50,000 is discovered, but a non-streaming request carries 16,384 and the
    // notice says so while still reporting the discovered limit.
    let transcripts = run_probe(50_000, 16_384);

    assert!(
        transcripts.contains("input 300,000 · output 50,000"),
        "notice should still state the discovered limits: {transcripts}"
    );
    assert!(
        transcripts.contains("output capped at 16,384 for non-streaming requests"),
        "notice should say the non-streaming cap applied: {transcripts}"
    );
}
