use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Client;
use tracing::{debug, info, warn};

use crate::auth::{is_token_expired, save_provider_auth, AuthCredentials};
use crate::error::ProviderError;
use crate::sse::SseParser;
use crate::types::*;

const DEFAULT_API_URL: &str = "https://api.openai.com/v1/responses";
const CODEX_BACKEND_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// OpenAI's Responses API enforces `^[a-zA-Z0-9_-]+$` on tool names
/// and rejects the request with HTTP 400 when any tool's name
/// contains a disallowed character. The MCP catalog uses dotted
/// names like `scm.repos.list_owned`, so we encode every `.` as
/// `__dot__` (a token no real tool name uses) on the way out and
/// decode it on the way back. The escape is reversible so the rest
/// of the agent runtime keeps using canonical (dotted) names.
const TOOL_NAME_DOT_ESCAPE: &str = "__dot__";

/// Canonical tag for the OpenAI Responses API wire format.
///
/// Deliberately distinct from `openai_wire`'s `openai_chat`: chat-completions
/// and Responses are different wire formats, and a reasoning payload from one
/// must never be echoed to the other.
pub(crate) const PROVIDER_TAG: &str = "openai_responses";

/// Concatenate the `summary_text` entries of any reasoning items in an
/// `output` array. Display only — `raw` is what goes back on the wire.
///
/// A reasoning item with an empty `summary` yields nothing here but is still
/// captured: an opaque `encrypted_content` blob is unreadable yet still
/// echo-able, and dropping it would break reasoning↔function_call pairing.
fn summary_text_from_output(items: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for item in items {
        if item["type"].as_str() != Some("reasoning") {
            continue;
        }
        let Some(summary) = item.get("summary").and_then(|s| s.as_array()) else {
            continue;
        };
        for entry in summary {
            if let Some(text) = entry.get("text").and_then(|t| t.as_str()) {
                out.push_str(text);
            }
        }
    }
    out
}

/// Build the verbatim-replay reasoning block for an assistant turn, if the
/// turn produced any output items at all.
///
/// The whole `output` array is stored — not just reasoning items — because the
/// Responses API 400s in BOTH directions on a broken pairing (`"Item 'rs_...'
/// was provided without its required following item"` and `"'function_call'
/// was provided without its required 'reasoning' item"`) and the adjacency rule
/// is undocumented. Replaying exactly what the server emitted, in order, with
/// IDs intact, is the only recipe known to work.
fn reasoning_block_from_output(
    raw_output: Vec<serde_json::Value>,
    model: &str,
    fallback_text: &str,
) -> Option<ContentBlock> {
    if raw_output.is_empty() {
        return None;
    }
    let mut summary = summary_text_from_output(&raw_output);
    if summary.is_empty() {
        // Streamed turns can carry the summary only on the deltas.
        summary.push_str(fallback_text);
    }
    Some(ContentBlock::Reasoning {
        text: if summary.is_empty() {
            None
        } else {
            Some(summary)
        },
        provider: PROVIDER_TAG.to_string(),
        model: model.to_string(),
        raw: serde_json::json!({ "output": raw_output }),
    })
}

/// Whether `model` is a reasoning model — the only kind that accepts the
/// Responses API's `reasoning` object or `include:
/// ["reasoning.encrypted_content"]`.
///
/// Non-reasoning models are a supported and in fact DEFAULT configuration on
/// this provider: `model_tier.rs` names `gpt-4.1` / `gpt-4.1-mini` as
/// `ProviderId::OpenaiCodex`'s default and fast models. Sending either
/// parameter to one is a hard `400 unsupported_parameter` ("'reasoning.summary'
/// is not supported with this model"), so the gate is fail-closed: emit nothing
/// unless the model is known to support it.
///
/// The `gpt-5` prefix is the same signal `build_request_body` already uses to
/// decide `max_output_tokens`; the o-series is added on top. Deliberately NOT
/// shared with that check — o-series models do accept `max_output_tokens`, so
/// reusing this helper there would change its behavior.
fn model_supports_reasoning(model: &str) -> bool {
    if model.starts_with("gpt-5") {
        return true;
    }
    // o-series (o1, o3, o4-mini, …): an `o` followed by a digit.
    let mut chars = model.chars();
    chars.next() == Some('o') && chars.next().is_some_and(|c| c.is_ascii_digit())
}

fn sanitize_openai_tool_name(name: &str) -> String {
    name.replace('.', TOOL_NAME_DOT_ESCAPE)
}

fn desanitize_openai_tool_name(name: &str) -> String {
    name.replace(TOOL_NAME_DOT_ESCAPE, ".")
}

fn normalize_function_call_output(content: &str) -> String {
    if content.is_empty() {
        "[tool completed with no textual output]".to_string()
    } else {
        content.to_string()
    }
}

/// OpenAI Codex client using the Responses API.
/// Translates LlmRequest/LlmResponse to/from OpenAI's Responses API format.
pub struct OpenAiCodexClient {
    client: Client,
    access_token: String,
    refresh_token: String,
    expires_ms: u64,
    account_id: String,
    api_url: String,
    auth_json_path: Option<PathBuf>,
    credential_store: Option<std::sync::Arc<dyn crate::credential_source::CredentialSource>>,
}

