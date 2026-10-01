//! `rupu run` discovers the model's real limits and sends them (spec
//! 2026-09-30 §6.1). The httpmock server plays Anthropic: it serves the model
//! listing (with `max_tokens`) and accepts the messages request only when it
//! carries the discovered cap.

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
            .body_contains("\"max_tokens\":50000");
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
}
