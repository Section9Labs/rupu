use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Client;
use tracing::{info, warn};

use crate::auth::{is_token_expired, save_provider_auth, AuthCredentials};
use crate::error::ProviderError;
use crate::sse::SseParser;
use crate::types::*;

// ── Endpoint URLs ────────────────────────────────────────────────────

const GEMINI_CLI_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const ANTIGRAVITY_ENDPOINT: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";

const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

// Google's public CLI OAuth client IDs and secrets (same as Pi).
// These are embedded in all CLI tools that use Google OAuth (safe to embed).
const GEMINI_CLI_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
const GEMINI_CLI_CLIENT_SECRET: &str = "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";

const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const ANTIGRAVITY_CLIENT_SECRET: &str = "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";

/// Which Google Cloud Code Assist variant to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiVariant {
    /// Production: cloudcode-pa.googleapis.com
    GeminiCli,
    /// Sandbox: daily-cloudcode-pa.sandbox.googleapis.com
    Antigravity,
}

impl GeminiVariant {
    fn endpoint(&self) -> &'static str {
        match self {
            GeminiVariant::GeminiCli => GEMINI_CLI_ENDPOINT,
            GeminiVariant::Antigravity => ANTIGRAVITY_ENDPOINT,
        }
    }

    fn client_id(&self) -> &'static str {
        match self {
            GeminiVariant::GeminiCli => GEMINI_CLI_CLIENT_ID,
            GeminiVariant::Antigravity => ANTIGRAVITY_CLIENT_ID,
        }
    }

    fn client_secret(&self) -> &'static str {
        match self {
            GeminiVariant::GeminiCli => GEMINI_CLI_CLIENT_SECRET,
            GeminiVariant::Antigravity => ANTIGRAVITY_CLIENT_SECRET,
        }
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        match self {
            GeminiVariant::GeminiCli => crate::provider_id::ProviderId::GoogleGeminiCli,
            GeminiVariant::Antigravity => crate::provider_id::ProviderId::GoogleAntigravity,
        }
    }

    fn user_agent(&self) -> &'static str {
        match self {
            GeminiVariant::GeminiCli => "google-cloud-sdk vscode_cloudshelleditor/0.1",
            GeminiVariant::Antigravity => "antigravity/1.18.4 darwin/arm64",
        }
    }
}

/// Google Gemini client for Cloud Code Assist API.
/// Shared implementation for both Gemini CLI and Antigravity variants.
pub struct GoogleGeminiClient {
    client: Client,
    variant: GeminiVariant,
    access_token: String,
    refresh_token: String,
    expires_ms: u64,
    project_id: String,
    auth_json_path: Option<PathBuf>,
}