impl OpenAiCodexClient {
    /// Create from resolved AuthCredentials.
    pub fn new(
        creds: AuthCredentials,
        auth_json_path: Option<PathBuf>,
    ) -> Result<Self, ProviderError> {
        match creds {
            AuthCredentials::OAuth {
                access,
                refresh,
                expires,
                extra,
            } => {
                let account_id = extract_account_id(&access)
                    .or_else(|| {
                        extra
                            .get("account_id")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .or_else(|| {
                        extra
                            .get("accountId")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();

                // OAuth tokens from ChatGPT use the backend URL; allow override via extra
                let api_url = extra
                    .get("api_url")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| {
                        // If account_id present, this is a ChatGPT OAuth token → use backend URL
                        if !account_id.is_empty() {
                            CODEX_BACKEND_URL.to_string()
                        } else {
                            DEFAULT_API_URL.to_string()
                        }
                    });

                Ok(Self {
                    client: Client::new(),
                    access_token: access,
                    refresh_token: refresh,
                    expires_ms: expires,
                    account_id,
                    api_url,
                    auth_json_path,
                    credential_store: None,
                })
            }
            AuthCredentials::ApiKey { key } => Ok(Self {
                client: Client::new(),
                access_token: key,
                refresh_token: String::new(),
                expires_ms: 0,
                account_id: String::new(),
                api_url: DEFAULT_API_URL.to_string(),
                auth_json_path,
                credential_store: None,
            }),
        }
    }

    /// Non-streaming send. Uses streaming internally because the OpenAI
    /// Set the credential store for persisting refreshed tokens.
    pub fn set_credential_store(
        &mut self,
        store: std::sync::Arc<dyn crate::credential_source::CredentialSource>,
    ) {
        self.credential_store = Some(store);
    }

    /// Responses API backend requires `stream: true` for all requests.
    pub async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.stream(request, &mut |_| {}).await
    }

    /// Streaming send with SSE.
    pub async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (impl FnMut(StreamEvent) + Send + ?Sized),
    ) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = self.build_request_body(request, true);

        let response = self
            .client
            .post(&self.api_url)
            .headers(self.build_headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: truncate_error(&text, 500),
            });
        }

        let mut parser = SseParser::new();
        let mut acc = ResponseAccumulator::new();
        let mut bytes_stream = response.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
            let events = parser.feed(&chunk)?;
            for event in events {
                self.process_sse_event(&event, &mut acc, on_event)?;
            }
        }

        acc.into_response()
            .ok_or(ProviderError::UnexpectedEndOfStream)
    }

    fn build_headers(&self) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let mut headers = reqwest::header::HeaderMap::new();
        let auth_val = format!("Bearer {}", self.access_token)
            .parse()
            .map_err(|_| {
                ProviderError::AuthConfig("access token contains invalid header characters".into())
            })?;
        headers.insert(reqwest::header::AUTHORIZATION, auth_val);
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            "text/event-stream".parse().unwrap(),
        );
        if !self.account_id.is_empty() {
            if let Ok(val) = self.account_id.parse() {
                headers.insert("chatgpt-account-id", val);
            } else {
                warn!(account_id = %self.account_id, "invalid account_id, omitting header");
            }
        }
        headers.insert("OpenAI-Beta", "responses=experimental".parse().unwrap());
        headers.insert("originator", "phi".parse().unwrap());
        Ok(headers)
    }

    fn build_request_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        let mut input = Vec::new();

        for msg in &request.messages {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };

            // Replay the server's own output items verbatim. This is what keeps
            // a reasoning item adjacent to its function_call with IDs intact —
            // the Responses API 400s in BOTH directions on a broken pairing
            // ("Item 'rs_...' of type 'reasoning' was provided without its
            // required following item." and the inverse), and the adjacency rule
            // is undocumented, so replaying exactly what arrived is the only
            // recipe known to work.
            //
            // Gate on the provider tag only, never the model: a chat-completions
            // (`openai_chat`) or Anthropic payload is a different wire format
            // and must never be replayed here.
            //
            // Find the tag-matched block first without extracting `output`, so
            // the empty-array and unusable-raw cases stay distinguishable for
            // diagnostics.
            let mut replay_blocks = msg.content.iter().filter_map(|b| match b {
                ContentBlock::Reasoning { provider, raw, .. } if provider == PROVIDER_TAG => {
                    Some(raw)
                }
                _ => None,
            });
            let replay_raw = replay_blocks.next();
            // Parse inserts exactly one tag-matched block per turn, so a second
            // one means the turn was assembled by something else and we'd be
            // dropping wire items on the floor. Unreachable today; log rather
            // than drop silently, consistent with the unusable-raw paths below.
            let extra_replay_blocks = replay_blocks.count();
            if extra_replay_blocks > 0 {
                debug!(
                    extra = extra_replay_blocks,
                    "openai_responses turn carried more than one {PROVIDER_TAG:?} replay block; \
                     replaying the first and ignoring the rest"
                );
            }
            if let Some(raw) = replay_raw {
                match raw.get("output").and_then(|o| o.as_array()) {
                    Some(items) if !items.is_empty() => {
                        // `input` is a flat item list across the whole
                        // conversation, so the items go in as-is, in order.
                        //
                        // INVARIANT: a replayed `function_call` carries the
                        // server's raw `call_id`, but its matching
                        // `function_call_output` is still built below via
                        // `normalize_tool_call_id(tool_use_id)`. The two agree
                        // only while `call_id.len() <= 64` — normalize is the
                        // identity below that. Server `call_id`s are ~30 chars,
                        // so this holds today. If OpenAI ever emitted a longer
                        // one, the replayed call and its output would reference
                        // different ids and the request would 400 with "No tool
                        // output found for function call ...". Fixing it means
                        // normalizing the replayed item's `call_id` too, not
                        // un-normalizing the output.
                        input.extend(items.iter().cloned());
                        // Skip the rebuild for this turn: re-emitting the
                        // ToolUse blocks would duplicate the function_call.
                        continue;
                    }
                    Some(_) => {
                        debug!("openai_responses replay block had empty output; rebuilding from blocks");
                    }
                    None => {
                        debug!(
                            "openai_responses replay block tagged {PROVIDER_TAG:?} had unusable raw \
                             (missing or non-array \"output\"); rebuilding from blocks — a \
                             function_call may be sent without its required reasoning item and \
                             OpenAI will hard-400"
                        );
                    }
                }
            }

            // OpenAI Responses API: text goes in role messages, but
            // function_call and function_call_output are top-level input items
            let mut text_content: Vec<serde_json::Value> = Vec::new();

            for block in &msg.content {
                match block {
                    ContentBlock::Text { text } => {
                        let content_type = if msg.role == Role::User {
                            "input_text"
                        } else {
                            "output_text"
                        };
                        text_content.push(serde_json::json!({"type": content_type, "text": text}));
                    }
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input: tool_input,
                    } => {
                        // Flush any pending text content first
                        if !text_content.is_empty() {
                            input.push(serde_json::json!({"role": role, "content": text_content}));
                            text_content = Vec::new();
                        }
                        // function_call is a top-level input item
                        input.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": normalize_tool_call_id(id),
                            "name": sanitize_openai_tool_name(name),
                            "arguments": tool_input.to_string(),
                        }));
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        // Flush any pending text content first
                        if !text_content.is_empty() {
                            input.push(serde_json::json!({"role": role, "content": text_content}));
                            text_content = Vec::new();
                        }
                        // function_call_output is a top-level input item
                        input.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": normalize_tool_call_id(tool_use_id),
                            "output": normalize_function_call_output(content),
                        }));
                    }
                    // Either already consumed by the verbatim replay above, or a
                    // foreign provider's block, which is deliberately ignored.
                    ContentBlock::Reasoning { .. } => {}
                    ContentBlock::Unknown => {}
                }
            }

            // Flush remaining text content
            if !text_content.is_empty() {
                input.push(serde_json::json!({"role": role, "content": text_content}));
            }
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "store": false,
            "stream": stream,
            "input": input,
            "tool_choice": "auto",
            "parallel_tool_calls": true,
        });

        // max_output_tokens is not supported by all models (e.g., gpt-5.x)
        if !request.model.starts_with("gpt-5") {
            body["max_output_tokens"] = serde_json::json!(request.max_tokens);
        }

        if let Some(system) = &request.system {
            body["instructions"] = serde_json::json!(system);
        }

        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "name": sanitize_openai_tool_name(&t.name),
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);
        }

        // Reasoning parameters go only to reasoning models. A non-reasoning
        // model (gpt-4.1 & co — this provider's DEFAULT tier) hard-400s on
        // either key regardless of what the agent asked for, so the gate is on
        // the model, not on `thinking`.
        if model_supports_reasoning(&request.model) {
            // `summary: "auto"` is the analogue of Anthropic's
            // display:"summarized" — without it `summary` comes back empty and
            // the transcript has nothing readable. Not conditional on an
            // explicit effort: reasoning capture must still happen on the
            // `Auto` / unset paths (which the runner and step factory actually
            // use), and those deliberately send no `effort` at all.
            //
            // NOTE: OpenAI may require organization verification before
            // summaries are available on its latest reasoning models.
            body["reasoning"] = serde_json::json!({ "summary": "auto" });

            // `include: ["reasoning.encrypted_content"]` is now legacy
            // (stateless mode returns encrypted_content by default) but is
            // still accepted, and openai/codex sends it — keep it for
            // Azure/proxy backends that haven't adopted the new default.
            body["include"] = serde_json::json!(["reasoning.encrypted_content"]);

            // Reasoning effort for o-series and GPT-5.x. `Auto` omits the
            // field so the server picks; OpenAI doesn't accept "auto" as a
            // value but treats absence as adaptive.
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
                    body["reasoning"]["effort"] = serde_json::json!(e);
                }
            }
        }

        // Cross-provider `output_format` mapping for OpenAI's
        // Responses API. We only know how to express the JSON
        // variant — `text` is the implicit default and emitting it
        // explicitly adds no value (and could conflict if OpenAI
        // ever introduces a stricter "free-form text" mode).
        if let Some(crate::types::OutputFormat::Json) = request.output_format {
            body["text"] = serde_json::json!({ "format": { "type": "json_object" } });
        }

        body
    }

    fn process_sse_event(
        &self,
        event: &crate::sse::SseEvent,
        acc: &mut ResponseAccumulator,
        on_event: &mut (impl FnMut(StreamEvent) + ?Sized),
    ) -> Result<(), ProviderError> {
        // OpenAI SSE: bare data: lines → event_type defaults to "message"
        if event.data == "[DONE]" {
            return Ok(());
        }

        let data: serde_json::Value = serde_json::from_str(&event.data)?;
        let event_type = data["type"].as_str().unwrap_or("");

        match event_type {
            "response.created" => {
                if let Some(resp) = data.get("response") {
                    acc.id = resp["id"].as_str().unwrap_or("").to_string();
                    acc.model = resp["model"].as_str().unwrap_or("").to_string();
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = data["delta"].as_str() {
                    acc.text.push_str(delta);
                    on_event(StreamEvent::TextDelta(delta.to_string()));
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(delta) = data["delta"].as_str() {
                    acc.current_tool_input.push_str(delta);
                    on_event(StreamEvent::InputJsonDelta(delta.to_string()));
                }
            }
            "response.output_item.added" => {
                if let Some(item) = data.get("item") {
                    if item["type"].as_str() == Some("function_call") {
                        let name = desanitize_openai_tool_name(item["name"].as_str().unwrap_or(""));
                        let call_id = item["call_id"].as_str().unwrap_or("").to_string();
                        acc.current_tool_name = Some(name.clone());
                        acc.current_tool_id = Some(call_id.clone());
                        acc.current_tool_input.clear();
                        on_event(StreamEvent::ToolUseStart { id: call_id, name });
                    }
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(delta) = data["delta"].as_str() {
                    acc.reasoning_summary.push_str(delta);
                    on_event(StreamEvent::ReasoningDelta(delta.to_string()));
                }
            }
            "response.output_item.done" => {
                if let Some(item) = data.get("item") {
                    // Capture every item type verbatim. `encrypted_content`
                    // arrives here on the reasoning item — not on the text
                    // deltas — so filtering to `function_call` (as this arm
                    // used to) dropped it entirely.
                    acc.raw_output.push(item.clone());

                    if item["type"].as_str() == Some("function_call") {
                        // Finalize tool call
                        if let (Some(id), Some(name)) =
                            (acc.current_tool_id.take(), acc.current_tool_name.take())
                        {
                            let input_str = std::mem::take(&mut acc.current_tool_input);
                            let input: serde_json::Value = serde_json::from_str(&input_str)
                                .map_err(|e| {
                                    ProviderError::Json(format!(
                                        "malformed tool arguments for '{name}': {e}"
                                    ))
                                })?;
                            acc.content_blocks
                                .push(ContentBlock::ToolUse { id, name, input });
                        }
                    }
                }
            }
            "response.completed" => {
                if let Some(resp) = data.get("response") {
                    // Extract usage
                    if let Some(usage) = resp.get("usage") {
                        acc.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0) as u32;
                        acc.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0) as u32;
                        on_event(StreamEvent::UsageSnapshot(Usage {
                            input_tokens: acc.input_tokens,
                            output_tokens: acc.output_tokens,
                            cached_tokens: 0,
                        }));
                    }
                    // Extract stop reason
                    let status = resp["status"].as_str().unwrap_or("completed");
                    acc.stop_reason = match status {
                        "completed" => Some(StopReason::EndTurn),
                        "incomplete" => Some(StopReason::MaxTokens),
                        _ => Some(StopReason::EndTurn),
                    };
                    // Check if any output items have tool use
                    if let Some(output) = resp.get("output").and_then(|o| o.as_array()) {
                        for item in output {
                            if item["type"].as_str() == Some("function_call") {
                                acc.stop_reason = Some(StopReason::ToolUse);
                                break;
                            }
                        }
                    }
                }
            }
            "response.failed" => {
                let error_msg = data
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("response failed (no details)");
                return Err(ProviderError::Api {
                    status: 500,
                    message: error_msg.to_string(),
                });
            }
            _ => {
                debug!(event_type, "ignoring OpenAI SSE event");
            }
        }

        Ok(())
    }

    async fn ensure_valid_token(&mut self) -> Result<(), ProviderError> {
        if self.refresh_token.is_empty() || !is_token_expired(self.expires_ms) {
            return Ok(());
        }

        info!("refreshing OpenAI OAuth token");

        // OpenAI token endpoint accepts JSON, not form-urlencoded
        // (matches the Codex CLI's request_chatgpt_token_refresh implementation)
        let response = self
            .client
            .post(OPENAI_TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "client_id": OPENAI_CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": &self.refresh_token,
            }))
            .send()
            .await
            .map_err(|e| ProviderError::TokenRefreshFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::TokenRefreshFailed(format!(
                "HTTP {status}: {}",
                truncate_error(&body, 500)
            )));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::TokenRefreshFailed(e.to_string()))?;

        self.access_token = body["access_token"]
            .as_str()
            .ok_or_else(|| ProviderError::TokenRefreshFailed("missing access_token".into()))?
            .to_string();

        if let Some(rt) = body["refresh_token"].as_str() {
            self.refresh_token = rt.to_string();
        }

        let expires_in_secs = body["expires_in"].as_u64().unwrap_or(3600);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.expires_ms = now_ms + (expires_in_secs * 1000);

        // Update account_id from new token
        if let Some(id) = extract_account_id(&self.access_token) {
            self.account_id = id;
        }

        info!("OpenAI token refreshed, expires in {expires_in_secs}s");

        // Persist refreshed credentials via CredentialStore or legacy path
        let mut extra = HashMap::new();
        if !self.account_id.is_empty() {
            extra.insert(
                "account_id".to_string(),
                serde_json::Value::String(self.account_id.clone()),
            );
        }
        let creds = AuthCredentials::OAuth {
            access: self.access_token.clone(),
            refresh: self.refresh_token.clone(),
            expires: self.expires_ms,
            extra,
        };
        if let Some(ref store) = self.credential_store {
            if let Err(e) = store.update(crate::provider_id::ProviderId::OpenaiCodex, creds) {
                warn!(error = %e, "failed to persist refreshed OpenAI credentials via store");
            }
        } else if let Some(ref path) = self.auth_json_path {
            if let Err(e) =
                save_provider_auth(path, crate::provider_id::ProviderId::OpenaiCodex, &creds)
            {
                warn!(error = %e, "failed to persist refreshed OpenAI credentials");
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl crate::provider::LlmProvider for OpenAiCodexClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        OpenAiCodexClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        OpenAiCodexClient::stream(self, request, on_event).await
    }

    fn default_model(&self) -> &str {
        "gpt-5.4"
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        crate::provider_id::ProviderId::OpenaiCodex
    }

    async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
        // Two backends, two listing-endpoint shapes:
        // - api-key path: `https://api.openai.com/v1/responses` →
        //   strip `/v1/responses`, append `/v1/models`. Standard
        //   OpenAI listing endpoint, returns `{ "data": [...] }`.
        // - ChatGPT OAuth path: `https://chatgpt.com/backend-api/
        //   codex/responses` → swap `/responses` for `/models`. The
        //   chatgpt.com backend doesn't expose `/v1/models`; the
        //   Codex CLI uses the per-product `/codex/models` route.
        // ChatGPT-backend URLs end in `/backend-api/codex/responses`;
        // standard OpenAI URLs end in `/v1/responses`. Distinguish by
        // path suffix, not host — that way tests with localhost mocks
        // hit the same branch as the chatgpt.com production URL.
        let url = if self.api_url.contains("/backend-api/codex/responses") {
            // The chatgpt.com `/codex/models` endpoint validates a
            // mandatory `client_version` query string — without it
            // the server returns 400 with
            // `{'type': 'missing', 'loc': ('query', 'client_version')}`.
            // Any non-empty value is accepted; we send a stable
            // Codex-CLI-shaped version string so the request still
            // fingerprints as the impersonated CLI.
            format!(
                "{}?client_version=0.50.0",
                self.api_url.replace("/responses", "/models")
            )
        } else {
            let base = self
                .api_url
                .trim_end_matches("/v1/responses")
                .trim_end_matches('/');
            format!("{base}/v1/models")
        };
        let mut req = self.client.get(&url).header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.access_token),
        );
        // ChatGPT backend requires the chatgpt-account-id header on
        // every request; without it /models returns 401 even with a
        // valid bearer token.
        if !self.account_id.is_empty() {
            req = req.header("chatgpt-account-id", &self.account_id);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, url = %url, "openai list_models: HTTP error");
                return Vec::new();
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let body_preview = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>();
            tracing::warn!(
                status = status.as_u16(),
                url = %url,
                body = %body_preview,
                "openai list_models: non-2xx response",
            );
            return Vec::new();
        }
        // Read the body as text first so a parse failure can log a
        // preview — without that we just see "error decoding response
        // body" with no clue about the actual shape, which is the
        // exact debugging story we hit shipping #62.
        let body_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "openai list_models: read body failed");
                return Vec::new();
            }
        };
        let parsed: serde_json::Value = match serde_json::from_str(&body_text) {
            Ok(v) => v,
            Err(e) => {
                let preview: String = body_text.chars().take(200).collect();
                tracing::warn!(error = %e, body = %preview, "openai list_models: JSON parse failed");
                return Vec::new();
            }
        };
        let ids = extract_model_ids(&parsed);
        if ids.is_empty() {
            // No recognizable shape — log a fat preview + the
            // top-level object's keys so we can teach the parser
            // the next variant we encounter without another round
            // of debugging.
            let preview: String = body_text.chars().take(2000).collect();
            let top_keys: Vec<String> = parsed
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let first_entry_keys: Vec<String> = parsed
                .pointer("/models/0")
                .or_else(|| parsed.pointer("/data/0"))
                .and_then(|v| v.as_object())
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            tracing::warn!(
                top_keys = ?top_keys,
                first_entry_keys = ?first_entry_keys,
                body = %preview,
                "openai list_models: response had no recognizable model list"
            );
        }
        ids.into_iter()
            .map(|id| make_model_info(id, crate::provider_id::ProviderId::OpenaiCodex))
            .collect()
    }
}

