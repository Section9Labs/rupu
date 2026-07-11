use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Client;
use tracing::{info, warn};

use crate::auth::{save_provider_auth, AuthCredentials};
use crate::error::ProviderError;
use crate::sse::SseParser;
use crate::types::*;

// ── Constants ────────────────────────────────────────────────────────

const DEFAULT_COPILOT_API_URL: &str = "https://api.githubcopilot.com";
const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// GitHub Copilot client using OpenAI chat/completions format.
///
/// Auth flow:
/// 1. auth.json stores a GitHub OAuth token as `access` (long-lived)
/// 2. On first use / expiry, exchange via GET copilot_internal/v2/token
/// 3. Returns short-lived Copilot `token` + `expires_at` + `proxy-ep`
/// 4. The Copilot token is used as Bearer for chat/completions
pub struct GithubCopilotClient {
    client: Client,
    /// Long-lived GitHub OAuth token (used to exchange for Copilot tokens).
    github_token: String,
    /// Short-lived Copilot API token (from token exchange).
    copilot_token: String,
    /// When the Copilot token expires (epoch ms).
    copilot_expires_ms: u64,
    /// API base URL (extracted from proxy-ep in token, or default).
    api_url: String,
    /// Optional enterprise domain override.
    enterprise_domain: Option<String>,
    auth_json_path: Option<PathBuf>,
}

impl GithubCopilotClient {
    /// Create from resolved AuthCredentials.
    pub fn new(
        creds: AuthCredentials,
        auth_json_path: Option<PathBuf>,
    ) -> Result<Self, ProviderError> {
        match creds {
            AuthCredentials::OAuth {
                access,
                refresh: _,
                expires: _,
                extra,
            } => {
                let enterprise_domain = extra
                    .get("enterprise_url")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                Ok(Self {
                    client: Client::new(),
                    github_token: access,
                    copilot_token: String::new(),
                    copilot_expires_ms: 0,
                    api_url: DEFAULT_COPILOT_API_URL.to_string(),
                    enterprise_domain,
                    auth_json_path,
                })
            }
            AuthCredentials::ApiKey { key } => Ok(Self {
                client: Client::new(),
                github_token: key,
                copilot_token: String::new(),
                copilot_expires_ms: 0,
                api_url: DEFAULT_COPILOT_API_URL.to_string(),
                enterprise_domain: None,
                auth_json_path,
            }),
        }
    }