impl GoogleGeminiClient {
    /// Create from resolved AuthCredentials + variant.
    pub fn new(
        creds: AuthCredentials,
        variant: GeminiVariant,
        auth_json_path: Option<PathBuf>,
    ) -> Result<Self, ProviderError> {
        match creds {
            AuthCredentials::OAuth {
                access,
                refresh,
                expires,
                extra,
            } => {
                let project_id = extra
                    .get("project_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                Ok(Self {
                    client: Client::new(),
                    variant,
                    access_token: access,
                    refresh_token: refresh,
                    expires_ms: expires,
                    project_id,
                    auth_json_path,
                })
            }
            AuthCredentials::ApiKey { .. } => Err(ProviderError::AuthConfig(
                "Google Gemini requires OAuth authentication, not API key".into(),
            )),
        }
    }

    /// Non-streaming send.
    pub async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = self.build_request_body(request);
        let url = self.build_url();

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: extract_google_error(&text),
            });
        }

        let resp_json: serde_json::Value = response.json().await?;
        parse_generate_content_response(&resp_json)
    }

    /// Streaming send with SSE.
    pub async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (impl FnMut(StreamEvent) + Send + ?Sized),
    ) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = self.build_request_body(request);
        let url = self.build_stream_url();

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: extract_google_error(&text),
            });
        }

        let mut parser = SseParser::new();
        let mut acc = GeminiAccumulator::new();
        let mut bytes_stream = response.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
            let events = parser.feed(&chunk)?;
            for event in events {
                process_gemini_sse(&event, &mut acc, on_event)?;
            }
        }

        acc.into_response()
            .ok_or(ProviderError::UnexpectedEndOfStream)
    }

    fn build_url(&self) -> String {
        format!("{}/v1internal:generateContent", self.variant.endpoint())
    }

    fn build_stream_url(&self) -> String {
        format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            self.variant.endpoint()
        )
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
        headers.insert(
            reqwest::header::USER_AGENT,
            self.variant.user_agent().parse().unwrap(),
        );
        Ok(headers)
    }

    fn build_request_body(&self, request: &LlmRequest) -> serde_json::Value {
        // Convert messages to Gemini contents format
        let contents = convert_messages(&request.messages);

        let mut inner_request = serde_json::json!({
            "contents": contents,
        });

        // System instruction
        if let Some(system) = &request.system {
            inner_request["systemInstruction"] = serde_json::json!({
                "parts": [{"text": system}]
            });
        }

        // Generation config
        let mut gen_config = serde_json::json!({
            "maxOutputTokens": request.max_tokens,
        });

        // Thinking config
        if let Some(level) = &request.thinking {
            use crate::model_tier::ThinkingLevel;
            let (level_str, budget) = match level {
                ThinkingLevel::Minimal => ("MINIMAL", 128),
                ThinkingLevel::Low => ("LOW", 2048),
                ThinkingLevel::Medium => ("MEDIUM", 8192),
                ThinkingLevel::High => ("HIGH", 32768),
                ThinkingLevel::Max => ("HIGH", 32768), // clamped to High for Google
            };
            gen_config["thinkingConfig"] = serde_json::json!({
                "includeThoughts": true,
                "thinkingLevel": level_str,
                "thinkingBudget": budget,
            });
        }

        inner_request["generationConfig"] = gen_config;

        // Tools
        if !request.tools.is_empty() {
            let declarations: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();
            inner_request["tools"] = serde_json::json!([{"functionDeclarations": declarations}]);
        }

        // Wrap in outer envelope
        let user_agent = if self.variant == GeminiVariant::Antigravity {
            "antigravity"
        } else {
            "phi-coding-agent"
        };

        serde_json::json!({
            "project": self.project_id,
            "model": request.model,
            "userAgent": user_agent,
            "requestId": format!("phi-{}-{}", now_ms(), request_counter()),
            "request": inner_request,
        })
    }

    async fn ensure_valid_token(&mut self) -> Result<(), ProviderError> {
        if self.refresh_token.is_empty() || !is_token_expired(self.expires_ms) {
            return Ok(());
        }

        info!(variant = ?self.variant, "refreshing Google OAuth token");

        let response = self
            .client
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.variant.client_id()),
                ("client_secret", self.variant.client_secret()),
                ("refresh_token", &self.refresh_token),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::TokenRefreshFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::TokenRefreshFailed(format!(
                "HTTP {status}: {}",
                truncate(&body, 500)
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
        let now = now_ms();
        self.expires_ms = now + (expires_in_secs * 1000);

        info!("Google token refreshed, expires in {expires_in_secs}s");

        // Persist refreshed credentials
        if let Some(ref path) = self.auth_json_path {
            let mut extra = HashMap::new();
            if !self.project_id.is_empty() {
                extra.insert(
                    "project_id".to_string(),
                    serde_json::Value::String(self.project_id.clone()),
                );
            }
            let creds = AuthCredentials::OAuth {
                access: self.access_token.clone(),
                refresh: self.refresh_token.clone(),
                expires: self.expires_ms,
                extra,
            };
            if let Err(e) = save_provider_auth(path, self.variant.provider_id(), &creds) {
                warn!(error = %e, "failed to persist refreshed Google credentials");
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl crate::provider::LlmProvider for GoogleGeminiClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        GoogleGeminiClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        GoogleGeminiClient::stream(self, request, on_event).await
    }

    fn default_model(&self) -> &str {
        "gemini-2.5-pro"
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        self.variant.provider_id()
    }
}

// ── Message Conversion ───────────────────────────────────────────────

/// Convert LlmRequest messages to Gemini contents format.
/// Builds a tool_use_id → name lookup from the conversation history so that
/// functionResponse parts include the correct function name (required by Gemini).
fn convert_messages(messages: &[Message]) -> Vec<serde_json::Value> {
    // Build tool_use_id → name lookup from all ToolUse blocks in the history
    let mut tool_name_map: HashMap<String, String> = HashMap::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                tool_name_map.insert(id.clone(), name.clone());
            }
        }
    }

    let mut contents = Vec::new();

    for msg in messages {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "model",
        };

        let mut parts = Vec::new();

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    if !text.is_empty() {
                        parts.push(serde_json::json!({"text": text}));
                    }
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    parts.push(serde_json::json!({
                        "functionCall": {
                            "name": name,
                            "args": input,
                        }
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } => {
                    // Look up the function name from the preceding ToolUse
                    let name = tool_name_map.get(tool_use_id).cloned().unwrap_or_default();
                    let response_value = if *is_error {
                        serde_json::json!({"error": content})
                    } else {
                        serde_json::json!({"output": content})
                    };
                    parts.push(serde_json::json!({
                        "functionResponse": {
                            "name": name,
                            "response": response_value,
                        }
                    }));
                }
            }
        }

        if !parts.is_empty() {
            contents.push(serde_json::json!({"role": role, "parts": parts}));
        }
    }

    contents
}

