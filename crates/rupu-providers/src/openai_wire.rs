//! Shared OpenAI `/v1/chat/completions` wire helpers.
//!
//! Pure request-build / response-parse / SSE-accumulate logic used by every
//! provider that speaks the OpenAI chat-completions dialect (GitHub Copilot,
//! the generic OpenAI-compatible client). Provider-specific concerns —
//! base URL, auth headers, token exchange — stay in the individual clients.

use crate::error::ProviderError;
use crate::types::{ContentBlock, LlmRequest, LlmResponse, Role, StopReason, StreamEvent, Usage};

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
        }
    }

    pub(crate) fn into_response(self) -> Option<LlmResponse> {
        if self.id.is_empty() && self.text.is_empty() && self.tool_calls.is_empty() {
            return None;
        }
        let mut content = Vec::new();
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
