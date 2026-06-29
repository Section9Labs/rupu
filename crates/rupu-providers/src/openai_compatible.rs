//! Generic OpenAI-compatible provider.
//!
//! Speaks the OpenAI `/v1/chat/completions` API against a configurable base
//! URL with a static Bearer key. Covers self-hosted vLLM, Oracle GenAI,
//! Together, Fireworks, OpenRouter, and similar endpoints. Wire-format logic
//! is shared with the Copilot client via [`crate::openai_wire`].

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;

use crate::error::ProviderError;
use crate::model_pool::{ModelCapability, ModelCost, ModelInfo, ModelState, ModelStatus};
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;
use crate::sse::SseParser;
use crate::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent};

/// A model offered by an OpenAI-compatible endpoint, declared in config.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleModel {
    pub id: String,
    pub context_window: u32,
    pub max_output: u32,
}

/// Client for an OpenAI-compatible `/v1/chat/completions` endpoint.
pub struct OpenAiCompatibleClient {
    base_url: String,
    api_key: String,
    default_model: String,
    models: Vec<OpenAiCompatibleModel>,
    stream: bool,
    client: Client,
}

impl OpenAiCompatibleClient {
    /// * `base_url` — endpoint root, with or without a trailing `/v1`
    ///   (e.g. `http://192.29.35.246:8080` or `…/v1`).
    /// * `api_key` — static Bearer key.
    /// * `default_model` — model id sent when the request doesn't override it.
    /// * `models` — config-declared models, surfaced via `list_models`.
    /// * `stream` — when false, never request SSE (servers without it).
    pub fn new(
        base_url: &str,
        api_key: &str,
        default_model: &str,
        models: Vec<OpenAiCompatibleModel>,
        stream: bool,
    ) -> Self {
        // Normalize: strip trailing slashes, then strip a trailing `/v1`
        // so we hold the bare root and append `/v1/...` consistently.
        let trimmed = base_url.trim_end_matches('/');
        let root = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
        Self {
            base_url: root.to_string(),
            api_key: api_key.to_string(),
            default_model: default_model.to_string(),
            models,
            stream,
            client: Client::new(),
        }
    }

    fn completions_url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }

    fn request_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        crate::openai_wire::build_chat_request_body(request, stream)
    }

    fn headers(&self, stream: bool) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let mut headers = reqwest::header::HeaderMap::new();
        let auth_val = format!("Bearer {}", self.api_key).parse().map_err(|_| {
            ProviderError::AuthConfig("api key contains invalid header characters".into())
        })?;
        headers.insert(reqwest::header::AUTHORIZATION, auth_val);
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        let accept = if stream {
            "text/event-stream"
        } else {
            "application/json"
        };
        headers.insert(reqwest::header::ACCEPT, accept.parse().unwrap());
        Ok(headers)
    }

    async fn send_inner(&self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let body = self.request_body(request, false);
        let response = self
            .client
            .post(self.completions_url())
            .headers(self.headers(false)?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: text.chars().take(500).collect(),
            });
        }
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Json(e.to_string()))?;
        crate::openai_wire::parse_chat_completion(&json)
    }

    async fn stream_inner(
        &self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        let body = self.request_body(request, true);
        let response = self
            .client
            .post(self.completions_url())
            .headers(self.headers(true)?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: text.chars().take(500).collect(),
            });
        }
        let mut parser = SseParser::new();
        let mut acc = crate::openai_wire::CompletionAccumulator::new();
        let mut bytes_stream = response.bytes_stream();
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
            for event in parser.feed(&chunk)? {
                crate::openai_wire::process_completion_sse(&event, &mut acc, on_event)?;
            }
        }
        acc.into_response()
            .ok_or(ProviderError::UnexpectedEndOfStream)
    }

    fn model_info(&self, m: &OpenAiCompatibleModel) -> ModelInfo {
        let mut capabilities = vec![ModelCapability::ToolUse];
        if self.stream {
            capabilities.push(ModelCapability::Streaming);
        }
        ModelInfo {
            id: m.id.clone(),
            provider: ProviderId::OpenaiCompatible,
            context_window: m.context_window,
            max_output_tokens: m.max_output,
            capabilities,
            cost: ModelCost {
                input_per_million: 0.0,
                output_per_million: 0.0,
            },
            status: ModelStatus {
                state: ModelState::Available,
                utilization: None,
                quota_reset: None,
                last_success: None,
                last_error: None,
                consecutive_failures: 0,
            },
        }
    }
}