// ── Response Parsing ─────────────────────────────────────────────────

/// Parse a complete GenerateContentResponse into LlmResponse.
fn parse_generate_content_response(json: &serde_json::Value) -> Result<LlmResponse, ProviderError> {
    let mut content = Vec::new();
    let mut stop_reason = Some(StopReason::EndTurn);
    let mut tool_call_counter: u32 = 0;

    if let Some(candidates) = json.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        // Skip thinking parts (thought: true)
                        if part.get("thought").and_then(|t| t.as_bool()) == Some(true) {
                            continue;
                        }
                        content.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc["name"].as_str().unwrap_or("").to_string();
                        let args = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
                        tool_call_counter += 1;
                        content.push(ContentBlock::ToolUse {
                            id: format!("gemini_tc_{tool_call_counter}"),
                            name,
                            input: args,
                        });
                        stop_reason = Some(StopReason::ToolUse);
                    }
                }
            }

            // Map finish reason
            if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str()) {
                stop_reason = Some(map_finish_reason(reason));
            }
        }
    }

    let usage = if let Some(meta) = json.get("usageMetadata") {
        Usage {
            input_tokens: meta["promptTokenCount"].as_u64().unwrap_or(0) as u32,
            output_tokens: meta["candidatesTokenCount"].as_u64().unwrap_or(0) as u32,
            ..Default::default()
        }
    } else {
        Usage::default()
    };

    Ok(LlmResponse {
        id: String::new(), // Gemini doesn't return a response ID in the same way
        model: String::new(),
        content,
        stop_reason,
        usage,
    })
}

/// Map Google finish reason string to StopReason.
fn map_finish_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "STOP_SEQUENCE" => StopReason::StopSequence,
        "FUNCTION_CALLING" => StopReason::ToolUse,
        _ => StopReason::EndTurn, // SAFETY, OTHER, etc. → graceful
    }
}

// ── SSE Processing ───────────────────────────────────────────────────

/// Accumulator for Gemini streaming responses.
struct GeminiAccumulator {
    text: String,
    content_blocks: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    input_tokens: u32,
    output_tokens: u32,
    tool_call_counter: u32,
}

impl GeminiAccumulator {
    fn new() -> Self {
        Self {
            text: String::new(),
            content_blocks: Vec::new(),
            stop_reason: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_counter: 0,
        }
    }

    fn into_response(self) -> Option<LlmResponse> {
        if self.text.is_empty() && self.content_blocks.is_empty() {
            return None;
        }
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        content.extend(self.content_blocks);

        Some(LlmResponse {
            id: String::new(),
            model: String::new(),
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

/// Process a Gemini SSE event.
fn process_gemini_sse(
    event: &crate::sse::SseEvent,
    acc: &mut GeminiAccumulator,
    on_event: &mut (impl FnMut(StreamEvent) + ?Sized),
) -> Result<(), ProviderError> {
    if event.data == "[DONE]" {
        return Ok(());
    }

    let data: serde_json::Value = serde_json::from_str(&event.data)?;

    // Process candidates
    if let Some(candidates) = data.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    // Skip thinking parts
                    if part.get("thought").and_then(|t| t.as_bool()) == Some(true) {
                        continue;
                    }

                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        acc.text.push_str(text);
                        on_event(StreamEvent::TextDelta(text.to_string()));
                    }

                    if let Some(fc) = part.get("functionCall") {
                        let name = fc["name"].as_str().unwrap_or("").to_string();
                        let args = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
                        acc.tool_call_counter += 1;
                        let id = format!("gemini_tc_{}", acc.tool_call_counter);
                        on_event(StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        let args_str = args.to_string();
                        on_event(StreamEvent::InputJsonDelta(args_str));
                        acc.content_blocks.push(ContentBlock::ToolUse {
                            id,
                            name,
                            input: args,
                        });
                    }
                }
            }

            if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str()) {
                acc.stop_reason = Some(map_finish_reason(reason));
            }
        }
    }

    // Process usage metadata
    if let Some(meta) = data.get("usageMetadata") {
        if let Some(input) = meta.get("promptTokenCount").and_then(|v| v.as_u64()) {
            acc.input_tokens = input as u32;
        }
        if let Some(output) = meta.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
            acc.output_tokens = output as u32;
        }
    }

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn request_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        let end = (0..=max_len)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        format!("{}...", &text[..end])
    }
}