/// Lenient model-id extractor for the assorted shapes OpenAI's
/// listing endpoints return. Handles all the variants we've seen:
///
/// - `{ "data":   [{"id":"gpt-5"}, ...] }` — api.openai.com /v1/models.
/// - `{ "models": [{"slug":"gpt-5","display_name":"...", ...}, ...] }`
///   — chatgpt.com `/backend-api/codex/models` (current production
///   shape, observed 2026-05). The objects carry rich metadata
///   per-model (context window, supported reasoning levels, etc.);
///   we only need the model id, which lives in `slug`.
/// - `{ "models": [{"id":"gpt-5"}, ...] }` — chatgpt.com's older
///   shape, kept for forward-compat with rollouts that revert.
/// - `{ "models": ["gpt-5", ...] }` — defensive: array of strings.
/// - `[ "gpt-5", ... ]` / `[{"id":"gpt-5"}, ...]` — defensive:
///   bare top-level array.
///
/// Object entries probe `id` first (legacy / api-key path), then
/// `slug` (current chatgpt.com), then `display_name` as a last
/// resort. Strings are taken as-is.
///
/// Free function so the unit tests below can exercise it directly
/// without spinning up an HTTP mock.
pub(crate) fn extract_model_ids(parsed: &serde_json::Value) -> Vec<String> {
    fn id_from_object(v: &serde_json::Value) -> Option<String> {
        for key in ["id", "slug", "display_name", "name"] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
        None
    }

    fn entries_from_array(arr: &[serde_json::Value]) -> Vec<String> {
        arr.iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(_) => id_from_object(v),
                _ => None,
            })
            .collect()
    }

    if let serde_json::Value::Array(arr) = parsed {
        return entries_from_array(arr);
    }
    if let Some(arr) = parsed.get("data").and_then(|v| v.as_array()) {
        let ids = entries_from_array(arr);
        if !ids.is_empty() {
            return ids;
        }
    }
    if let Some(arr) = parsed.get("models").and_then(|v| v.as_array()) {
        return entries_from_array(arr);
    }
    Vec::new()
}

// ── model_pool helper ────────────────────────────────────────────────

