//! Local model provider: wraps a local HTTP inference server.
//! Speaks the OpenAI-compatible /v1/chat/completions API.
//! Spec Phase 7C: local model for routine scans and recall.

#![allow(clippy::module_name_repetitions)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;
use crate::provider::LlmProvider;
use crate::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, Role, StopReason, StreamEvent, Usage,
};

/// Provider backed by a local HTTP inference server (llama.cpp, Ollama, vLLM).
///
/// Implements the OpenAI-compatible `/v1/chat/completions` endpoint. The local
/// server handles tokenization, sampling, and generation; this provider just
/// translates between the phi-cell `LlmRequest`/`LlmResponse` types and the
/// OpenAI wire format.
pub struct LocalModelProvider {
    endpoint: String,
    model_name: String,
    client: reqwest::Client,
}

impl LocalModelProvider {
    /// Create a new `LocalModelProvider`.
    ///
    /// * `endpoint` — base URL of the local server, e.g. `"http://localhost:8080"`
    /// * `model_name` — model identifier the server expects in the `model` field
    pub fn new(endpoint: &str, model_name: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            model_name: model_name.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Check if the local server is reachable by hitting `/v1/models`.
    pub async fn health_check(&self) -> bool {
        let url = format!("{}/v1/models", self.endpoint);
        match self.client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Return the configured endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Return the configured model name.
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Build an OpenAI-compatible JSON request body from an `LlmRequest`.
    fn build_openai_request(&self, request: &LlmRequest) -> serde_json::Value {
        let mut msgs: Vec<serde_json::Value> = Vec::new();

        if let Some(ref system) = request.system {
            msgs.push(serde_json::json!({
                "role": "system",
                "content": system
            }));
        }

        for msg in &request.messages {
            let role_str = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let content = extract_text_content(msg);
            msgs.push(serde_json::json!({
                "role": role_str,
                "content": content
            }));
        }

        serde_json::json!({
            "model": self.model_name,
            "messages": msgs,
            "max_tokens": request.max_tokens,
            "stream": false
        })
    }
}

/// Extract concatenated text content from a message, ignoring non-text blocks.
fn extract_text_content(msg: &Message) -> String {
    msg.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[async_trait]
impl LlmProvider for LocalModelProvider {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let url = format!("{}/v1/chat/completions", self.endpoint);
        let body = self.build_openai_request(request);

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http(format!("local model request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Json(format!("cannot parse local model response: {e}")))?;

        // Parse OpenAI-compatible response
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let input_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
        let output_tokens = json["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;

        Ok(LlmResponse {
            id: json["id"].as_str().unwrap_or("local").to_string(),
            model: self.model_name.clone(),
            content: vec![ContentBlock::Text { text: content }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage {
                input_tokens,
                output_tokens,
                ..Default::default()
            },
        })
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        // For local models, use non-streaming send and emit synthetic events.
        // Most local servers support SSE streaming but adding full SSE parsing
        // for local servers is Phase 7C+ scope. The callback still fires so
        // callers that depend on streaming feedback work correctly.
        let response = self.send(request).await?;

        if let Some(text) = response.text() {
            on_event(StreamEvent::TextDelta(text.to_string()));
        }

        Ok(response)
    }

    fn default_model(&self) -> &str {
        "local"
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        crate::provider_id::ProviderId::Anthropic
    }
}

// ── Routing Policy ──────────────────────────────────────────────────────────

/// Routes requests between frontier (Anthropic) and local model providers.
///
/// The router checks `task_type` against these lists to decide which provider
/// handles a given request. When the local model is unavailable, all requests
/// fall back to frontier automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// Task types eligible for the local model when it is available.
    pub local_eligible: Vec<String>,
    /// Task types that must always use the frontier provider.
    pub frontier_only: Vec<String>,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            local_eligible: vec![
                "watchdog_scan".into(),
                "recall".into(),
                "classification".into(),
            ],
            frontier_only: vec!["deep_think".into(), "governance".into(), "identity".into()],
        }
    }
}

impl RoutingPolicy {
    /// Returns `true` if `task_type` should be routed to the local model.
    pub fn is_local_eligible(&self, task_type: &str) -> bool {
        self.local_eligible.iter().any(|t| t == task_type)
    }

    /// Returns `true` if `task_type` must always go to frontier.
    pub fn is_frontier_only(&self, task_type: &str) -> bool {
        self.frontier_only.iter().any(|t| t == task_type)
    }

    /// Decide which provider to use for a given task type.
    /// Returns `RoutingDecision::Local` if the task is local-eligible and
    /// `local_available` is true; otherwise `RoutingDecision::Frontier`.
    pub fn route(&self, task_type: &str, local_available: bool) -> RoutingDecision {
        if self.is_frontier_only(task_type) {
            return RoutingDecision::Frontier;
        }
        if local_available && self.is_local_eligible(task_type) {
            return RoutingDecision::Local;
        }
        RoutingDecision::Frontier
    }
}

/// The outcome of a routing decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingDecision {
    /// Use the local model provider.
    Local,
    /// Use the frontier (Anthropic) provider.
    Frontier,
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_model_provider_new_trims_trailing_slash() {
        let provider = LocalModelProvider::new("http://localhost:8080/", "phi-local");
        assert_eq!(provider.endpoint(), "http://localhost:8080");
        assert_eq!(provider.model_name(), "phi-local");
    }

    #[test]
    fn test_local_model_provider_builds_request_with_system() {
        let provider = LocalModelProvider::new("http://localhost:8080", "phi-local");
        let request = LlmRequest {
            model: "test".into(),
            system: Some("You are helpful.".into()),
            messages: vec![Message::user("Hello")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
        };
        let body = provider.build_openai_request(&request);
        assert_eq!(body["model"], "phi-local");
        assert_eq!(body["max_tokens"], 100);
        assert_eq!(body["stream"], false);
        let messages = body["messages"]
            .as_array()
            .expect("messages should be array");
        assert_eq!(messages.len(), 2); // system + user
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello");
    }

    #[test]
    fn test_local_model_provider_builds_request_without_system() {
        let provider = LocalModelProvider::new("http://localhost:8080", "phi-local");
        let request = LlmRequest {
            model: "test".into(),
            system: None,
            messages: vec![
                Message::user("Hello"),
                Message::assistant("Hi there!"),
                Message::user("How are you?"),
            ],
            max_tokens: 200,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
        };
        let body = provider.build_openai_request(&request);
        let messages = body["messages"]
            .as_array()
            .expect("messages should be array");
        assert_eq!(messages.len(), 3); // no system
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "user");
    }

    #[test]
    fn test_extract_text_content_skips_tool_blocks() {
        let msg = Message {
            role: Role::User,
            content: vec![
                ContentBlock::Text {
                    text: "First ".into(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "result".into(),
                    is_error: false,
                },
                ContentBlock::Text {
                    text: "Second".into(),
                },
            ],
        };
        assert_eq!(extract_text_content(&msg), "First Second");
    }

    #[test]
    fn test_extract_text_content_empty() {
        let msg = Message {
            role: Role::User,
            content: vec![],
        };
        assert_eq!(extract_text_content(&msg), "");
    }

    // ── RoutingPolicy tests ─────────────────────────────────────────────

    #[test]
    fn test_routing_policy_defaults() {
        let policy = RoutingPolicy::default();
        assert!(policy.local_eligible.contains(&"watchdog_scan".into()));
        assert!(policy.local_eligible.contains(&"recall".into()));
        assert!(policy.local_eligible.contains(&"classification".into()));
        assert!(policy.frontier_only.contains(&"deep_think".into()));
        assert!(policy.frontier_only.contains(&"governance".into()));
        assert!(policy.frontier_only.contains(&"identity".into()));
    }

    #[test]
    fn test_routing_policy_serde_roundtrip() {
        let policy = RoutingPolicy::default();
        let json = serde_json::to_string(&policy).expect("serialize");
        let parsed: RoutingPolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(policy, parsed);
    }

    #[test]
    fn test_routing_policy_toml_roundtrip() {
        let policy = RoutingPolicy::default();
        let toml_str = toml::to_string(&policy).expect("serialize toml");
        let parsed: RoutingPolicy = toml::from_str(&toml_str).expect("deserialize toml");
        assert_eq!(policy, parsed);
    }

    #[test]
    fn test_is_local_eligible() {
        let policy = RoutingPolicy::default();
        assert!(policy.is_local_eligible("watchdog_scan"));
        assert!(policy.is_local_eligible("recall"));
        assert!(!policy.is_local_eligible("governance"));
        assert!(!policy.is_local_eligible("unknown_task"));
    }

    #[test]
    fn test_is_frontier_only() {
        let policy = RoutingPolicy::default();
        assert!(policy.is_frontier_only("governance"));
        assert!(policy.is_frontier_only("identity"));
        assert!(!policy.is_frontier_only("recall"));
    }

    #[test]
    fn test_route_frontier_only_always_returns_frontier() {
        let policy = RoutingPolicy::default();
        assert_eq!(policy.route("governance", true), RoutingDecision::Frontier);
        assert_eq!(policy.route("governance", false), RoutingDecision::Frontier);
    }

    #[test]
    fn test_route_local_eligible_when_available() {
        let policy = RoutingPolicy::default();
        assert_eq!(policy.route("watchdog_scan", true), RoutingDecision::Local);
        assert_eq!(policy.route("recall", true), RoutingDecision::Local);
    }

    #[test]
    fn test_route_local_eligible_when_unavailable_falls_back() {
        let policy = RoutingPolicy::default();
        assert_eq!(
            policy.route("watchdog_scan", false),
            RoutingDecision::Frontier
        );
    }

    #[test]
    fn test_route_unknown_task_goes_to_frontier() {
        let policy = RoutingPolicy::default();
        assert_eq!(
            policy.route("unknown_task", true),
            RoutingDecision::Frontier
        );
        assert_eq!(
            policy.route("unknown_task", false),
            RoutingDecision::Frontier
        );
    }

    #[test]
    fn test_routing_policy_custom() {
        let policy = RoutingPolicy {
            local_eligible: vec!["summarize".into()],
            frontier_only: vec!["code_gen".into()],
        };
        assert_eq!(policy.route("summarize", true), RoutingDecision::Local);
        assert_eq!(policy.route("code_gen", true), RoutingDecision::Frontier);
        assert_eq!(
            policy.route("anything_else", true),
            RoutingDecision::Frontier
        );
    }
}
