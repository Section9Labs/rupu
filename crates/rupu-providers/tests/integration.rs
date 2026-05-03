//! Integration tests for rupu-providers — public API only, no live API calls.

use rupu_providers::*;

#[test]
fn test_full_request_response_type_flow() {
    let request = LlmRequest {
        model: "claude-sonnet-4-6".into(),
        system: Some("You are a helpful assistant.".into()),
        messages: vec![Message::user("What is 2+2?")],
        max_tokens: 1024,
        tools: vec![],
        cell_id: None,
        trace_id: None,
        thinking: None,
        task_type: None,
    };
    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.model, "claude-sonnet-4-6");

    let response = LlmResponse {
        id: "msg_test".into(),
        model: "claude-sonnet-4-6".into(),
        content: vec![ContentBlock::Text { text: "4".into() }],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage {
            input_tokens: 15,
            output_tokens: 1,
            ..Default::default()
        },
    };
    assert_eq!(response.text(), Some("4"));
    assert!(response.tool_calls().is_empty());
}

#[test]
fn test_tool_use_flow() {
    let request = LlmRequest {
        model: "claude-sonnet-4-6".into(),
        system: None,
        messages: vec![Message::user("Read /tmp/test.txt")],
        max_tokens: 1024,
        tools: vec![ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }),
        }],
        cell_id: Some("test-cell".into()),
        trace_id: Some("trace-abc".into()),
        thinking: None,
        task_type: None,
    };
    assert_eq!(request.tools.len(), 1);

    let response = LlmResponse {
        id: "msg_tool".into(),
        model: "claude-sonnet-4-6".into(),
        content: vec![
            ContentBlock::Text {
                text: "Let me read that file.".into(),
            },
            ContentBlock::ToolUse {
                id: "toolu_abc".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "/tmp/test.txt"}),
            },
        ],
        stop_reason: Some(StopReason::ToolUse),
        usage: Usage::default(),
    };
    assert_eq!(response.tool_calls().len(), 1);

    let tool_result = Message::tool_result("toolu_abc", "file contents here", false);
    assert_eq!(tool_result.role, Role::User);
}

#[test]
fn test_sse_parser_full_stream_simulation() {
    use rupu_providers::sse::SseParser;

    let mut parser = SseParser::new();

    let stream_data = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4-6\",\"usage\":{\"input_tokens\":25}}}\n",
        "\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n",
        "\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
        "\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n",
        "\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
        "\n",
    );

    let events = parser.feed(stream_data.as_bytes()).unwrap();
    assert_eq!(events.len(), 7);
    assert_eq!(events[0].event_type, "message_start");
    assert_eq!(events[6].event_type, "message_stop");
}

#[test]
fn test_all_public_types_accessible() {
    let _request = LlmRequest {
        model: String::new(),
        system: None,
        messages: vec![],
        max_tokens: 0,
        tools: vec![],
        cell_id: None,
        trace_id: None,
        thinking: None,
        task_type: None,
    };
    let _role = Role::User;
    let _stop = StopReason::EndTurn;
    let _usage = Usage::default();
    let _block = ContentBlock::Text {
        text: String::new(),
    };
    let _err = ProviderError::MissingAuth {
        provider: "anthropic".into(),
        env_hint: "ANTHROPIC_API_KEY".into(),
    };
}
