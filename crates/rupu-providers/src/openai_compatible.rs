//! Generic OpenAI-compatible provider.
//!
//! Speaks the OpenAI `/v1/chat/completions` API against a configurable base
//! URL with a static Bearer key. Covers self-hosted vLLM, Oracle GenAI,
//! Together, Fireworks, OpenRouter, and similar endpoints. Wire-format logic
//! is shared with the Copilot client via [`crate::openai_wire`].

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest_middleware::ClientWithMiddleware;
use std::sync::Arc;

use crate::error::ProviderError;
use crate::model_pool::{ModelCapability, ModelCost, ModelInfo, ModelState, ModelStatus};
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;
use crate::sse::SseParser;
use crate::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent};

/// vLLM-style `GET /v1/models` → `ModelInfo`s (spec §3). `max_model_len` is the
/// input limit; a LoRA entry with a null value inherits its `parent`'s (a
/// parent absent from the listing leaves the limit unknown, 0).
///
/// A body without a `data` array is an `Err` — never an empty catalog, which
/// would read as "this server serves no models".
pub(crate) fn vllm_models_from_listing(
    v: &serde_json::Value,
) -> Result<Vec<ModelInfo>, ProviderError> {
    let Some(data) = v.get("data").and_then(|d| d.as_array()) else {
        return Err(crate::error::listing_shape_error(
            "openai_compatible",
            "a `data` array",
            v,
        ));
    };
    let entries: Vec<&serde_json::Value> = data.iter().collect();
    let own = |e: &serde_json::Value| e.get("max_model_len").and_then(|x| x.as_u64());
    Ok(entries
        .iter()
        .filter_map(|e| {
            let id = e.get("id")?.as_str()?.to_string();
            let len = own(e).or_else(|| {
                let parent = e.get("parent")?.as_str()?;
                entries
                    .iter()
                    .find(|p| p.get("id").and_then(|x| x.as_str()) == Some(parent))
                    .and_then(|p| own(p))
            });
            Some(ModelInfo {
                id,
                provider: ProviderId::OpenaiCompatible,
                context_window: len.map_or(0, |n| n.min(u32::MAX as u64) as u32),
                max_output_tokens: 0,
                capabilities: Vec::new(),
                cost: crate::model_pool::ModelCost::default(),
                status: crate::model_pool::ModelStatus::default(),
            })
        })
        .collect())
}

/// A model offered by an OpenAI-compatible endpoint, declared in config.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleModel {
    pub id: String,
    pub context_window: u32,
    pub max_output: u32,
}

/// `max_tokens` sent to a `stream = false` server when no cap is known (the
/// pre-discovery default).
const NO_SSE_FALLBACK_MAX_TOKENS: u32 = 8192;

/// Client for an OpenAI-compatible `/v1/chat/completions` endpoint.
pub struct OpenAiCompatibleClient {
    base_url: String,
    api_key: String,
    default_model: String,
    models: Vec<OpenAiCompatibleModel>,
    stream: bool,
    client: ClientWithMiddleware,
    /// The exact sink `client` was bound to, so `with_tuning`'s rebuild
    /// keeps using the same run's sink rather than requiring the caller
    /// to pass it again. See `AnthropicClient`'s `sink` field.
    sink: Arc<dyn rupu_netflow::FlowSink>,
}

