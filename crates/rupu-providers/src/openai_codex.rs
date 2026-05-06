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

fn sanitize_openai_tool_name(name: &str) -> String {
    name.replace('.', TOOL_NAME_DOT_ESCAPE)
}

fn desanitize_openai_tool_name(name: &str) -> String {
    name.replace(TOOL_NAME_DOT_ESCAPE, ".")
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
                            "output": content,
                        }));
                    }
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
                body["reasoning"] = serde_json::json!({ "effort": e });
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
                        let name =
                            desanitize_openai_tool_name(item["name"].as_str().unwrap_or(""));
                        let call_id = item["call_id"].as_str().unwrap_or("").to_string();
                        acc.current_tool_name = Some(name.clone());
                        acc.current_tool_id = Some(call_id.clone());
                        acc.current_tool_input.clear();
                        on_event(StreamEvent::ToolUseStart { id: call_id, name });
                    }
                }
            }
            "response.output_item.done" => {
                if let Some(item) = data.get("item") {
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
        // Strip the `/v1/responses` suffix (this client targets the Responses API
        // for `send`/`stream`) and append `/v1/models` for the listing endpoint.
        let base = self
            .api_url
            .trim_end_matches("/v1/responses")
            .trim_end_matches('/');
        let url = format!("{base}/v1/models");
        let resp = self
            .client
            .get(&url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.access_token),
            )
            .send()
            .await;
        let resp = match resp {
            Ok(r) if r.status().is_success() => r,
            _ => return Vec::new(),
        };
        #[derive(serde::Deserialize)]
        struct ListResp {
            data: Vec<ModelEntry>,
        }
        #[derive(serde::Deserialize)]
        struct ModelEntry {
            id: String,
        }
        let body: ListResp = match resp.json().await {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        body.data
            .into_iter()
            .map(|e| make_model_info(e.id, crate::provider_id::ProviderId::OpenaiCodex))
            .collect()
    }
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

    if let Some(output) = json.get("output").and_then(|o| o.as_array()) {
        for item in output {
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
        let creds = AuthCredentials::ApiKey { key: "sk-test".into() };
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
        assert_eq!(events.len(), 2); // two TextDelta events
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
