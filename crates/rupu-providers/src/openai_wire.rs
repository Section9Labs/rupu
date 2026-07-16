//! Shared OpenAI `/v1/chat/completions` wire helpers.
//!
//! Pure request-build / response-parse / SSE-accumulate logic used by every
//! provider that speaks the OpenAI chat-completions dialect (GitHub Copilot,
//! the generic OpenAI-compatible client). Provider-specific concerns —
//! base URL, auth headers, token exchange — stay in the individual clients.

use crate::error::ProviderError;
use crate::types::{ContentBlock, LlmRequest, LlmResponse, Role, StopReason, StreamEvent, Usage};

/// Canonical tag for the OpenAI chat-completions dialect. GitHub Copilot and
/// the generic OpenAI-compatible client both speak it and interoperate within
/// it, so they share one tag.
pub(crate) const PROVIDER_TAG: &str = "openai_chat";

/// The reasoning field names seen in the wild, in priority order.
///
/// There is no consensus: DeepSeek/Qwen/GLM use `reasoning_content`; vLLM
/// renamed it to `reasoning` at 0.12; Ollama's compat endpoint uses
/// `reasoning`. Reading one name is provably insufficient, and the server
/// version is not ours to control — so we read both and remember which one
/// arrived, because the echo must go back under the SAME key.
const REASONING_FIELDS: [&str; 2] = ["reasoning_content", "reasoning"];

/// Return the first present reasoning field of `v` as `(name, value)`.
///
/// Best-effort: an absent or empty field carries nothing to show and nothing
/// to echo, so it is skipped rather than treated as an error.
fn extract_reasoning_fields(v: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    REASONING_FIELDS.iter().find_map(|f| {
        let val = v.get(*f)?;
        if val.is_null() || val.as_str().map(|s| s.is_empty()).unwrap_or(false) {
            return None;
        }
        Some(((*f).to_string(), val.clone()))
    })
}

/// Build an OpenAI chat-completions request body from an `LlmRequest`.
pub(crate) fn build_chat_request_body(request: &LlmRequest, stream: bool) -> serde_json::Value {
    let mut messages = Vec::new();

    if let Some(system) = &request.system {
        messages.push(serde_json::json!({"role": "system", "content": system}));
    }

    for msg in &request.messages {
        match &msg.content[..] {
            [ContentBlock::Text { text }] => {
                let role = match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                messages.push(serde_json::json!({"role": role, "content": text}));
            }
            [ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            }] => {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                }));
            }
            blocks => {
                let role = match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();

                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => text_parts.push(text.clone()),
                        ContentBlock::ToolUse { id, name, input } => {
                            tool_calls.push(serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": { "name": name, "arguments": input.to_string() }
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            messages.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content,
                            }));
                            continue;
                        }
                        ContentBlock::Reasoning { .. } => { /* Plan 3: reasoning_content */ }
                        ContentBlock::Unknown => {}
                    }
                }

                if !text_parts.is_empty() || !tool_calls.is_empty() {
                    let mut msg_json =
                        serde_json::json!({"role": role, "content": text_parts.join("\n")});
                    if !tool_calls.is_empty() {
                        msg_json["tool_calls"] = serde_json::json!(tool_calls);
                    }
                    messages.push(msg_json);
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_tokens,
        "stream": stream,
    });

    // Ask the server to emit a final usage chunk on streamed responses.
    // OpenAI-compatible endpoints (vLLM, OpenAI, Copilot, …) omit usage from
    // SSE unless `stream_options.include_usage` is set, which otherwise leaves
    // token/cost accounting at zero for every streamed run.
    if stream {
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }

    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
        body["tool_choice"] = serde_json::json!("auto");
    }

    if let Some(level) = &request.thinking {
        use crate::model_tier::ThinkingLevel;
        let effort = match level {
            ThinkingLevel::Auto => None,
            ThinkingLevel::Minimal => Some("minimal"),
            ThinkingLevel::Low => Some("low"),
            ThinkingLevel::Medium => Some("medium"),
            ThinkingLevel::High => Some("high"),
            ThinkingLevel::Max => Some("xhigh"),
        };
        if let Some(e) = effort {
            body["reasoning_effort"] = serde_json::json!(e);
        }
    }

    body
}