    /// Non-streaming send.
    pub async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = crate::openai_wire::build_chat_request_body(request, false);
        let url = format!("{}/chat/completions", self.api_url);

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
                message: truncate(&text, 500),
            });
        }

        let json: serde_json::Value = response.json().await?;
        crate::openai_wire::parse_chat_completion(&json)
    }

    /// Streaming send with SSE.
    pub async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = crate::openai_wire::build_chat_request_body(request, true);
        let url = format!("{}/chat/completions", self.api_url);

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
                message: truncate(&text, 500),
            });
        }

        let mut parser = SseParser::new();
        let mut acc = crate::openai_wire::CompletionAccumulator::new();
        let mut bytes_stream = response.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
            let events = parser.feed(&chunk)?;
            for event in events {
                crate::openai_wire::process_completion_sse(&event, &mut acc, on_event)?;
            }
        }

        acc.into_response()
            .ok_or(ProviderError::UnexpectedEndOfStream)
    }

    fn build_headers(&self) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let mut headers = reqwest::header::HeaderMap::new();
        let auth_val = format!("Bearer {}", self.copilot_token)
            .parse()
            .map_err(|_| {
                ProviderError::AuthConfig("copilot token contains invalid header characters".into())
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
        // Copilot-specific headers
        headers.insert(
            reqwest::header::USER_AGENT,
            "GitHubCopilotChat/0.35.0".parse().unwrap(),
        );
        headers.insert("Editor-Version", "vscode/1.107.0".parse().unwrap());
        headers.insert(
            "Editor-Plugin-Version",
            "copilot-chat/0.35.0".parse().unwrap(),
        );
        headers.insert("Copilot-Integration-Id", "vscode-chat".parse().unwrap());
        headers.insert("X-Initiator", "user".parse().unwrap());
        headers.insert("Openai-Intent", "conversation-edits".parse().unwrap());
        Ok(headers)
    }

    /// Exchange GitHub OAuth token for short-lived Copilot API token.
    async fn ensure_valid_token(&mut self) -> Result<(), ProviderError> {
        if !self.copilot_token.is_empty() && !is_copilot_expired(self.copilot_expires_ms) {
            return Ok(());
        }

        info!("exchanging GitHub token for Copilot API token");

        let token_url = if let Some(ref domain) = self.enterprise_domain {
            validate_domain(domain)?;
            format!("https://api.{domain}/copilot_internal/v2/token")
        } else {
            COPILOT_TOKEN_URL.to_string()
        };

        let response = self
            .client
            .get(&token_url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.github_token),
            )
            .header(reqwest::header::USER_AGENT, "GitHubCopilotChat/0.35.0")
            .header("Editor-Version", "vscode/1.107.0")
            .header("Editor-Plugin-Version", "copilot-chat/0.35.0")
            .header("Copilot-Integration-Id", "vscode-chat")
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

        let token = body["token"]
            .as_str()
            .ok_or_else(|| ProviderError::TokenRefreshFailed("missing token field".into()))?
            .to_string();

        let expires_at = body["expires_at"]
            .as_u64()
            .ok_or_else(|| ProviderError::TokenRefreshFailed("missing expires_at field".into()))?;

        // expires_at is Unix seconds; convert to ms with 5-minute buffer (saturating to avoid underflow)
        self.copilot_expires_ms = (expires_at * 1000).saturating_sub(5 * 60 * 1000);
        self.copilot_token = token.clone();

        // Extract API URL from proxy-ep in the token (validated against allowlist)
        if let Some(url) = extract_api_url_from_token(&token) {
            self.api_url = url;
        }

        info!("Copilot token obtained, expires_at={expires_at}");

        // Persist the github_token (doesn't change, but update expires metadata)
        if let Some(ref path) = self.auth_json_path {
            let mut extra = HashMap::new();
            if let Some(ref domain) = self.enterprise_domain {
                extra.insert(
                    "enterprise_url".to_string(),
                    serde_json::Value::String(domain.clone()),
                );
            }
            let creds = AuthCredentials::OAuth {
                access: self.github_token.clone(),
                refresh: String::new(),
                expires: self.copilot_expires_ms,
                extra,
            };
            if let Err(e) =
                save_provider_auth(path, crate::provider_id::ProviderId::GithubCopilot, &creds)
            {
                warn!(error = %e, "failed to persist Copilot credentials");
            }
        }

        Ok(())
    }
}

fn make_model_info(id: &str) -> crate::model_pool::ModelInfo {
    crate::model_pool::ModelInfo {
        id: id.to_string(),
        provider: crate::provider_id::ProviderId::GithubCopilot,
        context_window: 0,
        max_output_tokens: 0,
        capabilities: Vec::new(),
        cost: crate::model_pool::ModelCost::default(),
        status: crate::model_pool::ModelStatus::default(),
    }
}