fn make_model_info(
    id: String,
    provider: crate::provider_id::ProviderId,
) -> crate::model_pool::ModelInfo {
    crate::model_pool::ModelInfo {
        id,
        provider,
        context_window: 0,
        max_output_tokens: 0,
        capabilities: Vec::new(),
        cost: crate::model_pool::ModelCost::default(),
        status: crate::model_pool::ModelStatus::default(),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Extract chatgpt_account_id from an OpenAI JWT access token.
fn extract_account_id(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    // JWT payload is base64url-encoded
    let payload = base64_decode_jwt(parts[1])?;
    let json: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    json.get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(|id| id.as_str())
        .map(String::from)
}

/// Base64url decode (JWT uses URL-safe base64 without padding).
fn base64_decode_jwt(input: &str) -> Option<Vec<u8>> {
    // Add padding if needed
    let padded = match input.len() % 4 {
        2 => format!("{input}=="),
        3 => format!("{input}="),
        _ => input.to_string(),
    };
    // URL-safe → standard base64
    let standard = padded.replace('-', "+").replace('_', "/");
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &standard).ok()
}

/// Normalize a tool call ID to fit within 64 characters.
/// OpenAI uses long composite IDs like "call_xxx|fc_xxx".
/// Uses a stable hex encoding (no non-deterministic hashing).
fn normalize_tool_call_id(id: &str) -> String {
    if id.len() <= 64 {
        return id.to_string();
    }
    // Stable hash: take first 20 chars + hex-encode last 16 bytes for uniqueness
    let prefix = &id[..20.min(id.len())];
    let suffix_bytes = id.as_bytes();
    // Simple stable hash: sum pairs of bytes into hex
    let mut hash: u64 = 0;
    for (i, &b) in suffix_bytes.iter().enumerate() {
        hash = hash
            .wrapping_mul(31)
            .wrapping_add(b as u64)
            .wrapping_add(i as u64);
    }
    let sanitized_prefix: String = prefix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    format!("fc_{sanitized_prefix}_{hash:016x}")
}

/// Parse a complete (non-streaming) Responses API response into LlmResponse.
#[allow(dead_code)]
fn parse_response(json: &serde_json::Value) -> Result<LlmResponse, ProviderError> {
    let id = json["id"].as_str().unwrap_or("").to_string();
    let model = json["model"].as_str().unwrap_or("").to_string();

    let mut content = Vec::new();
    let mut stop_reason = Some(StopReason::EndTurn);
    let mut raw_output: Vec<serde_json::Value> = Vec::new();

    if let Some(output) = json.get("output").and_then(|o| o.as_array()) {
        for item in output {
            // Capture every item verbatim, whatever its type — the replay has
            // to reproduce the server's own array exactly (see
            // `reasoning_block_from_output`).
            raw_output.push(item.clone());

            match item["type"].as_str() {
                Some("message") => {
                    if let Some(blocks) = item.get("content").and_then(|c| c.as_array()) {
                        for block in blocks {
                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                content.push(ContentBlock::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let call_id = item["call_id"].as_str().unwrap_or("").to_string();
                    let name = desanitize_openai_tool_name(item["name"].as_str().unwrap_or(""));
                    let args_str = item["arguments"].as_str().unwrap_or("{}");
                    let input: serde_json::Value = serde_json::from_str(args_str).map_err(|e| {
                        ProviderError::Json(format!("malformed tool arguments for '{}': {e}", name))
                    })?;
                    content.push(ContentBlock::ToolUse {
                        id: call_id,
                        name,
                        input,
                    });
                    stop_reason = Some(StopReason::ToolUse);
                }
                _ => {}
            }
        }
    }

    if let Some(block) = reasoning_block_from_output(raw_output, &model, "") {
        content.insert(0, block);
    }

    let status = json["status"].as_str().unwrap_or("completed");
    if status == "incomplete" {
        stop_reason = Some(StopReason::MaxTokens);
    }

    let usage = if let Some(u) = json.get("usage") {
        Usage {
            input_tokens: u["input_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: u["output_tokens"].as_u64().unwrap_or(0) as u32,
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

/// Truncate an error message to a maximum length (UTF-8 safe).
fn truncate_error(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        // Find last valid char boundary at or before max_len
        let end = (0..=max_len)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        format!("{}...", &text[..end])
    }
}

/// Stream accumulator for building LlmResponse from SSE events.
struct ResponseAccumulator {
    id: String,
    model: String,
    text: String,
    content_blocks: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    input_tokens: u32,
    output_tokens: u32,
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    current_tool_input: String,
    /// Every `response.output_item.done` item, verbatim and in arrival order.
    raw_output: Vec<serde_json::Value>,
    /// Streamed `reasoning_summary_text` deltas — display fallback only.
    reasoning_summary: String,
}

impl ResponseAccumulator {
    fn new() -> Self {
        Self {
            id: String::new(),
            model: String::new(),
            text: String::new(),
            content_blocks: Vec::new(),
            stop_reason: None,
            input_tokens: 0,
            output_tokens: 0,
            current_tool_id: None,
            current_tool_name: None,
            current_tool_input: String::new(),
            raw_output: Vec::new(),
            reasoning_summary: String::new(),
        }
    }

    fn into_response(self) -> Option<LlmResponse> {
        if self.id.is_empty() && self.text.is_empty() && self.content_blocks.is_empty() {
            return None;
        }
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        content.extend(self.content_blocks);

        if let Some(block) =
            reasoning_block_from_output(self.raw_output, &self.model, &self.reasoning_summary)
        {
            content.insert(0, block);
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

#[cfg(test)]
mod llm_provider_impl_tests {
    use super::*;
    use crate::auth::AuthCredentials;
    use crate::provider::LlmProvider;
    use crate::provider_id::ProviderId;

    #[test]
    fn implements_llm_provider_trait() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let client = OpenAiCodexClient::new(creds, None).expect("new");
        // The trait object cast must succeed. If `OpenAiCodexClient`
        // does not impl `LlmProvider`, this fails to compile.
        let boxed: Box<dyn LlmProvider> = Box::new(client);
        assert_eq!(boxed.provider_id(), ProviderId::OpenaiCodex);
        assert!(!boxed.default_model().is_empty());
    }
}

#[cfg(test)]
mod tool_name_sanitize_tests {
    use super::*;

    #[test]
    fn sanitize_replaces_dots_with_escape() {
        assert_eq!(
            sanitize_openai_tool_name("scm.repos.list_owned"),
            "scm__dot__repos__dot__list_owned",
        );
    }

    #[test]
    fn sanitize_is_a_noop_for_clean_names() {
        assert_eq!(sanitize_openai_tool_name("read_file"), "read_file");
        assert_eq!(sanitize_openai_tool_name("write-file"), "write-file");
    }

    #[test]
    fn round_trip_is_identity() {
        for name in [
            "read_file",
            "scm.repos.list_owned",
            "issues.update",
            "github.workflows_dispatch",
            "weird-but-valid-name",
        ] {
            let escaped = sanitize_openai_tool_name(name);
            assert_eq!(desanitize_openai_tool_name(&escaped), name, "round-trip");
        }
    }

    #[test]
    fn sanitized_names_pass_openai_regex() {
        // OpenAI rejects tool names not matching ^[a-zA-Z0-9_-]+$.
        // Confirm the escape produces compliant strings for every
        // dotted name in the live MCP catalog shape.
        let allowed = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        for input in [
            "scm.repos.list_owned",
            "scm.branches.list",
            "scm.files.read",
            "scm.prs.create",
            "issues.list_open",
            "github.workflows_dispatch",
            "gitlab.pipeline_trigger",
        ] {
            let out = sanitize_openai_tool_name(input);
            assert!(out.chars().all(allowed), "non-compliant char in {out}");
        }
    }

    #[test]
    fn build_request_body_emits_sanitized_tool_name() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let client = OpenAiCodexClient::new(creds, None).expect("new");
        let req = LlmRequest {
            model: "gpt-5".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 10,
            tools: vec![ToolDefinition {
                name: "scm.repos.list_owned".into(),
                description: "list owned repos".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
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
        };
        let body = client.build_request_body(&req, false);
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "scm__dot__repos__dot__list_owned");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_body_basic() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = "http://test".into();

        let request = LlmRequest {
            model: "gpt-4.1".into(),
            system: Some("Be helpful.".into()),
            messages: vec![Message::user("Hello")],
            max_tokens: 1024,
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
        };

        let body = client.build_request_body(&request, true);
        assert_eq!(body["model"], "gpt-4.1");
        assert_eq!(body["instructions"], "Be helpful.");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert!(body["input"].as_array().unwrap().len() == 1);
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = "http://test".into();

        let request = LlmRequest {
            model: "gpt-4.1".into(),
            system: None,
            messages: vec![Message::user("read file")],
            max_tokens: 1024,
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }],
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
        };

        let body = client.build_request_body(&request, false);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "read_file");
        assert_eq!(tools[0]["type"], "function");
    }

    #[test]
    fn test_build_request_body_with_reasoning() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = "http://test".into();

        let request = LlmRequest {
            model: "o3".into(),
            system: None,
            messages: vec![Message::user("think")],
            max_tokens: 8000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::High),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };

        let body = client.build_request_body(&request, false);
        assert_eq!(body["reasoning"]["effort"], "high");
    }

    #[test]
    fn test_build_request_body_reasoning_max() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = "http://test".into();

        let request = LlmRequest {
            model: "gpt-5.2".into(),
            system: None,
            messages: vec![Message::user("deep")],
            max_tokens: 32000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Max),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };

        let body = client.build_request_body(&request, false);
        assert_eq!(body["reasoning"]["effort"], "xhigh");
    }

    #[test]
    fn test_parse_response_text() {
        let json = serde_json::json!({
            "id": "resp_123",
            "model": "gpt-4.1",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Hello!"}]
            }],
            "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}
        });

        let response = parse_response(&json).unwrap();
        assert_eq!(response.id, "resp_123");
        assert_eq!(response.model, "gpt-4.1");
        assert_eq!(response.text(), Some("Hello!"));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
    }

    #[test]
    fn test_parse_response_tool_call() {
        let json = serde_json::json!({
            "id": "resp_456",
            "model": "gpt-4.1",
            "status": "completed",
            "output": [{
                "type": "function_call",
                "call_id": "call_abc",
                "name": "read_file",
                "arguments": "{\"path\":\"/tmp/test.txt\"}"
            }],
            "usage": {"input_tokens": 20, "output_tokens": 10}
        });

        let response = parse_response(&json).unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        let tools = response.tool_calls();
        assert_eq!(tools.len(), 1);
        match &tools[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/test.txt");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_parse_response_incomplete() {
        let json = serde_json::json!({
            "id": "resp_789",
            "model": "gpt-4.1",
            "status": "incomplete",
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "partial"}]}],
            "usage": {"input_tokens": 5, "output_tokens": 100}
        });

        let response = parse_response(&json).unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::MaxTokens));
    }

    #[test]
    fn test_normalize_tool_call_id_short() {
        let id = "call_abc123";
        assert_eq!(normalize_tool_call_id(id), "call_abc123");
    }

    #[test]
    fn test_normalize_tool_call_id_long() {
        let id = "a".repeat(100);
        let normalized = normalize_tool_call_id(&id);
        assert!(normalized.len() <= 64);
        assert!(normalized.starts_with("fc_"));
    }

    #[test]
    fn test_normalize_tool_call_id_deterministic() {
        let id = "a".repeat(100);
        assert_eq!(normalize_tool_call_id(&id), normalize_tool_call_id(&id));
    }

    #[test]
    fn test_extract_account_id_valid_jwt() {
        // Build a valid JWT with the account ID claim
        let header = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            r#"{"alg":"HS256","typ":"JWT"}"#,
        );
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acc_test123"}}"#,
        );
        let token = format!("{header}.{payload}.signature");
        assert_eq!(extract_account_id(&token), Some("acc_test123".to_string()));
    }

    #[test]
    fn test_extract_account_id_invalid_token() {
        assert_eq!(extract_account_id("not-a-jwt"), None);
        assert_eq!(extract_account_id(""), None);
    }

    #[test]
    fn test_sse_text_streaming() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let client = OpenAiCodexClient::new(creds, None).unwrap();
        let mut acc = ResponseAccumulator::new();
        let mut events = Vec::new();

        // Simulate SSE events
        let created = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.created","response":{"id":"resp_1","model":"gpt-4.1","status":"in_progress"}}"#.into(),
        };
        client
            .process_sse_event(&created, &mut acc, &mut |e| events.push(format!("{e:?}")))
            .unwrap();

        let delta1 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.output_text.delta","delta":"Hello "}"#.into(),
        };
        client
            .process_sse_event(&delta1, &mut acc, &mut |e| events.push(format!("{e:?}")))
            .unwrap();

        let delta2 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.output_text.delta","delta":"world!"}"#.into(),
        };
        client
            .process_sse_event(&delta2, &mut acc, &mut |e| events.push(format!("{e:?}")))
            .unwrap();

        let completed = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.completed","response":{"id":"resp_1","model":"gpt-4.1","status":"completed","output":[],"usage":{"input_tokens":10,"output_tokens":5}}}"#.into(),
        };
        client
            .process_sse_event(&completed, &mut acc, &mut |e| events.push(format!("{e:?}")))
            .unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.id, "resp_1");
        assert_eq!(response.text(), Some("Hello world!"));
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(events.len(), 3);
        assert!(events.iter().any(
            |event| event.contains("UsageSnapshot(Usage { input_tokens: 10, output_tokens: 5")
        ));
    }

    #[test]
    fn test_sse_tool_call_streaming() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let client = OpenAiCodexClient::new(creds, None).unwrap();
        let mut acc = ResponseAccumulator::new();
        let mut events = Vec::new();

        let created = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.created","response":{"id":"resp_2","model":"gpt-4.1","status":"in_progress"}}"#.into(),
        };
        client
            .process_sse_event(&created, &mut acc, &mut |_| {})
            .unwrap();

        let item_added = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_1","name":"read_file"}}"#.into(),
        };
        client
            .process_sse_event(&item_added, &mut acc, &mut |e| {
                events.push(format!("{e:?}"))
            })
            .unwrap();

        let args_delta = crate::sse::SseEvent {
            event_type: "message".into(),
            data:
                r#"{"type":"response.function_call_arguments.delta","delta":"{\"path\":\"/tmp\"}"}"#
                    .into(),
        };
        client
            .process_sse_event(&args_delta, &mut acc, &mut |e| {
                events.push(format!("{e:?}"))
            })
            .unwrap();

        let item_done = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"/tmp\"}"}}"#.into(),
        };
        client
            .process_sse_event(&item_done, &mut acc, &mut |_| {})
            .unwrap();

        let completed = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.completed","response":{"id":"resp_2","model":"gpt-4.1","status":"completed","output":[{"type":"function_call"}],"usage":{"input_tokens":15,"output_tokens":8}}}"#.into(),
        };
        client
            .process_sse_event(&completed, &mut acc, &mut |_| {})
            .unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        let tools = response.tool_calls();
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn test_sse_failed_event() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let client = OpenAiCodexClient::new(creds, None).unwrap();
        let mut acc = ResponseAccumulator::new();

        let failed = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.failed","response":{"id":"resp_3","status":"failed","error":{"type":"server_error","message":"LLM request failed"}}}"#.into(),
        };
        let result = client.process_sse_event(&failed, &mut acc, &mut |_| {});
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("LLM request failed"));
    }

    #[test]
    fn test_sse_done_signal() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let client = OpenAiCodexClient::new(creds, None).unwrap();
        let mut acc = ResponseAccumulator::new();

        let done = crate::sse::SseEvent {
            event_type: "message".into(),
            data: "[DONE]".into(),
        };
        let result = client.process_sse_event(&done, &mut acc, &mut |_| {});
        assert!(result.is_ok());
    }

    #[test]
    fn test_message_conversion_user() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = "http://test".into();

        let request = LlmRequest {
            model: "gpt-4.1".into(),
            system: None,
            messages: vec![
                Message::user("Hello"),
                Message::assistant("Hi there!"),
                Message::user("How are you?"),
            ],
            max_tokens: 1024,
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
        };

        let body = client.build_request_body(&request, false);
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
    }

    #[test]
    fn test_tool_result_conversion() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = "http://test".into();

        let request = LlmRequest {
            model: "gpt-4.1".into(),
            system: None,
            messages: vec![Message::tool_result("call_abc", "file contents", false)],
            max_tokens: 1024,
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
        };

        let body = client.build_request_body(&request, false);
        let input = body["input"].as_array().unwrap();
        // function_call_output is a top-level input item, not nested in content
        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_abc");
        assert_eq!(input[0]["output"], "file contents");
    }

    #[test]
    fn test_empty_tool_result_conversion_uses_non_empty_placeholder() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = "http://test".into();

        let request = LlmRequest {
            model: "gpt-4.1".into(),
            system: None,
            messages: vec![Message::tool_result("call_empty", "", false)],
            max_tokens: 1024,
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
        };

        let body = client.build_request_body(&request, false);
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_empty");
        assert_eq!(
            input[0]["output"],
            "[tool completed with no textual output]"
        );
    }

    #[test]
    fn test_multi_turn_tool_use_conversion() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = "http://test".into();

        // Simulate: user asks → assistant calls tool → tool result → next turn
        let request = LlmRequest {
            model: "gpt-5.3-codex".into(),
            system: Some("test".into()),
            messages: vec![
                Message::user("what files?"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Text {
                            text: "Let me check.".into(),
                        },
                        ContentBlock::ToolUse {
                            id: "call_123".into(),
                            name: "list_files".into(),
                            input: serde_json::json!({"dir": "."}),
                        },
                    ],
                },
                Message::tool_result("call_123", "file1.rs\nfile2.rs", false),
            ],
            max_tokens: 1024,
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
        };

        let body = client.build_request_body(&request, true);
        let input = body["input"].as_array().unwrap();

        // input[0]: user message
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");

        // input[1]: assistant text (flushed before tool_use)
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[1]["content"][0]["text"], "Let me check.");

        // input[2]: function_call (top-level, not nested)
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_123");
        assert_eq!(input[2]["name"], "list_files");

        // input[3]: function_call_output (top-level, not nested)
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_123");
    }

    #[test]
    fn test_process_sse_event_malformed_json_returns_error() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let client = OpenAiCodexClient::new(creds, None).unwrap();
        let mut acc = ResponseAccumulator::new();

        let bad_event = crate::sse::SseEvent {
            event_type: "message".into(),
            data: "this is not json".into(),
        };
        let result = client.process_sse_event(&bad_event, &mut acc, &mut |_| {});
        assert!(result.is_err());
    }

    #[test]
    fn test_sse_failed_event_without_response_key() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let client = OpenAiCodexClient::new(creds, None).unwrap();
        let mut acc = ResponseAccumulator::new();

        let malformed = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"type":"response.failed"}"#.into(),
        };
        let result = client.process_sse_event(&malformed, &mut acc, &mut |_| {});
        assert!(result.is_err(), "response.failed must always return error");
    }

    #[test]
    fn test_truncate_error_multibyte_utf8() {
        let text = "错误信息很长很长很长很长很长很长";
        let truncated = truncate_error(text, 10);
        assert!(truncated.ends_with("..."));
        // Must not panic — the important thing is it doesn't crash
    }

    #[tokio::test]
    async fn list_models_parses_response() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "data": [
                        { "id": "gpt-5", "object": "model" },
                        { "id": "gpt-4o", "object": "model" }
                    ]
                }));
        });
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        // Override api_url so list_models hits the mock instead of api.openai.com.
        // The list_models implementation strips the /responses suffix and appends /models,
        // so set api_url to "<server>/v1/responses".
        client.api_url = format!("{}/v1/responses", server.url(""));
        let models =
            <OpenAiCodexClient as crate::provider::LlmProvider>::list_models(&client).await;
        assert!(models.iter().any(|m| m.id == "gpt-5"));
        assert!(models.iter().any(|m| m.id == "gpt-4o"));
    }

    #[tokio::test]
    async fn list_models_chatgpt_oauth_path_swaps_responses_for_models() {
        use httpmock::prelude::*;
        // The ChatGPT backend doesn't have /v1/models — Codex CLI
        // uses the per-product /codex/models route instead.
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET)
                .path("/backend-api/codex/models")
                .header("chatgpt-account-id", "acct_1");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "data": [
                        { "id": "gpt-5-2025-08-07" },
                        { "id": "gpt-5-mini" }
                    ]
                }));
        });
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        // Mimic the ChatGPT-OAuth state: backend URL + non-empty
        // account_id. The list_models impl branches on the URL.
        client.api_url = format!("{}/backend-api/codex/responses", server.url(""));
        client.account_id = "acct_1".into();
        let models =
            <OpenAiCodexClient as crate::provider::LlmProvider>::list_models(&client).await;
        assert!(models.iter().any(|m| m.id == "gpt-5-2025-08-07"));
        assert!(models.iter().any(|m| m.id == "gpt-5-mini"));
    }

    #[tokio::test]
    async fn list_models_handles_models_field_alternative() {
        use httpmock::prelude::*;
        // Some chatgpt.com responses use `models` instead of `data`;
        // the parser accepts either.
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/backend-api/codex/models");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "models": [{ "id": "gpt-5" }]
                }));
        });
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = format!("{}/backend-api/codex/responses", server.url(""));
        let models =
            <OpenAiCodexClient as crate::provider::LlmProvider>::list_models(&client).await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5");
    }

    #[tokio::test]
    async fn list_models_handles_chatgpt_codex_slug_shape() {
        // Production shape (observed live 2026-05): chatgpt.com's
        // `/backend-api/codex/models` returns a `models` array of
        // rich objects keyed on `slug`, not `id`. Each entry has
        // dozens of metadata fields (context window, supported
        // reasoning levels, etc.); we only need the slug.
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/backend-api/codex/models");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "models": [
                        {
                            "slug": "gpt-5.2",
                            "display_name": "gpt-5.2",
                            "context_window": 272000,
                            "default_reasoning_level": "medium"
                        },
                        {
                            "slug": "gpt-5-mini",
                            "display_name": "gpt-5-mini",
                            "context_window": 128000
                        }
                    ]
                }));
        });
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.api_url = format!("{}/backend-api/codex/responses", server.url(""));
        let models =
            <OpenAiCodexClient as crate::provider::LlmProvider>::list_models(&client).await;
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-5.2", "gpt-5-mini"]);
    }

    #[test]
    fn extract_model_ids_handles_all_known_shapes() {
        // Top-level array of strings.
        let v = serde_json::json!(["gpt-5", "gpt-5-mini"]);
        assert_eq!(extract_model_ids(&v), vec!["gpt-5", "gpt-5-mini"]);

        // Top-level array of objects with `id`.
        let v = serde_json::json!([{"id":"gpt-5"}, {"id":"gpt-5-mini"}]);
        assert_eq!(extract_model_ids(&v), vec!["gpt-5", "gpt-5-mini"]);

        // `data` array (api.openai.com shape).
        let v = serde_json::json!({"data":[{"id":"gpt-5"},{"id":"gpt-4o"}]});
        assert_eq!(extract_model_ids(&v), vec!["gpt-5", "gpt-4o"]);

        // `models` array of slug-keyed objects (chatgpt.com 2026-05).
        let v = serde_json::json!({"models":[{"slug":"gpt-5.2"},{"slug":"gpt-5-mini"}]});
        assert_eq!(extract_model_ids(&v), vec!["gpt-5.2", "gpt-5-mini"]);

        // `models` array of strings (defensive).
        let v = serde_json::json!({"models":["gpt-5","gpt-5-mini"]});
        assert_eq!(extract_model_ids(&v), vec!["gpt-5", "gpt-5-mini"]);

        // `id` wins over `slug` when both are present.
        let v = serde_json::json!([{"id":"the-id","slug":"the-slug"}]);
        assert_eq!(extract_model_ids(&v), vec!["the-id"]);

        // Empty / unrecognized shapes return empty.
        assert!(extract_model_ids(&serde_json::json!({})).is_empty());
        assert!(extract_model_ids(&serde_json::json!({"weird":[1,2]})).is_empty());
        assert!(extract_model_ids(&serde_json::json!(null)).is_empty());
    }

    #[test]
    fn build_body_emits_response_format_when_output_format_json() {
        use crate::types::Message;
        let client = OpenAiCodexClient::new(
            crate::auth::AuthCredentials::ApiKey { key: "k".into() },
            None,
        )
        .unwrap();
        let request = LlmRequest {
            model: "gpt-5".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            output_format: Some(crate::types::OutputFormat::Json),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["text"]["format"]["type"], "json_object");
    }

    #[test]
    fn build_body_omits_response_format_when_output_format_text() {
        // We treat `text` as the implicit default and intentionally
        // do not emit it — keeps the wire payload identical to the
        // pre-feature shape for the common case.
        use crate::types::Message;
        let client = OpenAiCodexClient::new(
            crate::auth::AuthCredentials::ApiKey { key: "k".into() },
            None,
        )
        .unwrap();
        let request = LlmRequest {
            model: "gpt-5".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            output_format: Some(crate::types::OutputFormat::Text),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert!(body.get("text").is_none());
    }
}