/// Parse a non-streaming chat-completions response into an `LlmResponse`.
pub(crate) fn parse_chat_completion(
    json: &serde_json::Value,
) -> Result<LlmResponse, ProviderError> {
    let id = json["id"].as_str().unwrap_or("").to_string();
    let model = json["model"].as_str().unwrap_or("").to_string();

    let mut content = Vec::new();
    let mut stop_reason = Some(StopReason::EndTurn);

    if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
        if let Some(choice) = choices.first() {
            if let Some(text) = choice
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                if !text.is_empty() {
                    content.push(ContentBlock::Text {
                        text: text.to_string(),
                    });
                }
            }

            if let Some(tool_calls) = choice
                .get("message")
                .and_then(|m| m.get("tool_calls"))
                .and_then(|t| t.as_array())
            {
                for tc in tool_calls {
                    let tc_id = tc["id"].as_str().unwrap_or("").to_string();
                    let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                    let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                    let input: serde_json::Value = serde_json::from_str(args_str).map_err(|e| {
                        ProviderError::Json(format!("malformed tool arguments for '{name}': {e}"))
                    })?;
                    content.push(ContentBlock::ToolUse {
                        id: tc_id,
                        name,
                        input,
                    });
                    stop_reason = Some(StopReason::ToolUse);
                }
            }

            // Capture reasoning under the key it arrived on. `raw` is what
            // Task 2 echoes back verbatim: DeepSeek REQUIRES reasoning_content
            // on tool-call turns and returns a 400 if it is stripped.
            // Reasoning precedes text, consistent with the other providers.
            if let Some((field, value)) = choice.get("message").and_then(extract_reasoning_fields) {
                let text = value.as_str().map(|s| s.to_string());
                content.insert(
                    0,
                    ContentBlock::Reasoning {
                        text,
                        provider: PROVIDER_TAG.to_string(),
                        model: model.clone(),
                        raw: serde_json::json!({ field: value }),
                    },
                );
            }

            if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                stop_reason = Some(match reason {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    "tool_calls" => StopReason::ToolUse,
                    _ => StopReason::EndTurn,
                });
            }
        }
    }

    let usage = if let Some(u) = json.get("usage") {
        Usage {
            input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
            ..Default::default()
        }
    } else {
        Usage::default()
    };

    Ok(LlmResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

#[derive(Default)]
pub(crate) struct ToolCallAcc {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

pub(crate) struct CompletionAccumulator {
    pub id: String,
    pub model: String,
    pub text: String,
    pub tool_calls: Vec<ToolCallAcc>,
    pub stop_reason: Option<StopReason>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Concatenated reasoning deltas.
    pub reasoning_text: String,
    /// Which of `REASONING_FIELDS` this stream used — first one wins. The echo
    /// must go back under the same key the endpoint sent.
    pub reasoning_field: Option<String>,
}

impl CompletionAccumulator {
    pub(crate) fn new() -> Self {
        Self {
            id: String::new(),
            model: String::new(),
            text: String::new(),
            tool_calls: Vec::new(),
            stop_reason: None,
            input_tokens: 0,
            output_tokens: 0,
            reasoning_text: String::new(),
            reasoning_field: None,
        }
    }

    pub(crate) fn into_response(self) -> Option<LlmResponse> {
        if self.id.is_empty() && self.text.is_empty() && self.tool_calls.is_empty() {
            return None;
        }
        let mut content = Vec::new();
        if !self.reasoning_text.is_empty() {
            // Under the key this stream used — never renamed or normalized.
            let field = self
                .reasoning_field
                .clone()
                .unwrap_or_else(|| REASONING_FIELDS[0].to_string());
            content.push(ContentBlock::Reasoning {
                text: Some(self.reasoning_text.clone()),
                provider: PROVIDER_TAG.to_string(),
                model: self.model.clone(),
                raw: serde_json::json!({ field: self.reasoning_text }),
            });
        }
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        for tc in self.tool_calls {
            if tc.name.is_empty() {
                continue;
            }
            let input: serde_json::Value =
                serde_json::from_str(&tc.arguments).unwrap_or(serde_json::json!({}));
            content.push(ContentBlock::ToolUse {
                id: tc.id,
                name: tc.name,
                input,
            });
        }
        Some(LlmResponse {
            id: self.id,
            model: self.model,
            content,
            stop_reason: self.stop_reason,
            usage: Usage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                ..Default::default()
            },
        })
    }
}

/// Fold one chat-completions SSE event into the accumulator.
pub(crate) fn process_completion_sse(
    event: &crate::sse::SseEvent,
    acc: &mut CompletionAccumulator,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
) -> Result<(), ProviderError> {
    if event.data == "[DONE]" {
        return Ok(());
    }
    let data: serde_json::Value = serde_json::from_str(&event.data)?;

    if let Some(id) = data["id"].as_str() {
        if acc.id.is_empty() {
            acc.id = id.to_string();
        }
    }
    if let Some(model) = data["model"].as_str() {
        if acc.model.is_empty() {
            acc.model = model.to_string();
        }
    }

    if let Some(choices) = data.get("choices").and_then(|c| c.as_array()) {
        if let Some(choice) = choices.first() {
            let delta = &choice["delta"];

            // Reasoning deltas stream on their own field; they must never be
            // folded into `content` / emitted as a TextDelta.
            if let Some((field, value)) = extract_reasoning_fields(delta) {
                if let Some(chunk) = value.as_str() {
                    acc.reasoning_field.get_or_insert(field);
                    acc.reasoning_text.push_str(chunk);
                    on_event(StreamEvent::ReasoningDelta(chunk.to_string()));
                }
            }

            if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                acc.text.push_str(text);
                on_event(StreamEvent::TextDelta(text.to_string()));
            }

            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tool_calls {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    if let Some(func) = tc.get("function") {
                        if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                            let tc_id = tc["id"].as_str().unwrap_or("").to_string();
                            while acc.tool_calls.len() <= idx {
                                acc.tool_calls.push(ToolCallAcc::default());
                            }
                            acc.tool_calls[idx].id = tc_id.clone();
                            acc.tool_calls[idx].name = name.to_string();
                            on_event(StreamEvent::ToolUseStart {
                                id: tc_id,
                                name: name.to_string(),
                            });
                        }
                        if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                            while acc.tool_calls.len() <= idx {
                                acc.tool_calls.push(ToolCallAcc::default());
                            }
                            acc.tool_calls[idx].arguments.push_str(args);
                            on_event(StreamEvent::InputJsonDelta(args.to_string()));
                        }
                    }
                }
            }

            if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                acc.stop_reason = Some(match reason {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    "tool_calls" => StopReason::ToolUse,
                    _ => StopReason::EndTurn,
                });
            }
        }
    }

    if let Some(u) = data.get("usage") {
        if let Some(input) = u.get("prompt_tokens").and_then(|v| v.as_u64()) {
            acc.input_tokens = input as u32;
        }
        if let Some(output) = u.get("completion_tokens").and_then(|v| v.as_u64()) {
            acc.output_tokens = output as u32;
        }
        on_event(StreamEvent::UsageSnapshot(Usage {
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            cached_tokens: 0,
        }));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::SseEvent;

    fn ev(data: &str) -> SseEvent {
        SseEvent {
            event_type: "message".into(),
            data: data.into(),
        }
    }

    #[test]
    fn streamed_usage_chunk_is_recorded_into_the_response() {
        // A content chunk followed by an OpenAI-style usage-only final chunk
        // (`choices: []`, `usage: {...}`) — the shape a server emits once
        // `stream_options.include_usage` is requested.
        let mut acc = CompletionAccumulator::new();
        let mut sink = |_e: StreamEvent| {};
        process_completion_sse(
            &ev(r#"{"id":"cmpl_1","model":"glm","choices":[{"index":0,"delta":{"content":"hello"}}]}"#),
            &mut acc,
            &mut sink,
        )
        .unwrap();
        process_completion_sse(
            &ev(r#"{"id":"cmpl_1","model":"glm","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}}"#),
            &mut acc,
            &mut sink,
        )
        .unwrap();

        let resp = acc.into_response().expect("response");
        assert_eq!(resp.usage.input_tokens, 11);
        assert_eq!(resp.usage.output_tokens, 7);
    }

    /// The single `Reasoning` block in a response, or panic.
    fn only_reasoning(resp: &LlmResponse) -> (&Option<String>, &str, &str, &serde_json::Value) {
        let mut found = None;
        for b in &resp.content {
            if let ContentBlock::Reasoning {
                text,
                provider,
                model,
                raw,
            } = b
            {
                assert!(
                    found.is_none(),
                    "more than one Reasoning block: {:?}",
                    resp.content
                );
                found = Some((text, provider.as_str(), model.as_str(), raw));
            }
        }
        found.unwrap_or_else(|| panic!("no Reasoning block in {:?}", resp.content))
    }

    #[test]
    fn parse_captures_reasoning_content_field() {
        let json = serde_json::json!({
            "id": "cmpl_r1",
            "model": "deepseek-reasoner",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "42",
                    "reasoning_content": "step by step"
                },
                "finish_reason": "stop"
            }]
        });
        let resp = parse_chat_completion(&json).unwrap();

        let (text, provider, model, raw) = only_reasoning(&resp);
        assert_eq!(text.as_deref(), Some("step by step"));
        assert_eq!(provider, "openai_chat");
        assert_eq!(model, "deepseek-reasoner");
        // The original key is preserved verbatim — the echo must use it.
        assert_eq!(
            raw,
            &serde_json::json!({"reasoning_content": "step by step"})
        );

        // Reasoning precedes text, and the text block still parses as today.
        assert!(matches!(resp.content[0], ContentBlock::Reasoning { .. }));
        assert!(matches!(&resp.content[1], ContentBlock::Text { text } if text == "42"));
        assert_eq!(resp.content.len(), 2);
        assert_eq!(resp.reasoning_text().as_deref(), Some("step by step"));
    }

    #[test]
    fn parse_captures_renamed_reasoning_field() {
        // vLLM >= 0.12 renamed the field to `reasoning`.
        let json = serde_json::json!({
            "id": "cmpl_r2",
            "model": "qwen3-vllm",
            "choices": [{
                "message": {"role": "assistant", "content": "ok", "reasoning": "vllm thoughts"},
                "finish_reason": "stop"
            }]
        });
        let resp = parse_chat_completion(&json).unwrap();

        let (text, provider, _model, raw) = only_reasoning(&resp);
        assert_eq!(text.as_deref(), Some("vllm thoughts"));
        assert_eq!(provider, "openai_chat");
        // Recorded under the key it arrived on — NOT normalized to reasoning_content.
        assert_eq!(raw, &serde_json::json!({"reasoning": "vllm thoughts"}));
        assert!(raw.get("reasoning_content").is_none());
    }

    #[test]
    fn parse_prefers_reasoning_content_when_both_present() {
        let json = serde_json::json!({
            "id": "cmpl_r3",
            "model": "both",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "ok",
                    "reasoning_content": "canonical",
                    "reasoning": "renamed"
                }
            }]
        });
        let resp = parse_chat_completion(&json).unwrap();

        let (text, _provider, _model, raw) = only_reasoning(&resp);
        assert_eq!(text.as_deref(), Some("canonical"));
        assert_eq!(raw, &serde_json::json!({"reasoning_content": "canonical"}));
    }

    #[test]
    fn parse_without_reasoning_emits_no_reasoning_block() {
        let json = serde_json::json!({
            "id": "cmpl_r4",
            "model": "gpt-4o",
            "choices": [{
                "message": {"role": "assistant", "content": "plain"},
                "finish_reason": "stop"
            }]
        });
        let resp = parse_chat_completion(&json).unwrap();

        assert_eq!(resp.content.len(), 1);
        assert!(matches!(&resp.content[0], ContentBlock::Text { text } if text == "plain"));
        assert!(resp.reasoning_text().is_none());
    }

    #[test]
    fn parse_ignores_empty_reasoning_string() {
        let json = serde_json::json!({
            "id": "cmpl_r5",
            "model": "deepseek-reasoner",
            "choices": [{
                "message": {"role": "assistant", "content": "plain", "reasoning_content": ""}
            }]
        });
        let resp = parse_chat_completion(&json).unwrap();

        assert_eq!(resp.content.len(), 1);
        assert!(matches!(resp.content[0], ContentBlock::Text { .. }));
    }

    #[test]
    fn parse_keeps_reasoning_alongside_tool_calls() {
        // The DeepSeek 400 case: reasoning + tool_calls on one turn.
        let json = serde_json::json!({
            "id": "cmpl_r6",
            "model": "deepseek-reasoner",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "need the weather",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\":\"Oslo\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let resp = parse_chat_completion(&json).unwrap();

        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert!(matches!(resp.content[0], ContentBlock::Reasoning { .. }));
        match &resp.content[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "get_weather");
                assert_eq!(input, &serde_json::json!({"city": "Oslo"}));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        assert_eq!(resp.content.len(), 2);
    }

    #[test]
    fn sse_accumulates_reasoning_deltas_and_emits_events() {
        let mut acc = CompletionAccumulator::new();
        let mut events = Vec::new();
        {
            let mut sink = |e: StreamEvent| events.push(e);
            for chunk in ["step ", "by ", "step"] {
                let data = serde_json::json!({
                    "id": "cmpl_s1",
                    "model": "deepseek-reasoner",
                    "choices": [{"index": 0, "delta": {"reasoning_content": chunk}}]
                });
                process_completion_sse(&ev(&data.to_string()), &mut acc, &mut sink).unwrap();
            }
            let data = serde_json::json!({
                "id": "cmpl_s1",
                "model": "deepseek-reasoner",
                "choices": [{"index": 0, "delta": {"content": "42"}, "finish_reason": "stop"}]
            });
            process_completion_sse(&ev(&data.to_string()), &mut acc, &mut sink).unwrap();
        }

        // One ReasoningDelta per chunk; reasoning never leaks out as TextDelta.
        let reasoning_deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ReasoningDelta(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(reasoning_deltas, vec!["step ", "by ", "step"]);
        let text_deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_deltas, vec!["42"]);

        let resp = acc.into_response().expect("response");
        let (text, provider, model, raw) = only_reasoning(&resp);
        assert_eq!(text.as_deref(), Some("step by step"));
        assert_eq!(provider, "openai_chat");
        assert_eq!(model, "deepseek-reasoner");
        assert_eq!(
            raw,
            &serde_json::json!({"reasoning_content": "step by step"})
        );
        assert!(matches!(resp.content[0], ContentBlock::Reasoning { .. }));
        assert!(matches!(&resp.content[1], ContentBlock::Text { text } if text == "42"));
    }

    #[test]
    fn sse_accumulates_renamed_reasoning_deltas() {
        let mut acc = CompletionAccumulator::new();
        let mut sink = |_e: StreamEvent| {};
        for chunk in ["vllm ", "thoughts"] {
            let data = serde_json::json!({
                "id": "cmpl_s2",
                "model": "qwen3-vllm",
                "choices": [{"index": 0, "delta": {"reasoning": chunk}}]
            });
            process_completion_sse(&ev(&data.to_string()), &mut acc, &mut sink).unwrap();
        }

        let resp = acc.into_response().expect("response");
        let (text, _provider, _model, raw) = only_reasoning(&resp);
        assert_eq!(text.as_deref(), Some("vllm thoughts"));
        assert_eq!(raw, &serde_json::json!({"reasoning": "vllm thoughts"}));
        assert!(raw.get("reasoning_content").is_none());
    }

    #[test]
    fn sse_without_reasoning_is_unchanged() {
        let mut acc = CompletionAccumulator::new();
        let mut events = Vec::new();
        {
            let mut sink = |e: StreamEvent| events.push(e);
            process_completion_sse(
                &ev(r#"{"id":"cmpl_s3","model":"glm","choices":[{"index":0,"delta":{"content":"hi"}}]}"#),
                &mut acc,
                &mut sink,
            )
            .unwrap();
        }

        let resp = acc.into_response().expect("response");
        assert_eq!(resp.content.len(), 1);
        assert!(matches!(&resp.content[0], ContentBlock::Text { text } if text == "hi"));
        assert!(!events
            .iter()
            .any(|e| matches!(e, StreamEvent::ReasoningDelta(_))));
    }
}
