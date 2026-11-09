//! Actual session-turn wiring with a fixture MCP server and mock provider.
use super::tests::{idle_dense_session, with_mock_home};
use super::*;
use httpmock::{Method::POST, MockServer};
use serde_json::json;

fn metadata(server: &MockServer) {
    for (method, id, result) in [
        (
            "initialize",
            1,
            json!({"protocolVersion":rupu_mcp_client::PROTOCOL_VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}),
        ),
        (
            "tools/list",
            2,
            json!({"tools":[{"name":"query","description":"Fixture","inputSchema":{"type":"object"}}]}),
        ),
    ] {
        server.mock(|when, then| {
            when.method(POST)
                .json_body_partial(json!({"method":method}).to_string());
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"jsonrpc":"2.0","id":id,"result":result}));
        });
    }
    server.mock(|when, then| {
        when.method(POST)
            .json_body_partial(r#"{"method":"notifications/initialized"}"#);
        then.status(202);
    });
}

#[tokio::test]
async fn worker_mcp_uses_frozen_grant_permission_and_override_before_dispatch() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let server = MockServer::start_async().await;
    metadata(&server);
    let calls=server.mock(|when,then| {
        when.method(POST).json_body_partial(r#"{"method":"tools/call","params":{"name":"query"}}"#);
        then.status(200).header("content-type","application/json").json_body(json!({"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"fixture"}],"structuredContent":{"fixture":true}}}));
    });
    for (mode, disabled, decision, expected_calls) in [
        ("readonly", false, "denied:readonly", 0),
        ("bypass", true, "not_granted", 0),
        ("bypass", false, "allowed", 1),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (global, mut record) = idle_dense_session(&tmp, "ses_mcp_fixture");
        record.message_history.clear();
        record.permission_mode = mode.into();
        record.agent_tools = Some(vec![]);
        let source = format!(
            "version=1\n[servers.fixture]\nlabel='Fixture'\nurl='{}'\nenabled=true",
            server.url("/mcp")
        );
        fs::write(global.join("mcp.toml"), &source).unwrap();
        let registry = rupu_config::mcp::Registry::parse(&source).unwrap();
        let frozen = rupu_runtime::session_mcp::freeze(
            &registry,
            &rupu_mcp_client::EnvironmentCredentials,
            None,
            None,
        )
        .await
        .unwrap();
        let name = frozen.tools[0].identity.local_name();
        record.mcp_tools = Some(frozen);
        write_session(&global, SessionScope::Active, &record).unwrap();
        if disabled {
            rupu_runtime::session_settings::SessionSettings {
                mcp: Some(Default::default()),
                ..Default::default()
            }
            .save(&session_dir(
                &global,
                SessionScope::Active,
                &record.session_id,
            ))
            .unwrap();
        }
        let script=json!([
            {"AssistantToolUse":{"text":null,"tool_id":"mcp_call_fixture","tool_name":name,"tool_input":{},"stop":"tool_use"}},
            {"AssistantText":{"text":"done","stop":"end_turn"}}
        ]).to_string();
        with_mock_home(
            &global,
            &script,
            run_turn(RunTurnArgs {
                session_id: record.session_id.clone(),
                run_id: "run_mcp_fixture".into(),
                prompt: "fixture".into(),
                operator_skills: vec![],
            }),
        )
        .await
        .unwrap();
        let events: Vec<_> =
            JsonlReader::iter(record.transcripts_dir.join("run_mcp_fixture.jsonl"))
                .unwrap()
                .map(Result::unwrap)
                .collect();
        assert!(events.iter().any(|event| matches!(event,TranscriptEvent::ToolAudit {tool,decision:Some(actual),..} if tool==&name && actual==decision)),"missing {decision} audit: {events:?}");
        let granted=events.iter().any(|event| matches!(event,TranscriptEvent::ToolGrant {entries,..} if entries.iter().any(|entry|entry.remote.as_ref().is_some_and(|remote|remote.server=="fixture" && remote.tool=="query"))));
        assert_eq!(granted, !disabled);
        if expected_calls == 1 {
            assert!(events.iter().any(|event| matches!(event,TranscriptEvent::ToolResult {structured:Some(value),..} if value["structuredContent"]["fixture"]==true)));
        }
        calls.assert_hits(expected_calls);
    }
}

#[tokio::test]
async fn worker_stale_policy_fails_before_provider_or_tool_invocation() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let server = MockServer::start_async().await;
    metadata(&server);
    let calls = server.mock(|when, then| {
        when.method(POST)
            .json_body_partial(r#"{"method":"tools/call"}"#);
        then.status(500);
    });
    let tmp = tempfile::tempdir().unwrap();
    let (global, mut record) = idle_dense_session(&tmp, "ses_mcp_stale");
    let source = format!(
        "version=1\n[servers.fixture]\nlabel='Fixture'\nurl='{}'\nenabled=true",
        server.url("/mcp")
    );
    let registry = rupu_config::mcp::Registry::parse(&source).unwrap();
    record.mcp_tools = Some(
        rupu_runtime::session_mcp::freeze(
            &registry,
            &rupu_mcp_client::EnvironmentCredentials,
            None,
            None,
        )
        .await
        .unwrap(),
    );
    write_session(&global, SessionScope::Active, &record).unwrap();
    fs::write(
        global.join("mcp.toml"),
        format!("{source}\n[servers.fixture.tool_effects]\nquery='read'"),
    )
    .unwrap();
    let result = with_mock_home(
        &global,
        "INVALID MOCK SCRIPT: provider must not be constructed",
        run_turn(RunTurnArgs {
            session_id: record.session_id.clone(),
            run_id: "run_mcp_stale".into(),
            prompt: "fixture".into(),
            operator_skills: vec![],
        }),
    )
    .await;
    assert!(matches!(
        result
            .unwrap_err()
            .downcast_ref::<rupu_runtime::session_mcp::Error>(),
        Some(rupu_runtime::session_mcp::Error::Stale)
    ));
    calls.assert_hits(0);
    // Discovery may truthfully record metadata netflow before policy refusal.
    // The rejected turn must never enter the provider/tool execution lifecycle.
    let transcript = record.transcripts_dir.join("run_mcp_stale.jsonl");
    if transcript.exists() {
        let events: Vec<_> = JsonlReader::iter(transcript)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!events.iter().any(|event| matches!(
            event,
            TranscriptEvent::RunStart { .. }
        )));
    }
}
