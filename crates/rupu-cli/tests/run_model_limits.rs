//! `rupu run` discovers the model's real limits and sends them (spec
//! 2026-09-30 §6.1). The httpmock server plays Anthropic: it serves the model
//! listing (with `max_tokens`) and accepts the messages request only when it
//! carries the discovered cap.
//!
//! The run passes `--no-stream`, which only changes display: the request still
//! streams on the wire, so the mock serves an SSE body and the full discovered
//! cap goes out (nothing is capped to dodge an HTTP timeout).

use assert_cmd::Command;
use httpmock::prelude::*;

#[test]
fn rupu_run_sends_the_discovered_output_cap_and_announces_limits() {
    let server = MockServer::start();
    let list = server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{
                "id": "claude-test-1",
                "type": "model",
                "max_input_tokens": 300000,
                "max_tokens": 50000
            }],
            "has_more": false
        }));
    });
    let msgs = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("\"max_tokens\":50000")
            .body_contains("\"stream\":true");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",",
                "\"model\":\"claude-test-1\",\"usage\":{\"input_tokens\":5}}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,",
                "\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,",
                "\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
                "event: content_block_stop\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},",
                "\"usage\":{\"output_tokens\":1}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            ));
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

    // The run-start notice states what was resolved and where it came from.
    let transcripts = std::fs::read_dir(home.join("transcripts"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(
        transcripts.contains("\"kind\":\"model_limits\""),
        "no model_limits notice in the transcript: {transcripts}"
    );
    assert!(
        transcripts.contains("input 300,000 · output 50,000"),
        "notice should state the discovered limits: {transcripts}"
    );
    assert!(
        transcripts.contains("anthropic model list"),
        "notice should name the limit source: {transcripts}"
    );
    assert!(
        !transcripts.contains("capped"),
        "nothing is capped for a --no-stream run: {transcripts}"
    );
    // `--no-stream` keeps the transcript's final-message-only shape even
    // though the request streamed.
    assert!(
        !transcripts.contains("\"type\":\"assistant_delta\""),
        "a --no-stream transcript has no assistant_delta events: {transcripts}"
    );
}