#[cfg(test)]
mod reasoning_capture_tests {
    use super::*;
    use crate::model_tier::ThinkingLevel;
    use crate::types::Message;

    fn client() -> OpenAiCodexClient {
        OpenAiCodexClient::new(
            crate::auth::AuthCredentials::ApiKey { key: "k".into() },
            None,
        )
        .unwrap()
    }

    fn sse(data: &str) -> crate::sse::SseEvent {
        crate::sse::SseEvent {
            event_type: "message".into(),
            data: data.into(),
        }
    }

    #[test]
    fn request_includes_encrypted_content_and_summary() {
        let request = LlmRequest {
            model: "gpt-5".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            thinking: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = client().build_request_body(&request, false);

        assert_eq!(
            body["include"],
            serde_json::json!(["reasoning.encrypted_content"]),
        );
        // `summary: "auto"` is the analogue of Anthropic's
        // display:"summarized" — without it the summary array comes back
        // empty and the transcript shows nothing readable.
        assert_eq!(body["reasoning"]["summary"], "auto");
        // The existing effort mapping is untouched.
        assert_eq!(body["reasoning"]["effort"], "high");
        // store:false is deliberate (ZDR orgs force it) and unchanged.
        assert_eq!(body["store"], serde_json::json!(false));
    }

    #[test]
    fn request_sets_summary_even_without_explicit_effort() {
        // ThinkingLevel::Auto omits `reasoning.effort` (server default);
        // reasoning capture must not depend on an explicit effort.
        let request = LlmRequest {
            model: "gpt-5".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            thinking: Some(ThinkingLevel::Auto),
            ..Default::default()
        };
        let body = client().build_request_body(&request, false);
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert!(
            body["reasoning"].get("effort").is_none(),
            "Auto must not pin an effort: {}",
            body["reasoning"],
        );
        assert_eq!(
            body["include"],
            serde_json::json!(["reasoning.encrypted_content"]),
        );
    }

    #[test]
    fn request_sets_summary_when_thinking_is_unset() {
        let request = LlmRequest {
            model: "gpt-5".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            thinking: None,
            ..Default::default()
        };
        let body = client().build_request_body(&request, false);
        assert_eq!(body["reasoning"]["summary"], "auto");
    }

    // ---- model-conditional request shape ----
    //
    // `reasoning` / `include` are accepted only by reasoning models. gpt-4.1 is
    // this provider's DEFAULT model (`model_tier.rs`), so sending either key
    // unconditionally 400s every request on the default configuration.

    #[test]
    fn reasoning_model_with_auto_thinking_gets_reasoning_and_include() {
        // `Auto` is the level the agent runner actually sets.
        let request = LlmRequest {
            model: "o3".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            thinking: Some(ThinkingLevel::Auto),
            ..Default::default()
        };
        let body = client().build_request_body(&request, false);
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(
            body["include"],
            serde_json::json!(["reasoning.encrypted_content"]),
        );
    }

    #[test]
    fn non_reasoning_model_without_thinking_gets_no_reasoning_params() {
        // gpt-4.1 + thinking unset is what the orchestrator's step factory
        // sends. Neither key may appear AT ALL — not even as null.
        let request = LlmRequest {
            model: "gpt-4.1".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            thinking: None,
            ..Default::default()
        };
        let body = client().build_request_body(&request, false);
        assert!(
            body.get("reasoning").is_none(),
            "gpt-4.1 must carry no `reasoning` key: {body}",
        );
        assert!(
            body.get("include").is_none(),
            "gpt-4.1 must carry no `include` key: {body}",
        );
    }

    #[test]
    fn non_reasoning_model_ignores_explicit_thinking_level() {
        // A non-reasoning model cannot use `reasoning` regardless of what the
        // agent asked for, so an explicit effort is dropped rather than sent.
        //
        // This is a DELIBERATE behavior change: before the verbatim-replay
        // work, an explicit effort WOULD have been sent to gpt-4.1 — which was
        // already a guaranteed 400 (`unsupported_parameter`), just one only
        // reachable by explicitly setting `thinking`. The gate is on the model,
        // not on `thinking`, precisely so both paths are safe.
        let request = LlmRequest {
            model: "gpt-4.1".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            thinking: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = client().build_request_body(&request, false);
        assert!(
            body.get("reasoning").is_none(),
            "explicit effort must not resurrect `reasoning` on gpt-4.1: {body}",
        );
        assert!(
            body.get("include").is_none(),
            "explicit effort must not resurrect `include` on gpt-4.1: {body}",
        );
        // The non-reasoning path still gets max_output_tokens, unchanged.
        assert_eq!(body["max_output_tokens"], 100);
    }

    #[test]
    fn model_supports_reasoning_covers_gpt5_and_o_series() {
        for m in ["gpt-5", "gpt-5.2", "gpt-5-mini", "o1", "o3", "o4-mini"] {
            assert!(
                model_supports_reasoning(m),
                "{m} should be a reasoning model"
            );
        }
        for m in ["gpt-4.1", "gpt-4.1-mini", "gpt-4o", "gpt-4o-mini", "omni"] {
            assert!(
                !model_supports_reasoning(m),
                "{m} should NOT be a reasoning model",
            );
        }
    }

    #[test]
    fn parse_captures_reasoning_item_verbatim() {
        let output = serde_json::json!([
            {
                "type": "reasoning",
                "id": "rs_1",
                "summary": [{"type": "summary_text", "text": "planning"}],
                "encrypted_content": "enc_abc",
            },
            {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"},
        ]);
        let json = serde_json::json!({
            "id": "resp_1",
            "model": "gpt-5",
            "status": "completed",
            "output": output,
        });

        let resp = parse_response(&json).expect("parse");

        let reasoning: Vec<_> = resp
            .content
            .iter()
            .filter(|b| matches!(b, ContentBlock::Reasoning { .. }))
            .collect();
        assert_eq!(reasoning.len(), 1, "exactly one reasoning block");
        match reasoning[0] {
            ContentBlock::Reasoning {
                text,
                provider,
                model,
                raw,
            } => {
                assert_eq!(text.as_deref(), Some("planning"));
                assert_eq!(provider, PROVIDER_TAG);
                assert_eq!(model, "gpt-5");
                // The FULL original output array, verbatim: both items,
                // encrypted_content and ids intact.
                assert_eq!(raw["output"], output);
            }
            _ => unreachable!(),
        }

        // The ToolUse block is still emitted as today.
        let tools = resp.tool_calls();
        assert_eq!(tools.len(), 1);
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
    }

    #[test]
    fn parse_captures_reasoning_with_empty_summary() {
        // Unverified orgs / summary-less responses: the item is still
        // captured — an unreadable blob is still echo-able, and dropping
        // it would break pairing.
        let output = serde_json::json!([
            {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "enc_abc"},
        ]);
        let json = serde_json::json!({
            "id": "resp_1",
            "model": "gpt-5",
            "status": "completed",
            "output": output,
        });

        let resp = parse_response(&json).expect("parse");
        match &resp.content[0] {
            ContentBlock::Reasoning { text, raw, .. } => {
                assert!(text.is_none(), "no summary text to display");
                assert_eq!(raw["output"], output);
                assert_eq!(raw["output"][0]["encrypted_content"], "enc_abc");
            }
            other => panic!("expected Reasoning at index 0, got {other:?}"),
        }
    }

    #[test]
    fn parse_without_reasoning_items_still_stores_output_for_replay() {
        // Pairing is enforced in BOTH directions ("'function_call' was
        // provided without its required 'reasoning' item"), so a turn's
        // output is stored whenever there are output items at all.
        let output = serde_json::json!([
            {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"},
        ]);
        let json = serde_json::json!({
            "id": "resp_1",
            "model": "gpt-5",
            "status": "completed",
            "output": output,
        });

        let resp = parse_response(&json).expect("parse");
        match &resp.content[0] {
            ContentBlock::Reasoning { text, raw, .. } => {
                assert!(text.is_none());
                assert_eq!(raw["output"], output);
            }
            other => panic!("expected Reasoning at index 0, got {other:?}"),
        }
        assert_eq!(resp.tool_calls().len(), 1);
    }

    #[test]
    fn parse_with_empty_output_emits_no_reasoning_block() {
        let json = serde_json::json!({
            "id": "resp_1",
            "model": "gpt-5",
            "status": "completed",
            "output": [],
        });
        let resp = parse_response(&json).expect("parse");
        assert!(
            !resp
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Reasoning { .. })),
            "nothing to replay",
        );
    }

    #[test]
    fn parse_unknown_item_types_are_captured_for_replay() {
        // Never filter: an item type we don't model still has to go back
        // on the wire in order, or pairing breaks.
        let output = serde_json::json!([
            {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "e"},
            {"type": "web_search_call", "id": "ws_1", "status": "completed"},
        ]);
        let json = serde_json::json!({
            "id": "resp_1",
            "model": "gpt-5",
            "status": "completed",
            "output": output,
        });
        let resp = parse_response(&json).expect("parse");
        match &resp.content[0] {
            ContentBlock::Reasoning { raw, .. } => assert_eq!(raw["output"], output),
            other => panic!("expected Reasoning, got {other:?}"),
        }
    }

    #[test]
    fn sse_captures_reasoning_item_from_output_item_done() {
        // THE streaming bug: `encrypted_content` arrives on
        // response.output_item.done for the reasoning item, NOT on the
        // text deltas. output_item.added/done used to be filtered to
        // function_call, dropping the encrypted content entirely.
        let c = client();
        let mut acc = ResponseAccumulator::new();

        c.process_sse_event(
            &sse(r#"{"type":"response.created","response":{"id":"resp_9","model":"gpt-5"}}"#),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();

        c.process_sse_event(
            &sse(
                r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"planning"}],"encrypted_content":"enc_abc"}}"#,
            ),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();

        c.process_sse_event(
            &sse(
                r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"c1","name":"f"}}"#,
            ),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();
        c.process_sse_event(
            &sse(r#"{"type":"response.function_call_arguments.delta","delta":"{}"}"#),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();
        c.process_sse_event(
            &sse(
                r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"c1","name":"f","arguments":"{}"}}"#,
            ),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();

        let resp = acc.into_response().expect("response");

        match &resp.content[0] {
            ContentBlock::Reasoning {
                text,
                provider,
                model,
                raw,
            } => {
                assert_eq!(provider, PROVIDER_TAG);
                assert_eq!(model, "gpt-5");
                // Summary came only from the done item, not from deltas.
                assert_eq!(text.as_deref(), Some("planning"));
                assert_eq!(
                    raw["output"],
                    serde_json::json!([
                        {"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"planning"}],"encrypted_content":"enc_abc"},
                        {"type":"function_call","id":"fc_1","call_id":"c1","name":"f","arguments":"{}"},
                    ]),
                );
            }
            other => panic!("expected Reasoning at index 0, got {other:?}"),
        }

        // Existing function_call handling stays intact.
        assert_eq!(resp.tool_calls().len(), 1);
    }

    #[test]
    fn sse_emits_reasoning_delta_from_summary_text_delta() {
        let c = client();
        let mut acc = ResponseAccumulator::new();
        let mut events: Vec<StreamEvent> = Vec::new();

        c.process_sse_event(
            &sse(r#"{"type":"response.created","response":{"id":"resp_8","model":"gpt-5"}}"#),
            &mut acc,
            &mut |e| events.push(e),
        )
        .unwrap();
        c.process_sse_event(
            &sse(r#"{"type":"response.reasoning_summary_text.delta","delta":"plan"}"#),
            &mut acc,
            &mut |e| events.push(e),
        )
        .unwrap();
        c.process_sse_event(
            &sse(r#"{"type":"response.reasoning_summary_text.delta","delta":"ning"}"#),
            &mut acc,
            &mut |e| events.push(e),
        )
        .unwrap();

        let deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ReasoningDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["plan", "ning"]);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::TextDelta(_))),
            "reasoning summary must never be emitted as assistant text",
        );
        assert_eq!(acc.text, "", "reasoning must not pollute the text block");
        assert_eq!(acc.reasoning_summary, "planning");
    }

    #[test]
    fn sse_reasoning_summary_deltas_do_not_duplicate_the_done_summary() {
        // Deltas stream the same summary the done item carries whole.
        // Prefer the done item's verbatim summary as the display text.
        let c = client();
        let mut acc = ResponseAccumulator::new();

        c.process_sse_event(
            &sse(r#"{"type":"response.created","response":{"id":"resp_7","model":"gpt-5"}}"#),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();
        c.process_sse_event(
            &sse(r#"{"type":"response.reasoning_summary_text.delta","delta":"planning"}"#),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();
        c.process_sse_event(
            &sse(
                r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"planning"}],"encrypted_content":"e"}}"#,
            ),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();

        let resp = acc.into_response().expect("response");
        match &resp.content[0] {
            ContentBlock::Reasoning { text, .. } => {
                assert_eq!(text.as_deref(), Some("planning"));
            }
            other => panic!("expected Reasoning, got {other:?}"),
        }
    }

    #[test]
    fn sse_without_output_items_emits_no_reasoning_block() {
        let c = client();
        let mut acc = ResponseAccumulator::new();
        c.process_sse_event(
            &sse(r#"{"type":"response.created","response":{"id":"resp_6","model":"gpt-5"}}"#),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();
        c.process_sse_event(
            &sse(r#"{"type":"response.output_text.delta","delta":"hi"}"#),
            &mut acc,
            &mut |_| {},
        )
        .unwrap();

        let resp = acc.into_response().expect("response");
        assert!(!resp
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::Reasoning { .. })),);
        assert_eq!(resp.text(), Some("hi"));
    }

    // ---- Task 2: replay reasoning items into `input` ----

    /// The stored output items for an assistant turn that called one tool:
    /// a reasoning item immediately followed by its function_call, exactly as
    /// the Responses API emitted them.
    fn stored_output() -> serde_json::Value {
        serde_json::json!([
            {
                "type": "reasoning",
                "id": "rs_1",
                "encrypted_content": "enc_abc",
                "summary": [],
            },
            {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "c1",
                "name": "f",
                "arguments": "{}",
            },
        ])
    }

    /// An assistant turn as Task 1's parse produces it: the verbatim replay
    /// block plus the ToolUse block rupu uses for dispatch.
    fn replay_turn(raw: serde_json::Value) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: None,
                    provider: PROVIDER_TAG.to_string(),
                    model: "gpt-5".to_string(),
                    raw,
                },
                ContentBlock::ToolUse {
                    id: "c1".to_string(),
                    name: "f".to_string(),
                    input: serde_json::json!({}),
                },
            ],
        }
    }

    fn request_with(messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            model: "gpt-5".into(),
            messages,
            max_tokens: 100,
            ..Default::default()
        }
    }

    #[test]
    fn assistant_turn_replays_stored_output_items_verbatim() {
        let request = request_with(vec![
            Message::user("go"),
            replay_turn(serde_json::json!({ "output": stored_output() })),
        ]);
        let body = client().build_request_body(&request, false);
        let input = body["input"].as_array().expect("input array");

        // The user message, then both stored items verbatim.
        assert_eq!(input.len(), 3, "input items: {input:#?}");
        assert_eq!(input[1], stored_output()[0]);
        assert_eq!(input[2], stored_output()[1]);
        // encrypted_content survived the round trip.
        assert_eq!(input[1]["encrypted_content"], "enc_abc");

        // The ToolUse block must NOT be rebuilt on top of the replay, or the
        // function_call would be sent twice.
        let function_calls = input
            .iter()
            .filter(|i| i["type"] == "function_call")
            .count();
        assert_eq!(function_calls, 1, "function_call must not be duplicated");
    }

    #[test]
    fn replay_keeps_reasoning_adjacent_to_its_function_call() {
        // The Responses API 400s in BOTH directions on a broken pairing, so
        // the reasoning item must immediately precede its function_call
        // exactly as the server emitted it.
        let request = request_with(vec![
            Message::user("go"),
            replay_turn(serde_json::json!({ "output": stored_output() })),
        ]);
        let body = client().build_request_body(&request, false);
        let input = body["input"].as_array().expect("input array");

        let rs = input
            .iter()
            .position(|i| i["type"] == "reasoning")
            .expect("reasoning item present");
        let fc = input
            .iter()
            .position(|i| i["type"] == "function_call")
            .expect("function_call item present");
        assert_eq!(
            fc,
            rs + 1,
            "reasoning must immediately precede function_call"
        );
    }

    #[test]
    fn replay_preserves_item_ids() {
        // The pairing errors are expressed in terms of item IDs
        // ("Item 'rs_...' ..."), so IDs must survive verbatim.
        let request = request_with(vec![replay_turn(
            serde_json::json!({ "output": stored_output() }),
        )]);
        let body = client().build_request_body(&request, false);
        let input = body["input"].as_array().expect("input array");

        assert_eq!(input[0]["id"], "rs_1");
        assert_eq!(input[1]["id"], "fc_1");
        assert_eq!(input[1]["call_id"], "c1");
    }

    #[test]
    fn falls_back_to_rebuild_without_reasoning_block() {
        // Backward compat: today's body, unchanged.
        let request = request_with(vec![
            Message::user("go"),
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Text {
                        text: "calling".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "f".into(),
                        input: serde_json::json!({"a": 1}),
                    },
                ],
            },
        ]);
        let body = client().build_request_body(&request, false);
        let input = body["input"].as_array().expect("input array");

        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["text"], "calling");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "c1");
        assert_eq!(input[2]["arguments"], "{\"a\":1}");
    }

    #[test]
    fn foreign_provider_reasoning_is_not_replayed() {
        // `openai_chat` is Plan 3's chat-completions tag: same vendor, a
        // DIFFERENT wire format. It must never cross into the Responses API.
        for foreign in ["anthropic", "openai_chat", "google_gemini"] {
            let mut turn = replay_turn(serde_json::json!({ "output": stored_output() }));
            if let ContentBlock::Reasoning { provider, .. } = &mut turn.content[0] {
                *provider = foreign.to_string();
            }
            let request = request_with(vec![turn]);
            let body = client().build_request_body(&request, false);
            let input = body["input"].as_array().expect("input array");

            // Rebuilt from blocks: just the function_call, no replayed items.
            assert_eq!(input.len(), 1, "{foreign}: {input:#?}");
            assert_eq!(input[0]["type"], "function_call");
            assert!(
                !input.iter().any(|i| i["type"] == "reasoning"),
                "{foreign}: foreign reasoning must not be replayed"
            );
        }
    }

    #[test]
    fn malformed_raw_falls_back_to_rebuild() {
        // raw missing "output", "output" not an array, and an empty array.
        for raw in [
            serde_json::json!({}),
            serde_json::json!({ "output": "not-an-array" }),
            serde_json::json!({ "output": [] }),
        ] {
            let request = request_with(vec![replay_turn(raw.clone())]);
            let body = client().build_request_body(&request, false);
            let input = body["input"].as_array().expect("input array");

            assert_eq!(input.len(), 1, "raw {raw}: {input:#?}");
            assert_eq!(input[0]["type"], "function_call");
            assert_eq!(input[0]["call_id"], "c1");
        }
    }

    #[test]
    fn internal_fields_never_reach_the_wire() {
        let request = request_with(vec![replay_turn(
            serde_json::json!({ "output": stored_output() }),
        )]);
        let body = client().build_request_body(&request, false);
        let wire = serde_json::to_string(&body).expect("serialize");

        // No internal ContentBlock plumbing leaks into the request.
        assert!(!wire.contains("\"provider\""), "{wire}");
        assert!(!wire.contains("\"raw\""), "{wire}");
        // "model" is a legitimate top-level request field, but the Reasoning
        // block's own model must not ride along inside an input item.
        for item in body["input"].as_array().expect("input array") {
            assert!(item.get("provider").is_none(), "{item}");
            assert!(item.get("raw").is_none(), "{item}");
            assert!(item.get("model").is_none(), "{item}");
        }
    }

    /// End-to-end: real SSE capture → real `build_request_body` replay.
    ///
    /// This guards the SEAM between capture (Task 1) and replay (Task 2). Every
    /// other test on either side hand-builds the value at the boundary, so the
    /// two halves could drift apart while staying green.
    ///
    /// Replay is all-or-nothing per turn — the `continue` skips the rebuild for
    /// `Text` blocks too — so correctness depends entirely on the capture
    /// storing the WHOLE output array, `message` item included. Narrowing the
    /// `output_item.done` arm to `reasoning | function_call` (a plausible "only
    /// capture what we need" cleanup) would silently erase the model's own
    /// prior answers from history: no 400, no error, no other test failing.
    /// This one fails.
    ///
    /// Streaming is the live path — `parse_response` is `#[allow(dead_code)]`
    /// — so this composes through `process_sse_event`.
    #[test]
    fn sse_capture_composes_into_verbatim_replay() {
        let c = client();
        let mut acc = ResponseAccumulator::new();

        // A realistic turn: a text message, then reasoning, then a tool call.
        let message_item = serde_json::json!({
            "type": "message",
            "id": "msg_1",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": "on it", "annotations": []}],
        });
        let reasoning_item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "summary_text", "text": "planning"}],
            "encrypted_content": "enc_abc",
        });
        let function_call_item = serde_json::json!({
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_xyz",
            "name": "f",
            "arguments": "{\"a\":1}",
        });

        for ev in [
            serde_json::json!({"type": "response.created",
                "response": {"id": "resp_1", "model": "gpt-5"}}),
            serde_json::json!({"type": "response.output_item.added", "item": message_item}),
            serde_json::json!({"type": "response.output_text.delta", "delta": "on it"}),
            serde_json::json!({"type": "response.output_item.done", "item": message_item}),
            serde_json::json!({"type": "response.output_item.done", "item": reasoning_item}),
            serde_json::json!({"type": "response.output_item.added", "item": function_call_item}),
            serde_json::json!({"type": "response.function_call_arguments.delta",
                "delta": "{\"a\":1}"}),
            serde_json::json!({"type": "response.output_item.done", "item": function_call_item}),
        ] {
            c.process_sse_event(&sse(&ev.to_string()), &mut acc, &mut |_| {})
                .expect("sse event");
        }

        // The captured content goes STRAIGHT into the next request — no
        // hand-building. This is the seam under test.
        let response = acc.into_response().expect("response");
        let request = request_with(vec![
            Message::user("go"),
            Message {
                role: Role::Assistant,
                content: response.content,
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_xyz".into(),
                    content: "done".into(),
                    is_error: false,
                }],
            },
        ]);
        let body = c.build_request_body(&request, true);
        let input = body["input"].as_array().expect("input array");

        // user, [message, reasoning, function_call] verbatim, function_call_output.
        assert_eq!(input.len(), 5, "input items: {input:#?}");
        assert_eq!(input[1], message_item, "message item must replay verbatim");
        assert_eq!(
            input[2], reasoning_item,
            "reasoning item must replay verbatim"
        );
        assert_eq!(
            input[3], function_call_item,
            "function_call item must replay verbatim",
        );

        // Ids and the opaque blob survive the round trip intact.
        assert_eq!(input[2]["encrypted_content"], "enc_abc");
        assert_eq!(input[3]["call_id"], "call_xyz");
        assert_eq!(input[4]["type"], "function_call_output");
        assert_eq!(input[4]["call_id"], "call_xyz");

        // The rebuild must not fire on top of the replay.
        assert_eq!(
            input
                .iter()
                .filter(|i| i["type"] == "function_call")
                .count(),
            1,
            "function_call must not be duplicated: {input:#?}",
        );
    }
}
