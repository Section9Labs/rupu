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
                let mut reasoning_fields = serde_json::Map::new();

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
                        ContentBlock::Reasoning { provider, raw, .. }
                            if provider == PROVIDER_TAG =>
                        {
                            // Echo back exactly the reasoning fields this
                            // endpoint sent, under their original keys.
                            // DeepSeek REQUIRES `reasoning_content` back on
                            // tool-call turns (400 if stripped); backends that
                            // don't want it ignore or drop it. We never invent
                            // a field an endpoint didn't send, and never
                            // rename between `reasoning_content`/`reasoning` —
                            // that is what makes one shared code path safe
                            // across servers with contradictory contracts.
                            if let Some(obj) = raw.as_object() {
                                reasoning_fields.extend(obj.clone());
                            }
                        }
                        ContentBlock::Reasoning { .. } => {
                            // Foreign provider (an Anthropic signature, a
                            // Gemini part): an alien wire format that this
                            // endpoint never sent and would reject. Drop it.
                        }
                        ContentBlock::Unknown => {}
                    }
                }

                if !text_parts.is_empty() || !tool_calls.is_empty() {
                    let mut msg_json =
                        serde_json::json!({"role": role, "content": text_parts.join("\n")});
                    if !tool_calls.is_empty() {
                        msg_json["tool_calls"] = serde_json::json!(tool_calls);
                    }
                    // Sibling of `content`/`tool_calls` — the replay shape
                    // DeepSeek documents. Merged inside the emit guard so
                    // reasoning alone never resurrects an otherwise-empty
                    // message, while a reasoning + tool_calls turn carries it.
                    for (k, v) in reasoning_fields {
                        msg_json[k] = v;
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
            if let Some(message) = choice.get("message") {
                if let Some((field, value)) = extract_reasoning_fields(message) {
                    let text = value.as_str().map(|s| s.to_string());
                    // OpenRouter sends `reasoning` (this same string field)
                    // PLUS a structured `reasoning_details` array carrying the
                    // real continuity tokens (`reasoning.encrypted`,
                    // `signature`). OpenRouter's documented contract is that
                    // the entire reasoning_details sequence must be replayed
                    // verbatim and in order on the next turn — we do not
                    // support that yet (out of scope, needs its own design).
                    // Echoing the bare `reasoning` string back without
                    // `reasoning_details` would be a partial, modified replay
                    // of an order-sensitive sequence: worse than sending
                    // nothing, which is what rupu does today. So when
                    // `reasoning_details` is present we still capture the
                    // text for the transcript, but leave `raw` empty so the
                    // echo merge in `build_chat_request_body` contributes
                    // nothing and the outgoing body is unchanged.
                    let has_reasoning_details = message
                        .get("reasoning_details")
                        .map(|rd| !rd.is_null())
                        .unwrap_or(false);
                    let raw = if has_reasoning_details {
                        serde_json::json!({})
                    } else {
                        serde_json::json!({ field: value })
                    };
                    content.insert(
                        0,
                        ContentBlock::Reasoning {
                            text,
                            provider: PROVIDER_TAG.to_string(),
                            model: model.clone(),
                            raw,
                        },
                    );
                }
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
            // Only echo under the key this stream actually used. If reasoning
            // text somehow accumulated without ever recording which field it
            // arrived on, there is nothing safe to guess: dropping the block
            // beats inventing or renaming a key, which is exactly the failure
            // this module exists to prevent.
            if let Some(field) = self.reasoning_field.clone() {
                content.push(ContentBlock::Reasoning {
                    text: Some(self.reasoning_text.clone()),
                    provider: PROVIDER_TAG.to_string(),
                    model: self.model.clone(),
                    raw: serde_json::json!({ field: self.reasoning_text }),
                });
            }
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
    fn parse_captures_openrouter_reasoning_for_display_only_when_reasoning_details_present() {
        // OpenRouter sends `reasoning` (a plain string, same shape as
        // vLLM/Ollama) PLUS a structured `reasoning_details` array carrying
        // the real continuity tokens. Its documented contract requires the
        // entire reasoning_details sequence be replayed verbatim and in
        // order on tool-call turns. We don't support replaying
        // reasoning_details yet, so we capture the plain text for the
        // transcript but leave `raw` empty — nothing to echo.
        let json = serde_json::json!({
            "id": "cmpl_or1",
            "model": "openrouter/some-model",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "42",
                    "reasoning": "step by step",
                    "reasoning_details": [{
                        "type": "reasoning.encrypted",
                        "data": "enc-blob",
                        "signature": "sig-abc"
                    }]
                },
                "finish_reason": "stop"
            }]
        });
        let resp = parse_chat_completion(&json).unwrap();

        let (text, provider, _model, raw) = only_reasoning(&resp);
        assert_eq!(text.as_deref(), Some("step by step"));
        assert_eq!(provider, "openai_chat");
        assert_eq!(raw, &serde_json::json!({}));
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

    // ---- Echo (request-build) -------------------------------------------

    use crate::types::Message;

    fn req(messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            model: "deepseek-reasoner".into(),
            messages,
            max_tokens: 128,
            ..Default::default()
        }
    }

    /// An `openai_chat` reasoning block carrying `raw` verbatim.
    fn reasoning(raw: serde_json::Value) -> ContentBlock {
        ContentBlock::Reasoning {
            text: Some("thinking".into()),
            provider: PROVIDER_TAG.to_string(),
            model: "deepseek-reasoner".into(),
            raw,
        }
    }

    fn tool_use() -> ContentBlock {
        ContentBlock::ToolUse {
            id: "call_1".into(),
            name: "get_weather".into(),
            input: serde_json::json!({"city": "Oslo"}),
        }
    }

    fn assistant(content: Vec<ContentBlock>) -> Message {
        Message {
            role: Role::Assistant,
            content,
        }
    }

    /// The lone assistant message in a built body, or panic.
    fn only_assistant_msg(body: &serde_json::Value) -> &serde_json::Value {
        let msgs = body["messages"].as_array().expect("messages array");
        let mut found = None;
        for m in msgs {
            if m["role"] == "assistant" {
                assert!(found.is_none(), "more than one assistant message: {body}");
                found = Some(m);
            }
        }
        found.unwrap_or_else(|| panic!("no assistant message in {body}"))
    }

    #[test]
    fn assistant_message_echoes_reasoning_content_key() {
        // The DeepSeek 400 case: reasoning + text + tool_calls on one turn.
        // The reasoning must ride back as a SIBLING of content/tool_calls.
        let body = build_chat_request_body(
            &req(vec![
                Message::user("weather in Oslo?"),
                assistant(vec![
                    reasoning(serde_json::json!({"reasoning_content": "thinking"})),
                    ContentBlock::Text {
                        text: "let me check".into(),
                    },
                    tool_use(),
                ]),
            ]),
            false,
        );

        assert_eq!(
            only_assistant_msg(&body),
            &serde_json::json!({
                "role": "assistant",
                "content": "let me check",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Oslo\"}"}
                }],
                "reasoning_content": "thinking",
            })
        );
    }

    #[test]
    fn assistant_message_echoes_under_the_original_key() {
        // vLLM sent `reasoning`; it gets `reasoning` back. We never invent a
        // field an endpoint did not send.
        let body = build_chat_request_body(
            &req(vec![assistant(vec![
                reasoning(serde_json::json!({"reasoning": "vllm thoughts"})),
                tool_use(),
            ])]),
            false,
        );

        let msg = only_assistant_msg(&body);
        assert_eq!(msg["reasoning"], serde_json::json!("vllm thoughts"));
        assert!(
            msg.get("reasoning_content").is_none(),
            "must not translate between field names: {msg}"
        );
    }

    #[test]
    fn openrouter_reasoning_with_details_is_not_echoed_to_the_wire() {
        // Reuse the exact Reasoning block parsing an OpenRouter response
        // would produce (raw == {}, see
        // `parse_captures_openrouter_reasoning_for_display_only_when_reasoning_details_present`)
        // in a tool-call turn. The outgoing body must carry no reasoning key
        // at all — byte-identical to a turn with no reasoning block, since
        // today rupu sends nothing to OpenRouter and a partial `reasoning`
        // echo (without `reasoning_details`) would create a new,
        // order-violating 400 that does not exist today.
        let reasoning_block = ContentBlock::Reasoning {
            text: Some("step by step".into()),
            provider: PROVIDER_TAG.to_string(),
            model: "openrouter/some-model".into(),
            raw: serde_json::json!({}),
        };

        let body = build_chat_request_body(
            &req(vec![assistant(vec![reasoning_block, tool_use()])]),
            false,
        );

        let msg = only_assistant_msg(&body);
        assert!(msg.get("reasoning").is_none());
        assert!(msg.get("reasoning_content").is_none());
        assert_eq!(
            msg,
            &serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Oslo\"}"}
                }]
            })
        );
    }

    #[test]
    fn reasoning_without_reasoning_details_still_echoes_as_before() {
        // vLLM (and any other backend that only ever sends `reasoning`) must
        // not regress: the absence of `reasoning_details` means there is no
        // OpenRouter-style order-sensitive contract to worry about, so the
        // existing echo behavior stands end-to-end, parse through build.
        let json = serde_json::json!({
            "id": "cmpl_vllm",
            "model": "qwen3-vllm",
            "choices": [{
                "message": {"role": "assistant", "content": "ok", "reasoning": "vllm thoughts"},
                "finish_reason": "stop"
            }]
        });
        let resp = parse_chat_completion(&json).unwrap();
        let (_text, _provider, _model, raw) = only_reasoning(&resp);
        assert_eq!(raw, &serde_json::json!({"reasoning": "vllm thoughts"}));

        let reasoning_block = resp
            .content
            .into_iter()
            .find(|b| matches!(b, ContentBlock::Reasoning { .. }))
            .expect("reasoning block");
        let body = build_chat_request_body(
            &req(vec![assistant(vec![reasoning_block, tool_use()])]),
            false,
        );
        let msg = only_assistant_msg(&body);
        assert_eq!(msg["reasoning"], serde_json::json!("vllm thoughts"));
        assert!(msg.get("reasoning_content").is_none());
    }

    #[test]
    fn foreign_provider_reasoning_is_not_echoed() {
        // An Anthropic thinking signature must never reach a
        // chat-completions endpoint.
        let body = build_chat_request_body(
            &req(vec![assistant(vec![
                ContentBlock::Reasoning {
                    text: Some("thinking".into()),
                    provider: "anthropic".into(),
                    model: "claude-sonnet-4".into(),
                    raw: serde_json::json!({"type": "thinking", "signature": "abc123"}),
                },
                ContentBlock::Text {
                    text: "hello".into(),
                },
            ])]),
            false,
        );

        assert_eq!(
            only_assistant_msg(&body),
            &serde_json::json!({"role": "assistant", "content": "hello"})
        );
        assert!(!body.to_string().contains("abc123"));
    }

    #[test]
    fn internal_fields_never_reach_the_wire() {
        let body = build_chat_request_body(
            &req(vec![assistant(vec![
                reasoning(serde_json::json!({"reasoning_content": "thinking"})),
                ContentBlock::Text {
                    text: "hello".into(),
                },
            ])]),
            false,
        );

        let msg = only_assistant_msg(&body);
        // Only the echoed key — never the block's envelope.
        for internal in ["provider", "model", "raw", "text", "type"] {
            assert!(
                msg.get(internal).is_none(),
                "internal field {internal} leaked: {msg}"
            );
        }
        let serialized = body.to_string();
        assert!(!serialized.contains(PROVIDER_TAG));
        assert!(!serialized.contains(r#""type":"reasoning""#));
    }

    #[test]
    fn reasoning_only_assistant_turn_is_not_dropped() {
        // Reasoning + tool_calls, no text: the message is still emitted (the
        // guard sees tool_calls) and the reasoning rides along.
        let body = build_chat_request_body(
            &req(vec![assistant(vec![
                reasoning(serde_json::json!({"reasoning_content": "need the weather"})),
                tool_use(),
            ])]),
            false,
        );

        let msg = only_assistant_msg(&body);
        assert_eq!(
            msg["reasoning_content"],
            serde_json::json!("need the weather")
        );
        assert_eq!(msg["content"], serde_json::json!(""));
        assert_eq!(msg["tool_calls"].as_array().expect("tool_calls").len(), 1);
    }

    #[test]
    fn reasoning_alone_does_not_resurrect_an_empty_message() {
        // No text, no tool_calls: nothing to say. Reasoning alone must not
        // conjure an assistant message that today's code would not emit.
        let body = build_chat_request_body(
            &req(vec![
                Message::user("hi"),
                assistant(vec![reasoning(
                    serde_json::json!({"reasoning_content": "thinking"}),
                )]),
            ]),
            false,
        );

        assert_eq!(
            body["messages"],
            serde_json::json!([{"role": "user", "content": "hi"}])
        );
    }

    #[test]
    fn reasoning_then_text_turn_bypasses_the_single_block_fast_path() {
        // `[Reasoning, Text]` must fall through to the general blocks arm —
        // the `[Text]` fast path must not swallow a reasoning-bearing turn.
        let body = build_chat_request_body(
            &req(vec![assistant(vec![
                reasoning(serde_json::json!({"reasoning_content": "thinking"})),
                ContentBlock::Text {
                    text: "hello".into(),
                },
            ])]),
            false,
        );

        assert_eq!(
            only_assistant_msg(&body),
            &serde_json::json!({
                "role": "assistant",
                "content": "hello",
                "reasoning_content": "thinking",
            })
        );
    }

    #[test]
    fn unknown_block_is_not_echoed() {
        let body = build_chat_request_body(
            &req(vec![assistant(vec![
                ContentBlock::Unknown,
                ContentBlock::Text {
                    text: "hello".into(),
                },
            ])]),
            false,
        );

        assert_eq!(
            only_assistant_msg(&body),
            &serde_json::json!({"role": "assistant", "content": "hello"})
        );
    }

    #[test]
    fn no_reasoning_block_leaves_body_unchanged() {
        // Backward compat: an endpoint that sends no reasoning sees exactly
        // today's body.
        let body = build_chat_request_body(
            &req(vec![
                Message::user("weather in Oslo?"),
                assistant(vec![
                    ContentBlock::Text {
                        text: "let me check".into(),
                    },
                    tool_use(),
                ]),
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "sunny".into(),
                        is_error: false,
                    }],
                },
            ]),
            false,
        );

        assert_eq!(
            body,
            serde_json::json!({
                "model": "deepseek-reasoner",
                "max_tokens": 128,
                "stream": false,
                "messages": [
                    {"role": "user", "content": "weather in Oslo?"},
                    {
                        "role": "assistant",
                        "content": "let me check",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "get_weather", "arguments": "{\"city\":\"Oslo\"}"}
                        }]
                    },
                    {"role": "tool", "tool_call_id": "call_1", "content": "sunny"},
                ]
            })
        );
    }

    #[test]
    fn non_string_reasoning_value_is_echoed_verbatim() {
        // `raw` is echoed key-for-key, whatever it holds — including a
        // non-string value. Verbatim is the contract; we do not stringify.
        let body = build_chat_request_body(
            &req(vec![assistant(vec![
                reasoning(serde_json::json!({"reasoning_content": {"nested": 1}})),
                tool_use(),
            ])]),
            false,
        );

        assert_eq!(
            only_assistant_msg(&body)["reasoning_content"],
            serde_json::json!({"nested": 1})
        );
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