/// Extract a clean error message from a Google API JSON error response.
fn extract_google_error(text: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
    }
    truncate(text, 500)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_creds(project_id: &str) -> AuthCredentials {
        let mut extra = HashMap::new();
        extra.insert(
            "project_id".to_string(),
            serde_json::Value::String(project_id.to_string()),
        );
        AuthCredentials::OAuth {
            access: "test-token".into(),
            refresh: "test-refresh".into(),
            expires: 9999999999999,
            extra,
        }
    }

    #[test]
    fn test_new_gemini_cli() {
        let client =
            GoogleGeminiClient::new(test_creds("my-project"), GeminiVariant::GeminiCli, None)
                .unwrap();
        assert_eq!(client.project_id, "my-project");
        assert_eq!(client.variant, GeminiVariant::GeminiCli);
    }

    #[test]
    fn test_new_antigravity() {
        let client =
            GoogleGeminiClient::new(test_creds("ag-project"), GeminiVariant::Antigravity, None)
                .unwrap();
        assert_eq!(client.project_id, "ag-project");
        assert_eq!(client.variant, GeminiVariant::Antigravity);
    }

    #[test]
    fn test_api_key_rejected() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let result = GoogleGeminiClient::new(creds, GeminiVariant::GeminiCli, None);
        match result {
            Err(e) => assert!(
                e.to_string().contains("OAuth"),
                "expected OAuth error, got: {e}"
            ),
            Ok(_) => panic!("expected error for API key auth"),
        }
    }

    #[test]
    fn test_build_request_body_basic() {
        let client =
            GoogleGeminiClient::new(test_creds("proj"), GeminiVariant::GeminiCli, None).unwrap();

        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: Some("Be helpful.".into()),
            messages: vec![Message::user("Hello")],
            max_tokens: 4096,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        };

        let body = client.build_request_body(&request);
        assert_eq!(body["project"], "proj");
        assert_eq!(body["model"], "gemini-2.5-pro");
        assert_eq!(body["userAgent"], "phi-coding-agent");

        let inner = &body["request"];
        assert!(inner["contents"].as_array().unwrap().len() == 1);
        assert_eq!(
            inner["systemInstruction"]["parts"][0]["text"],
            "Be helpful."
        );
        assert_eq!(inner["generationConfig"]["maxOutputTokens"], 4096);
    }

    #[test]
    fn test_build_request_body_antigravity_user_agent() {
        let client =
            GoogleGeminiClient::new(test_creds("proj"), GeminiVariant::Antigravity, None).unwrap();

        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("Hi")],
            max_tokens: 1024,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        };

        let body = client.build_request_body(&request);
        assert_eq!(body["userAgent"], "antigravity");
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let client =
            GoogleGeminiClient::new(test_creds("proj"), GeminiVariant::GeminiCli, None).unwrap();

        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("read file")],
            max_tokens: 4096,
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }],
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        };

        let body = client.build_request_body(&request);
        let tools = body["request"]["tools"][0]["functionDeclarations"]
            .as_array()
            .unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "read_file");
    }

    #[test]
    fn test_build_request_body_with_thinking() {
        let client =
            GoogleGeminiClient::new(test_creds("proj"), GeminiVariant::GeminiCli, None).unwrap();

        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("think hard")],
            max_tokens: 16000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Medium),
            task_type: None,
        };

        let body = client.build_request_body(&request);
        let config = &body["request"]["generationConfig"]["thinkingConfig"];
        assert_eq!(config["includeThoughts"], true);
        assert_eq!(config["thinkingLevel"], "MEDIUM");
        assert_eq!(config["thinkingBudget"], 8192);
    }

    #[test]
    fn test_build_request_body_thinking_max_clamped() {
        let client =
            GoogleGeminiClient::new(test_creds("proj"), GeminiVariant::GeminiCli, None).unwrap();

        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("max")],
            max_tokens: 32000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Max),
            task_type: None,
        };

        let body = client.build_request_body(&request);
        let config = &body["request"]["generationConfig"]["thinkingConfig"];
        assert_eq!(config["thinkingBudget"], 32768); // clamped to High
    }

    #[test]
    fn test_convert_messages_user_assistant() {
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi there!"),
            Message::user("How are you?"),
        ];
        let contents = convert_messages(&messages);
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[2]["role"], "user");
    }

    #[test]
    fn test_convert_messages_tool_result_with_name_lookup() {
        // Simulate a multi-turn: assistant calls a tool, user provides result
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tc_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "/tmp"}),
                }],
            },
            Message::tool_result("tc_1", "file contents", false),
        ];
        let contents = convert_messages(&messages);
        assert_eq!(contents.len(), 2);
        // Tool result should have the function name resolved from the ToolUse
        let func_resp = &contents[1]["parts"][0]["functionResponse"];
        assert_eq!(func_resp["name"], "read_file");
        assert_eq!(func_resp["response"]["output"], "file contents");
    }

    #[test]
    fn test_parse_response_text() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "The answer is 42."}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 15,
                "candidatesTokenCount": 8,
                "totalTokenCount": 23
            }
        });

        let response = parse_generate_content_response(&json).unwrap();
        assert_eq!(response.text(), Some("The answer is 42."));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 15);
        assert_eq!(response.usage.output_tokens, 8);
    }

    #[test]
    fn test_parse_response_function_call() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "read_file",
                            "args": {"path": "/tmp/test.txt"}
                        }
                    }]
                },
                "finishReason": "FUNCTION_CALLING"
            }],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 5}
        });

        let response = parse_generate_content_response(&json).unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        let tools = response.tool_calls();
        assert_eq!(tools.len(), 1);
        match &tools[0] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/test.txt");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_parse_response_max_tokens() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "partial"}]},
                "finishReason": "MAX_TOKENS"
            }]
        });

        let response = parse_generate_content_response(&json).unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::MaxTokens));
    }

    #[test]
    fn test_parse_response_skips_thinking() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        {"thought": true, "text": "Let me think..."},
                        {"text": "The answer is 42."}
                    ]
                },
                "finishReason": "STOP"
            }]
        });

        let response = parse_generate_content_response(&json).unwrap();
        assert_eq!(response.content.len(), 1);
        assert_eq!(response.text(), Some("The answer is 42."));
    }

    #[test]
    fn test_sse_text_streaming() {
        let mut acc = GeminiAccumulator::new();
        let mut events = Vec::new();

        let event1 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"text":"Hello "}]}}]}"#.into(),
        };
        process_gemini_sse(&event1, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let event2 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"text":"world!"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#.into(),
        };
        process_gemini_sse(&event2, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.text(), Some("Hello world!"));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_sse_function_call_streaming() {
        let mut acc = GeminiAccumulator::new();
        let mut events = Vec::new();

        let event = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"shell_exec","args":{"command":"ls"}}}]},"finishReason":"FUNCTION_CALLING"}]}"#.into(),
        };
        process_gemini_sse(&event, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(response.tool_calls().len(), 1);
        assert!(!events.is_empty()); // ToolUseStart + InputJsonDelta
    }

    #[test]
    fn test_variant_endpoints() {
        assert!(GeminiVariant::GeminiCli
            .endpoint()
            .contains("cloudcode-pa.googleapis.com"));
        assert!(GeminiVariant::Antigravity
            .endpoint()
            .contains("sandbox.googleapis.com"));
    }

    #[test]
    fn test_variant_client_ids_differ() {
        assert_ne!(
            GeminiVariant::GeminiCli.client_id(),
            GeminiVariant::Antigravity.client_id()
        );
    }

    #[test]
    fn test_variant_user_agents() {
        assert!(GeminiVariant::GeminiCli
            .user_agent()
            .contains("google-cloud-sdk"));
        assert!(GeminiVariant::Antigravity
            .user_agent()
            .contains("antigravity"));
    }

    #[test]
    fn test_map_finish_reason() {
        assert_eq!(map_finish_reason("STOP"), StopReason::EndTurn);
        assert_eq!(map_finish_reason("MAX_TOKENS"), StopReason::MaxTokens);
        assert_eq!(map_finish_reason("FUNCTION_CALLING"), StopReason::ToolUse);
        assert_eq!(map_finish_reason("SAFETY"), StopReason::EndTurn); // graceful
        assert_eq!(map_finish_reason("UNKNOWN"), StopReason::EndTurn);
    }

    #[test]
    fn test_extract_google_error_json() {
        let text = r#"{"error":{"code":429,"message":"Rate limit exceeded","status":"RESOURCE_EXHAUSTED"}}"#;
        assert_eq!(extract_google_error(text), "Rate limit exceeded");
    }

    #[test]
    fn test_extract_google_error_plain() {
        let text = "Internal server error";
        assert_eq!(extract_google_error(text), "Internal server error");
    }

    #[test]
    fn test_stream_url() {
        let client =
            GoogleGeminiClient::new(test_creds("proj"), GeminiVariant::GeminiCli, None).unwrap();
        assert!(client.build_stream_url().contains("streamGenerateContent"));
        assert!(client.build_stream_url().contains("alt=sse"));
    }

    #[test]
    fn test_thinking_levels() {
        let client =
            GoogleGeminiClient::new(test_creds("proj"), GeminiVariant::GeminiCli, None).unwrap();

        for (level, expected_budget) in [
            (crate::model_tier::ThinkingLevel::Minimal, 128),
            (crate::model_tier::ThinkingLevel::Low, 2048),
            (crate::model_tier::ThinkingLevel::Medium, 8192),
            (crate::model_tier::ThinkingLevel::High, 32768),
        ] {
            let request = LlmRequest {
                model: "gemini-2.5-pro".into(),
                system: None,
                messages: vec![Message::user("test")],
                max_tokens: 64000,
                tools: vec![],
                cell_id: None,
                trace_id: None,
                thinking: Some(level),
                task_type: None,
            };
            let body = client.build_request_body(&request);
            let budget = body["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"]
                .as_u64()
                .unwrap();
            assert_eq!(
                budget, expected_budget,
                "ThinkingLevel::{level:?} budget mismatch"
            );
        }
    }

    #[test]
    fn test_process_gemini_sse_malformed_json_returns_error() {
        let mut acc = GeminiAccumulator::new();
        let bad_event = crate::sse::SseEvent {
            event_type: "message".into(),
            data: "{ not valid json".into(),
        };
        let result = process_gemini_sse(&bad_event, &mut acc, &mut |_| {});
        assert!(result.is_err());
    }

    #[test]
    fn test_gemini_accumulator_empty_returns_none() {
        let acc = GeminiAccumulator::new();
        assert!(acc.into_response().is_none());
    }
}