/// Emit stream events for an already-complete response, so the `stream=false`
/// fallback produces the same event sequence a real SSE stream would.
fn emit_response_events(
    resp: &LlmResponse,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
) {
    for block in &resp.content {
        match block {
            ContentBlock::Text { text } => {
                on_event(StreamEvent::TextDelta(text.clone()));
            }
            ContentBlock::ToolUse { id, name, input } => {
                on_event(StreamEvent::ToolUseStart {
                    id: id.clone(),
                    name: name.clone(),
                });
                on_event(StreamEvent::InputJsonDelta(input.to_string()));
            }
            ContentBlock::ToolResult { .. } => {}
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.send_inner(request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        if self.stream {
            self.stream_inner(request, on_event).await
        } else {
            // Server doesn't support SSE — do a blocking send and synthesise
            // the same event sequence a real SSE stream would have produced.
            let resp = self.send_inner(request).await?;
            emit_response_events(&resp, on_event);
            Ok(resp)
        }
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::OpenaiCompatible
    }

    async fn list_models(&self) -> Vec<ModelInfo> {
        self.models.iter().map(|m| self.model_info(m)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LlmRequest, Message, ToolDefinition};

    fn client() -> OpenAiCompatibleClient {
        OpenAiCompatibleClient::new(
            "http://192.29.35.246:8080/",
            "sk-test",
            "/raid/models/zai-org/GLM-5.2-FP8",
            vec![OpenAiCompatibleModel {
                id: "/raid/models/zai-org/GLM-5.2-FP8".into(),
                context_window: 131072,
                max_output: 8192,
            }],
            true,
        )
    }

    #[test]
    fn base_url_normalizes_trailing_slash_and_appends_v1() {
        let c = client();
        assert_eq!(
            c.completions_url(),
            "http://192.29.35.246:8080/v1/chat/completions"
        );
    }

    #[test]
    fn base_url_tolerates_explicit_v1() {
        let c = OpenAiCompatibleClient::new("http://host:8080/v1", "k", "m", vec![], true);
        assert_eq!(c.completions_url(), "http://host:8080/v1/chat/completions");
    }

    #[test]
    fn request_body_passes_model_verbatim_and_tools() {
        let c = client();
        let req = LlmRequest {
            model: "/raid/models/zai-org/GLM-5.2-FP8".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 2048,
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "read".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            ..Default::default()
        };
        let body = c.request_body(&req, false);
        assert_eq!(body["model"], "/raid/models/zai-org/GLM-5.2-FP8");
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn default_model_and_provider_id() {
        let c = client();
        assert_eq!(c.default_model(), "/raid/models/zai-org/GLM-5.2-FP8");
        assert_eq!(
            c.provider_id(),
            crate::provider_id::ProviderId::OpenaiCompatible
        );
    }

    #[tokio::test]
    async fn list_models_returns_configured_models() {
        let c = client();
        let models = c.list_models().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "/raid/models/zai-org/GLM-5.2-FP8");
        assert_eq!(models[0].context_window, 131072);
        assert_eq!(
            models[0].provider,
            crate::provider_id::ProviderId::OpenaiCompatible
        );
    }

    #[test]
    fn emit_response_events_surfaces_text_and_tool_calls() {
        use crate::types::{ContentBlock, LlmResponse, StopReason, Usage};
        let resp = LlmResponse {
            id: "1".into(),
            model: "m".into(),
            content: vec![
                ContentBlock::Text { text: "hi".into() },
                ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "a.rs"}),
                },
            ],
            stop_reason: Some(StopReason::ToolUse),
            usage: Usage::default(),
        };
        let mut events = Vec::new();
        emit_response_events(&resp, &mut |e| events.push(e));
        assert!(matches!(events[0], StreamEvent::TextDelta(ref t) if t == "hi"));
        assert!(
            matches!(events[1], StreamEvent::ToolUseStart { ref name, .. } if name == "read_file")
        );
        assert!(matches!(events[2], StreamEvent::InputJsonDelta(_)));
    }

    #[tokio::test]
    async fn list_models_streaming_false_omits_streaming_capability() {
        use crate::model_pool::ModelCapability;
        let c = OpenAiCompatibleClient::new(
            "http://host:8080",
            "k",
            "m",
            vec![OpenAiCompatibleModel {
                id: "m".into(),
                context_window: 4096,
                max_output: 1024,
            }],
            false,
        );
        let models = c.list_models().await;
        assert!(!models[0].capabilities.contains(&ModelCapability::Streaming));
        assert!(models[0].capabilities.contains(&ModelCapability::ToolUse));
    }
}