#[async_trait::async_trait]
impl crate::provider::LlmProvider for GithubCopilotClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        GithubCopilotClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        GithubCopilotClient::stream(self, request, on_event).await
    }

    async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
        // GitHub Copilot doesn't expose a public /models endpoint.
        // Slice B-1 spec §6a: ship a baked-in list. Users who have access
        // to additional models register them as custom entries in
        // ~/.rupu/config.toml.
        ["gpt-4o", "gpt-4o-mini", "claude-sonnet-4-6", "o4-mini"]
            .into_iter()
            .map(make_model_info)
            .collect()
    }

    fn default_model(&self) -> &str {
        "claude-sonnet-4-6"
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        crate::provider_id::ProviderId::GithubCopilot
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Check if Copilot token is expired (with buffer already baked in).
fn is_copilot_expired(expires_ms: u64) -> bool {
    if expires_ms == 0 {
        return true; // No token yet
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    now_ms >= expires_ms
}

/// Allowed Copilot API host suffixes for SSRF prevention.
const ALLOWED_COPILOT_HOSTS: &[&str] = &["githubcopilot.com", "ghe-copilot.com", "github.com"];

/// Validate that a hostname is a legitimate GitHub Copilot domain.
fn is_valid_copilot_host(host: &str) -> bool {
    ALLOWED_COPILOT_HOSTS
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
}

/// Validate an enterprise domain for safe URL construction.
fn validate_domain(domain: &str) -> Result<(), ProviderError> {
    if domain.is_empty()
        || domain.contains('/')
        || domain.contains('?')
        || domain.contains('#')
        || domain.contains('@')
        || domain.contains(':')
        || domain.contains(' ')
    {
        return Err(ProviderError::AuthConfig(format!(
            "invalid enterprise domain: {domain}"
        )));
    }
    Ok(())
}

/// Extract API base URL from the Copilot token's proxy-ep field.
/// Token format: `tid=...;exp=...;proxy-ep=proxy.individual.githubcopilot.com;...`
/// Validates the extracted host against an allowlist of GitHub domains.
fn extract_api_url_from_token(token: &str) -> Option<String> {
    let proxy_ep = token
        .split(';')
        .find(|part| part.starts_with("proxy-ep="))?
        .strip_prefix("proxy-ep=")?;
    // Convert proxy.xxx to api.xxx
    let api_host = proxy_ep.replacen("proxy.", "api.", 1);
    // Validate against allowlist to prevent SSRF
    if !is_valid_copilot_host(&api_host) {
        warn!(
            host = api_host,
            "rejected proxy-ep: not a known GitHub Copilot domain"
        );
        return None;
    }
    Some(format!("https://{api_host}"))
}

/// UTF-8 safe truncation.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai_wire::{
        build_chat_request_body, parse_chat_completion, process_completion_sse,
        CompletionAccumulator,
    };

    fn test_creds() -> AuthCredentials {
        AuthCredentials::OAuth {
            access: "ghu_test_github_token".into(),
            refresh: String::new(),
            expires: 0,
            extra: HashMap::new(),
        }
    }

    fn test_creds_enterprise() -> AuthCredentials {
        let mut extra = HashMap::new();
        extra.insert(
            "enterprise_url".to_string(),
            serde_json::Value::String("mycompany.ghe.com".into()),
        );
        AuthCredentials::OAuth {
            access: "ghu_enterprise_token".into(),
            refresh: String::new(),
            expires: 0,
            extra,
        }
    }

    #[test]
    fn test_new_from_oauth() {
        let client = GithubCopilotClient::new(test_creds(), None).unwrap();
        assert_eq!(client.github_token, "ghu_test_github_token");
        assert!(client.copilot_token.is_empty());
        assert!(client.enterprise_domain.is_none());
    }

    #[test]
    fn test_new_enterprise() {
        let client = GithubCopilotClient::new(test_creds_enterprise(), None).unwrap();
        assert_eq!(
            client.enterprise_domain,
            Some("mycompany.ghe.com".to_string())
        );
    }

    #[test]
    fn test_new_from_api_key() {
        let creds = AuthCredentials::ApiKey {
            key: "ghp_test".into(),
        };
        let client = GithubCopilotClient::new(creds, None).unwrap();
        assert_eq!(client.github_token, "ghp_test");
    }

    #[test]
    fn test_build_request_body_basic() {
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: Some("Be helpful.".into()),
            messages: vec![Message::user("Hello")],
            max_tokens: 4096,
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

        let body = build_chat_request_body(&request, true);
        assert_eq!(body["model"], "claude-sonnet-4-6");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["stream"], true);

        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2); // system + user
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "Be helpful.");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let request = LlmRequest {
            model: "o3".into(),
            system: None,
            messages: vec![Message::user("read file")],
            max_tokens: 4096,
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
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

        let body = build_chat_request_body(&request, false);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "read_file");
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn test_build_request_body_with_reasoning() {
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

        let body = build_chat_request_body(&request, false);
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn test_build_request_body_no_tool_choice_without_tools() {
        let request = LlmRequest {
            model: "gpt-4.1".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 100,
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

        let body = build_chat_request_body(&request, false);
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn test_parse_chat_completion_text() {
        let json = serde_json::json!({
            "id": "chatcmpl-123",
            "model": "claude-sonnet-4-6",
            "choices": [{
                "message": {"role": "assistant", "content": "Hello!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });

        let response = parse_chat_completion(&json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.text(), Some("Hello!"));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 10);
    }

    #[test]
    fn test_parse_chat_completion_tool_calls() {
        let json = serde_json::json!({
            "id": "chatcmpl-456",
            "model": "o3",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"/tmp/test.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 10}
        });

        let response = parse_chat_completion(&json).unwrap();
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
    fn test_parse_chat_completion_length() {
        let json = serde_json::json!({
            "id": "chatcmpl-789",
            "model": "gpt-4.1",
            "choices": [{
                "message": {"role": "assistant", "content": "partial"},
                "finish_reason": "length"
            }]
        });

        let response = parse_chat_completion(&json).unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::MaxTokens));
    }

    #[test]
    fn test_extract_api_url_from_token() {
        let token = "tid=abc;exp=123;proxy-ep=proxy.individual.githubcopilot.com;st=dotcom";
        assert_eq!(
            extract_api_url_from_token(token),
            Some("https://api.individual.githubcopilot.com".to_string())
        );
    }

    #[test]
    fn test_extract_api_url_from_token_no_proxy() {
        assert_eq!(extract_api_url_from_token("tid=abc;exp=123"), None);
    }

    #[test]
    fn test_extract_api_url_enterprise() {
        let token = "tid=abc;proxy-ep=proxy.myorg.ghe-copilot.com;st=enterprise";
        assert_eq!(
            extract_api_url_from_token(token),
            Some("https://api.myorg.ghe-copilot.com".to_string())
        );
    }

    #[test]
    fn test_is_copilot_expired_zero() {
        assert!(is_copilot_expired(0)); // No token yet
    }

    #[test]
    fn test_is_copilot_expired_future() {
        let future = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 3_600_000;
        assert!(!is_copilot_expired(future));
    }

    #[test]
    fn test_is_copilot_expired_past() {
        assert!(is_copilot_expired(1000));
    }

    #[test]
    fn test_sse_text_streaming() {
        let mut acc = CompletionAccumulator::new();
        let mut events = Vec::new();

        let event1 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"id":"chatcmpl-1","model":"gpt-4.1","choices":[{"delta":{"content":"Hello "}}]}"#.into(),
        };
        process_completion_sse(&event1, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let event2 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"choices":[{"delta":{"content":"world!"},"finish_reason":"stop"}]}"#.into(),
        };
        process_completion_sse(&event2, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.id, "chatcmpl-1");
        assert_eq!(response.text(), Some("Hello world!"));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_sse_tool_call_streaming() {
        let mut acc = CompletionAccumulator::new();
        let mut events = Vec::new();

        let event1 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"id":"chatcmpl-2","model":"o3","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":""}}]}}]}"#.into(),
        };
        process_completion_sse(&event1, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let event2 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":\"/tmp\"}"}}]}}]}"#.into(),
        };
        process_completion_sse(&event2, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let event3 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"choices":[{"finish_reason":"tool_calls"}]}"#.into(),
        };
        process_completion_sse(&event3, &mut acc, &mut |_| {}).unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        let tools = response.tool_calls();
        assert_eq!(tools.len(), 1);
        match &tools[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_sse_done_signal() {
        let mut acc = CompletionAccumulator::new();
        let done = crate::sse::SseEvent {
            event_type: "message".into(),
            data: "[DONE]".into(),
        };
        assert!(process_completion_sse(&done, &mut acc, &mut |_| {}).is_ok());
    }

    #[test]
    fn test_sse_malformed_json_returns_error() {
        let mut acc = CompletionAccumulator::new();
        let bad = crate::sse::SseEvent {
            event_type: "message".into(),
            data: "not json".into(),
        };
        assert!(process_completion_sse(&bad, &mut acc, &mut |_| {}).is_err());
    }

    #[test]
    fn test_accumulator_empty_returns_none() {
        let acc = CompletionAccumulator::new();
        assert!(acc.into_response().is_none());
    }

    #[test]
    fn test_tool_result_message() {
        let request = LlmRequest {
            model: "gpt-4.1".into(),
            system: None,
            messages: vec![Message::tool_result("call_1", "file contents", false)],
            max_tokens: 100,
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

        let body = build_chat_request_body(&request, false);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "tool");
        assert_eq!(msgs[0]["tool_call_id"], "call_1");
        assert_eq!(msgs[0]["content"], "file contents");
    }

    // ── Security tests ───────────────────────────────────────────────

    #[test]
    fn test_extract_api_url_rejects_unknown_host() {
        let token = "tid=abc;proxy-ep=proxy.evil.com;st=dotcom";
        assert_eq!(extract_api_url_from_token(token), None);
    }

    #[test]
    fn test_extract_api_url_accepts_known_hosts() {
        let token = "tid=abc;proxy-ep=proxy.individual.githubcopilot.com;st=dotcom";
        assert!(extract_api_url_from_token(token).is_some());

        let token = "tid=abc;proxy-ep=proxy.myorg.ghe-copilot.com;st=enterprise";
        assert!(extract_api_url_from_token(token).is_some());
    }

    #[test]
    fn test_validate_domain_rejects_malicious() {
        assert!(validate_domain("evil.com/steal?x=").is_err());
        assert!(validate_domain("127.0.0.1:8080").is_err());
        assert!(validate_domain("").is_err());
        assert!(validate_domain("host with spaces").is_err());
        assert!(validate_domain("host#fragment").is_err());
        assert!(validate_domain("user@host").is_err());
    }

    #[test]
    fn test_validate_domain_accepts_valid() {
        assert!(validate_domain("mycompany.ghe.com").is_ok());
        assert!(validate_domain("github.myenterprise.com").is_ok());
    }

    #[test]
    fn test_is_valid_copilot_host() {
        assert!(is_valid_copilot_host("api.individual.githubcopilot.com"));
        assert!(is_valid_copilot_host("api.myorg.ghe-copilot.com"));
        assert!(is_valid_copilot_host("githubcopilot.com"));
        assert!(!is_valid_copilot_host("evil.com"));
        assert!(!is_valid_copilot_host("api.evil.com"));
    }

    #[test]
    fn test_expiry_saturating_sub() {
        // expires_at=0 should not underflow
        let expires_ms = 0u64.saturating_sub(5 * 60 * 1000);
        assert_eq!(expires_ms, 0);
        assert!(is_copilot_expired(expires_ms));
    }

    #[test]
    fn test_parse_chat_completion_malformed_tool_arguments() {
        let json = serde_json::json!({
            "id": "chatcmpl-bad",
            "model": "o3",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_bad",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "not valid json {"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let result = parse_chat_completion(&json);
        assert!(
            result.is_err(),
            "malformed tool arguments should return error"
        );
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
            key: "ghp_test".into(),
        };
        let client = GithubCopilotClient::new(creds, None).expect("new");
        let boxed: Box<dyn LlmProvider> = Box::new(client);
        assert_eq!(boxed.provider_id(), ProviderId::GithubCopilot);
        assert!(!boxed.default_model().is_empty());
    }
}

#[cfg(test)]
mod baked_in_tests {
    use super::*;
    use crate::auth::AuthCredentials;
    use crate::provider::LlmProvider;

    #[tokio::test]
    async fn list_models_returns_baked_in_when_offline() {
        let creds = AuthCredentials::ApiKey {
            key: "ghp-test".into(),
        };
        let client = GithubCopilotClient::new(creds, None).unwrap();
        let models = client.list_models().await;
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id == "gpt-4o"));
    }
}