#[cfg(test)]
mod llm_provider_impl_tests {
    use super::*;
    use crate::provider::LlmProvider;
    use crate::provider_id::ProviderId;

    fn oauth_creds() -> AuthCredentials {
        let mut extra = std::collections::HashMap::new();
        extra.insert(
            "project_id".to_string(),
            serde_json::Value::String("test-project".to_string()),
        );
        AuthCredentials::OAuth {
            access: "test-token".into(),
            refresh: "test-refresh".into(),
            expires: 9_999_999_999_999,
            extra,
        }
    }

    #[test]
    fn implements_llm_provider_trait() {
        let client =
            GoogleGeminiClient::new(oauth_creds(), GeminiVariant::GeminiCli, None).expect("new");
        let boxed: Box<dyn LlmProvider> = Box::new(client);
        assert_eq!(boxed.provider_id(), ProviderId::GoogleGeminiCli);
        assert!(!boxed.default_model().is_empty());
    }

    #[tokio::test]
    async fn list_models_returns_empty_until_ai_studio_wired() {
        // Plan 3 reality: Vertex/CLI endpoint has no equivalent of AI Studio's
        // `/v1beta/models?key=...` listing. Gemini API-key path is deferred
        // (see TODO.md). Until then, list_models defaults to empty and the
        // ModelRegistry's baked-in fallback (Plan 3 Task 5) provides a
        // curated v0 list.
        let client =
            GoogleGeminiClient::new(oauth_creds(), GeminiVariant::GeminiCli, None).unwrap();
        let models = <GoogleGeminiClient as LlmProvider>::list_models(&client).await;
        assert!(
            models.is_empty(),
            "Gemini list_models should be empty until AI Studio endpoint is wired; got {} entries",
            models.len()
        );
    }
}