impl OpenAiCompatibleClient {
    /// Apply `[providers.<name>]` tuning — currently the `timeout_ms`
    /// inactivity deadline (ISSUES.md I-9) on this client's HTTP stack.
    /// Applied as connect + read timeouts so a long streaming generation is
    /// never cut off mid-flight. A builder failure leaves the existing client
    /// in place rather than panicking on user-supplied config.
    pub fn with_tuning(mut self, tuning: &crate::tuning::ProviderTuning) -> Self {
        let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Provider(
            "openai_compatible".into(),
        ));
        if let Ok(client) =
            rupu_netflow::http::shared_client(ctx, tuning.transport(), self.sink.clone())
        {
            self.client = client;
        }
        self
    }

    /// * `base_url` — endpoint root, with or without a trailing `/v1`
    ///   (e.g. `http://192.29.35.246:8080` or `…/v1`).
    /// * `api_key` — static Bearer key.
    /// * `default_model` — model id sent when the request doesn't override it.
    /// * `models` — config-declared models, surfaced via `list_models`.
    /// * `stream` — when false, never request SSE (servers without it).
    /// * `sink` — the run's netflow sink; there is no process-global fallback.
    pub fn new(
        base_url: &str,
        api_key: &str,
        default_model: &str,
        models: Vec<OpenAiCompatibleModel>,
        stream: bool,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        // Normalize: strip trailing slashes, then strip a trailing `/v1`
        // so we hold the bare root and append `/v1/...` consistently.
        let trimmed = base_url.trim_end_matches('/');
        let root = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
        let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Provider(
            "openai_compatible".into(),
        ));
        let client = rupu_netflow::http::shared_client(
            ctx,
            rupu_netflow::http::Transport::default(),
            sink.clone(),
        )
        .expect("reqwest TLS backend failed to initialise; no HTTP client can be built");
        Self {
            base_url: root.to_string(),
            api_key: api_key.to_string(),
            default_model: default_model.to_string(),
            models,
            stream,
            client,
            sink,
        }
    }

    fn completions_url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }

    fn request_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        let mut body = crate::openai_wire::build_chat_request_body(request, stream);
        // A server configured `stream = false` genuinely cannot stream, so
        // every response is one HTTP exchange and an unbounded generation
        // (servers default `max_tokens` to the whole remaining context) could
        // outlive the read timeout: keep the pre-discovery default of 8192
        // when no cap is known. Servers that stream keep an unknown cap
        // omitted, as the runner sends `None` for it.
        if request.max_tokens.is_none() && !self.stream {
            body["max_tokens"] = serde_json::json!(NO_SSE_FALLBACK_MAX_TOKENS);
        }
        body
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
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            return Err(crate::error::api_error_from_response(
                "openai-compatible",
                status,
                &headers,
                &text,
            ));
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
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            return Err(crate::error::api_error_from_response(
                "openai-compatible",
                status,
                &headers,
                &text,
            ));
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
fn emit_response_events(resp: &LlmResponse, on_event: &mut (dyn FnMut(StreamEvent) + Send)) {
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
            ContentBlock::Reasoning { .. } => {}
            ContentBlock::Unknown => {}
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

    async fn fetch_models(&mut self) -> Result<Vec<ModelInfo>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .headers(self.headers(false)?)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let text = resp.text().await.unwrap_or_default();
            return Err(crate::error::api_error_from_response(
                "openai-compatible",
                status.as_u16(),
                &headers,
                &text,
            ));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        let v: serde_json::Value = crate::error::parse_listing_json("openai_compatible", &body)?;
        vllm_models_from_listing(&v)
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
            Arc::new(rupu_netflow::NullSink),
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
        let c = OpenAiCompatibleClient::new(
            "http://host:8080/v1",
            "k",
            "m",
            vec![],
            true,
            Arc::new(rupu_netflow::NullSink),
        );
        assert_eq!(c.completions_url(), "http://host:8080/v1/chat/completions");
    }

    #[test]
    fn request_body_passes_model_verbatim_and_tools() {
        let c = client();
        let req = LlmRequest {
            model: "/raid/models/zai-org/GLM-5.2-FP8".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(2048),
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
        // Non-streaming requests must not carry stream_options.
        assert!(body.get("stream_options").is_none());
    }

    fn client_with_stream(stream: bool) -> OpenAiCompatibleClient {
        OpenAiCompatibleClient::new(
            "http://host:8080",
            "sk-test",
            "m",
            vec![],
            stream,
            Arc::new(rupu_netflow::NullSink),
        )
    }

    fn uncapped_request() -> LlmRequest {
        LlmRequest {
            model: "m".into(),
            messages: vec![Message::user("hi")],
            max_tokens: None,
            ..Default::default()
        }
    }

    #[test]
    fn stream_false_client_defaults_an_unset_cap_to_8192() {
        let body = client_with_stream(false).request_body(&uncapped_request(), false);
        assert_eq!(body["max_tokens"], 8192);
    }

    #[test]
    fn streaming_client_omits_an_unset_cap() {
        let body = client_with_stream(true).request_body(&uncapped_request(), true);
        assert!(body.get("max_tokens").is_none(), "{body}");
    }

    #[test]
    fn streaming_capable_client_omits_an_unset_cap_even_for_a_blocking_request() {
        // Only a server configured `stream = false` gets the 8192 default; one
        // that can stream is never capped just because this call is blocking.
        let body = client_with_stream(true).request_body(&uncapped_request(), false);
        assert!(body.get("max_tokens").is_none(), "{body}");
    }

    #[test]
    fn an_explicit_cap_is_never_replaced() {
        let mut req = uncapped_request();
        req.max_tokens = Some(4096);
        for (cfg, call) in [(false, false), (true, false), (true, true)] {
            let body = client_with_stream(cfg).request_body(&req, call);
            assert_eq!(body["max_tokens"], 4096, "cfg={cfg} call={call}");
        }
    }

    #[test]
    fn streaming_request_asks_server_to_include_usage() {
        // OpenAI-compatible servers (vLLM, OpenAI, …) only emit the final
        // usage chunk on a streamed response when the request sets
        // `stream_options.include_usage = true`. Without it, token accounting
        // for streamed runs comes back zero.
        let c = client();
        let req = LlmRequest {
            model: "/raid/models/zai-org/GLM-5.2-FP8".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(2048),
            ..Default::default()
        };
        let body = c.request_body(&req, true);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
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
        use crate::types::{ContentBlock, LlmResponse, Stop, StopReason, Usage};
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
            stop: Stop::synthetic(StopReason::ToolUse, "mock"),
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
            Arc::new(rupu_netflow::NullSink),
        );
        let models = c.list_models().await;
        assert!(!models[0].capabilities.contains(&ModelCapability::Streaming));
        assert!(models[0].capabilities.contains(&ModelCapability::ToolUse));
    }

    #[test]
    fn vllm_listing_reads_max_model_len_and_inherits_for_lora() {
        let v = serde_json::json!({ "object": "list", "data": [
            { "id": "base-model", "object": "model", "max_model_len": 131072, "parent": null },
            { "id": "my-lora", "object": "model", "max_model_len": null, "parent": "base-model" },
            { "id": "mystery", "object": "model" }
        ]});
        let ms = vllm_models_from_listing(&v).unwrap();
        let get = |id: &str| ms.iter().find(|m| m.id == id).unwrap().context_window;
        assert_eq!(get("base-model"), 131_072);
        assert_eq!(get("my-lora"), 131_072);
        assert_eq!(get("mystery"), 0);
    }

    #[tokio::test]
    async fn fetch_models_calls_v1_models() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .header("authorization", "Bearer k");
            then.status(200).json_body(
                serde_json::json!({ "data": [{ "id": "base-model", "max_model_len": 4096 }] }),
            );
        });
        let mut c = OpenAiCompatibleClient::new(
            &format!("{}/v1", server.url("")),
            "k",
            "base-model",
            vec![],
            true,
            Arc::new(rupu_netflow::NullSink),
        );
        let ms = <OpenAiCompatibleClient as LlmProvider>::fetch_models(&mut c)
            .await
            .unwrap();
        m.assert();
        assert_eq!(ms[0].context_window, 4096);
        assert_eq!(
            ms[0].provider,
            crate::provider_id::ProviderId::OpenaiCompatible
        );
    }

    #[test]
    fn vllm_listing_edge_cases() {
        let v = serde_json::json!({ "object": "list", "data": [
            { "id": "base-model", "object": "model", "max_model_len": 8192, "parent": null },
            // Parent is not in the listing: nothing to inherit.
            { "id": "orphan-lora", "object": "model", "max_model_len": null, "parent": "missing-base" },
            // Own value wins over the parent's.
            { "id": "tuned-lora", "object": "model", "max_model_len": 4096, "parent": "base-model" },
            // Beyond u32: clamps rather than wrapping.
            { "id": "huge", "object": "model", "max_model_len": 99999999999u64 },
            // Not an object with an id: skipped.
            { "object": "model", "max_model_len": 1 }
        ]});
        let ms = vllm_models_from_listing(&v).unwrap();
        assert_eq!(ms.len(), 4);
        let get = |id: &str| ms.iter().find(|m| m.id == id).unwrap();
        assert_eq!(get("base-model").context_window, 8192);
        assert_eq!(get("orphan-lora").context_window, 0);
        assert_eq!(get("tuned-lora").context_window, 4096);
        assert_eq!(get("huge").context_window, u32::MAX);
        // /v1/models never reports an output cap.
        assert!(ms.iter().all(|m| m.max_output_tokens == 0));
    }

    /// No `data` array: an error, not an empty catalog ("this server has no
    /// models"). An empty `data` array is genuinely empty.
    #[test]
    fn vllm_listing_without_data_is_a_shape_error() {
        for v in [
            serde_json::json!({}),
            serde_json::json!({ "object": "list" }),
            serde_json::json!({ "data": "nope" }),
            serde_json::json!([{ "id": "x" }]),
        ] {
            assert!(
                matches!(vllm_models_from_listing(&v), Err(ProviderError::Json(_))),
                "{v}"
            );
        }
        assert!(vllm_models_from_listing(&serde_json::json!({ "data": [] }))
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn fetch_models_without_a_data_array_is_an_error() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "object": "list" }));
        });
        let mut c = OpenAiCompatibleClient::new(
            &format!("{}/v1", server.url("")),
            "k",
            "base-model",
            vec![],
            true,
            Arc::new(rupu_netflow::NullSink),
        );
        let err = <OpenAiCompatibleClient as LlmProvider>::fetch_models(&mut c)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Json(_)), "{err:?}");
    }

    /// A 200 whose body is not JSON is a decode failure, not a transport
    /// failure.
    #[tokio::test]
    async fn fetch_models_non_json_body_is_a_json_error() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200).body("not json");
        });
        let mut c = OpenAiCompatibleClient::new(
            &format!("{}/v1", server.url("")),
            "k",
            "base-model",
            vec![],
            true,
            Arc::new(rupu_netflow::NullSink),
        );
        let err = <OpenAiCompatibleClient as LlmProvider>::fetch_models(&mut c)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Json(_)), "{err:?}");
    }

    #[tokio::test]
    async fn fetch_models_surfaces_non_2xx_as_error() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(401);
        });
        let mut c = OpenAiCompatibleClient::new(
            &format!("{}/v1", server.url("")),
            "k",
            "base-model",
            vec![],
            true,
            Arc::new(rupu_netflow::NullSink),
        );
        let err = <OpenAiCompatibleClient as LlmProvider>::fetch_models(&mut c)
            .await
            .unwrap_err();
        m.assert();
        assert_eq!(err.status(), Some(401));
    }
}
