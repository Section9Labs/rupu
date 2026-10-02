//! BrokerClient: credential-brokered LLM access.
//!
//! Implements `LlmProvider` by signing requests with the cell's Ed25519 key
//! and sending them to the Credential Broker for proxying. Spec Phase 3B item 6.

use async_trait::async_trait;
use ed25519_dalek::{Signer, SigningKey};
use reqwest_middleware::ClientWithMiddleware;
use std::sync::Arc;

use crate::broker_types::{BrokerRequest, LlmRequestWire};
use crate::error::ProviderError;
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;
use crate::reply_error::{parse_error_value, ErrorOrigin};
use crate::sse::SseParser;
use crate::types::{ContentBlock, LlmRequest, LlmResponse, Stop, StopReason, StreamEvent, Usage};

/// Client that sends signed LLM requests to the Credential Broker.
pub struct BrokerClient {
    client: ClientWithMiddleware,
    broker_url: String,
    signing_key: SigningKey,
    nonce: u64,
}

impl BrokerClient {
    pub fn new(
        broker_url: String,
        signing_key: SigningKey,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        // Random nonce seed so each process session produces disjoint nonce ranges.
        // Prevents nonce collision across process restarts (Spec Compliance HIGH).
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        // No run context is available at construction — stamped
        // `FlowCtx::system(Origin::Provider("broker"))`. Plan 2 threads
        // the real run id through once the provider factory is touched.
        let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Provider("broker".into()));
        let client =
            rupu_netflow::http::shared_client(ctx, rupu_netflow::http::Transport::default(), sink)
                .expect("reqwest TLS backend failed to initialise; no HTTP client can be built");
        Self {
            client,
            broker_url,
            signing_key,
            nonce: seed,
        }
    }

    fn next_nonce(&mut self) -> u64 {
        self.nonce += 1;
        self.nonce
    }

    fn sign_request(&mut self, wire: &LlmRequestWire) -> Result<BrokerRequest, ProviderError> {
        let nonce = self.next_nonce();
        let signable = BrokerRequest::signable_bytes(wire, nonce).map_err(ProviderError::Json)?;
        let signature = self.signing_key.sign(&signable);
        let vk = self.signing_key.verifying_key();

        Ok(BrokerRequest {
            request: wire.clone(),
            public_key: vk.as_bytes().iter().map(|b| format!("{:02x}", b)).collect(),
            signature: signature
                .to_bytes()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect(),
            nonce,
        })
    }
}

#[async_trait]
impl LlmProvider for BrokerClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let wire = LlmRequestWire::from(request);
        let broker_req = self.sign_request(&wire)?;

        let response = self
            .client
            .post(format!("{}/v1/llm/send", self.broker_url))
            .json(&broker_req)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            return Err(crate::error::api_error_from_response(
                "broker",
                status.as_u16(),
                &headers,
                &text,
            ));
        }

        let body: serde_json::Value = response.json().await?;
        let resp = body
            .get("response")
            .ok_or_else(|| ProviderError::Json("missing 'response' field".into()))?;

        parse_broker_response(resp)
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        let wire = LlmRequestWire::from(request);
        let broker_req = self.sign_request(&wire)?;

        let response = self
            .client
            .post(format!("{}/v1/llm/stream", self.broker_url))
            .json(&broker_req)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            return Err(crate::error::api_error_from_response(
                "broker",
                status.as_u16(),
                &headers,
                &text,
            ));
        }

        let mut parser = SseParser::new();
        let mut acc = BrokerStreamAccumulator::default();
        let mut response = response;

        while let Some(chunk) = response.chunk().await? {
            let events = parser.feed(&chunk)?;
            for event in events {
                match serde_json::from_str::<serde_json::Value>(&event.data) {
                    Ok(data) => acc.process(&data, on_event)?,
                    Err(e) => {
                        tracing::debug!(error = %e, "skipping unparseable broker stream event")
                    }
                }
            }
        }

        Ok(acc.finish(&request.model))
    }

    fn default_model(&self) -> &str {
        "claude-sonnet-4-6"
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::Broker
    }
}

/// Folds the broker's stream events into a reply.
#[derive(Default)]
struct BrokerStreamAccumulator {
    text: String,
    tool_blocks: Vec<ContentBlock>,
    current_tool: Option<(String, String)>,
    current_tool_input: String,
    usage: Usage,
    /// Only the last bad tool is kept, deliberately: one record is enough to
    /// name the failure, as in the other providers.
    bad_tool: Option<serde_json::Value>,
}

impl BrokerStreamAccumulator {
    fn process(
        &mut self,
        data: &serde_json::Value,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<(), ProviderError> {
        match data["type"].as_str() {
            Some("text_delta") => {
                if let Some(text) = data["text"].as_str() {
                    self.text.push_str(text);
                    on_event(StreamEvent::TextDelta(text.to_string()));
                }
            }
            Some("tool_use_start") => {
                self.finish_tool();
                let id = data["id"].as_str().unwrap_or_default().to_string();
                let name = data["name"].as_str().unwrap_or_default().to_string();
                self.current_tool = Some((id.clone(), name.clone()));
                on_event(StreamEvent::ToolUseStart { id, name });
            }
            Some("input_json_delta") => {
                if let Some(json) = data["json"].as_str() {
                    self.current_tool_input.push_str(json);
                    on_event(StreamEvent::InputJsonDelta(json.to_string()));
                }
            }
            Some("cost") => {
                self.usage.input_tokens = data["input_tokens"].as_u64().unwrap_or(0) as u32;
                self.usage.output_tokens = data["output_tokens"].as_u64().unwrap_or(0) as u32;
                on_event(StreamEvent::UsageSnapshot(self.usage.clone()));
            }
            Some("error") => {
                return Err(ProviderError::Reply(Box::new(parse_error_value(
                    "broker",
                    ErrorOrigin::Stream,
                    data,
                ))));
            }
            other => {
                tracing::debug!(event_type = ?other, "ignoring unknown broker stream event");
            }
        }
        Ok(())
    }

    /// Close the pending tool call: empty input is a zero-parameter call
    /// (`{}`); input that does not parse drops the call and is recorded as
    /// the bad tool.
    fn finish_tool(&mut self) {
        let input_text = std::mem::take(&mut self.current_tool_input);
        let Some((id, name)) = self.current_tool.take() else {
            return;
        };
        if input_text.trim().is_empty() {
            self.tool_blocks.push(ContentBlock::ToolUse {
                id,
                name,
                input: serde_json::json!({}),
            });
            return;
        }
        match serde_json::from_str(&input_text) {
            Ok(input) => self
                .tool_blocks
                .push(ContentBlock::ToolUse { id, name, input }),
            Err(e) => {
                tracing::warn!(tool = %name, error = %e, "dropping tool call with unparseable input");
                self.bad_tool = Some(serde_json::json!({
                    "name": name,
                    "id": id,
                    "error": e.to_string(),
                }));
            }
        }
    }

    fn finish(mut self, model: &str) -> LlmResponse {
        self.finish_tool();
        // The broker's stream carries no stop signal.
        let mut stop = if self.tool_blocks.is_empty() {
            Stop::synthetic(StopReason::EndTurn, "broker")
        } else {
            Stop::synthetic(StopReason::ToolUse, "broker")
        };
        stop.apply_bad_tool(self.bad_tool);
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        content.extend(self.tool_blocks);
        LlmResponse {
            id: String::new(),
            model: model.to_string(),
            content,
            stop,
            usage: self.usage,
        }
    }
}

/// Parse the broker's `response` object. The broker relays the upstream
/// stop string, so it is parsed tolerantly: a value this build does not know
/// is `Unrecognized` (keeping the raw string), and an absent one is
/// `Unreported`. `EndTurn` and `Unreported` become `ToolUse` when the content
/// carries tool blocks (the rule every provider shares), keeping the wire
/// value.
fn parse_broker_response(resp: &serde_json::Value) -> Result<LlmResponse, ProviderError> {
    let content: Vec<ContentBlock> = serde_json::from_value(resp["content"].clone())
        .map_err(|e| ProviderError::Json(e.to_string()))?;
    let wire = resp["stop_reason"].as_str();
    let reason = match wire {
        Some(v) => serde_json::from_value(serde_json::Value::String(v.to_string()))
            .unwrap_or(StopReason::Unrecognized),
        None => StopReason::Unreported,
    };
    let mut stop = Stop::from_wire(reason, "broker", wire);
    let has_tool_use = content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
    if has_tool_use && matches!(stop.reason, StopReason::EndTurn | StopReason::Unreported) {
        stop.reason = StopReason::ToolUse;
    }
    Ok(LlmResponse {
        id: resp["id"].as_str().unwrap_or_default().to_string(),
        model: resp["model"].as_str().unwrap_or_default().to_string(),
        content,
        stop,
        usage: serde_json::from_value(resp["usage"].clone()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;
    use ed25519_dalek::Verifier;

    fn sample_client() -> BrokerClient {
        BrokerClient::new(
            "http://localhost:9901".into(),
            SigningKey::from_bytes(&[42u8; 32]),
            std::sync::Arc::new(rupu_netflow::NullSink),
        )
    }

    #[test]
    fn provider_id_is_broker() {
        assert_eq!(sample_client().provider_id(), ProviderId::Broker);
    }

    #[test]
    fn broker_refusal_stop_is_parsed_with_its_wire_value() {
        let resp = serde_json::json!({
            "id": "r1", "model": "m",
            "content": [{"type": "text", "text": "no"}],
            "stop_reason": "refusal",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        });
        let r = parse_broker_response(&resp).unwrap();
        assert_eq!(r.stop.reason, StopReason::Refusal);
        assert_eq!(r.stop.wire.provider, "broker");
        assert_eq!(r.stop.wire.value.as_deref(), Some("refusal"));
    }

    #[test]
    fn broker_unknown_stop_is_unrecognized_and_keeps_the_raw_value() {
        let resp = serde_json::json!({
            "content": [{"type": "text", "text": "x"}],
            "stop_reason": "brand_new"
        });
        let r = parse_broker_response(&resp).unwrap();
        assert_eq!(r.stop.reason, StopReason::Unrecognized);
        assert_eq!(r.stop.wire.value.as_deref(), Some("brand_new"));
    }

    #[test]
    fn broker_missing_stop_is_unreported_or_tool_use() {
        let text = serde_json::json!({"content": [{"type": "text", "text": "x"}]});
        let r = parse_broker_response(&text).unwrap();
        assert_eq!(r.stop.reason, StopReason::Unreported);
        assert!(r.stop.wire.value.is_none());

        let tool = serde_json::json!({"content": [
            {"type": "tool_use", "id": "t1", "name": "bash", "input": {}}
        ]});
        let r = parse_broker_response(&tool).unwrap();
        assert_eq!(r.stop.reason, StopReason::ToolUse);
    }

    #[test]
    fn test_broker_client_signs_request() {
        let sk = SigningKey::from_bytes(&[42u8; 32]);
        let mut client = BrokerClient::new(
            "http://localhost:9901".into(),
            sk,
            std::sync::Arc::new(rupu_netflow::NullSink),
        );

        let request = LlmRequest {
            model: "claude-sonnet-4-6-20250514".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
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
            disable_prompt_cache: false,
        };

        let wire = LlmRequestWire::from(&request);
        let broker_req = client.sign_request(&wire).unwrap();
        assert!(!broker_req.public_key.is_empty());
        assert!(!broker_req.signature.is_empty());
        assert!(
            broker_req.nonce > 0,
            "nonce should be a non-zero timestamp-based seed"
        );
    }

    #[test]
    fn test_nonce_increments() {
        let sk = SigningKey::from_bytes(&[42u8; 32]);
        let mut client = BrokerClient::new(
            "http://localhost:9901".into(),
            sk,
            std::sync::Arc::new(rupu_netflow::NullSink),
        );
        let n1 = client.next_nonce();
        let n2 = client.next_nonce();
        let n3 = client.next_nonce();
        assert_eq!(n2, n1 + 1);
        assert_eq!(n3, n2 + 1);
    }

    #[test]
    fn test_signed_request_is_verifiable() {
        let sk = SigningKey::from_bytes(&[42u8; 32]);
        let vk = sk.verifying_key();
        let mut client = BrokerClient::new(
            "http://localhost:9901".into(),
            sk,
            std::sync::Arc::new(rupu_netflow::NullSink),
        );

        let wire = LlmRequestWire {
            model: "test".into(),
            system: None,
            messages: vec![],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
        };
        let broker_req = client.sign_request(&wire).unwrap();

        let signable =
            BrokerRequest::signable_bytes(&broker_req.request, broker_req.nonce).unwrap();
        let sig_bytes: Vec<u8> = (0..broker_req.signature.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&broker_req.signature[i..i + 2], 16).unwrap())
            .collect();
        let sig = ed25519_dalek::Signature::from_slice(&sig_bytes).unwrap();
        assert!(vk.verify(&signable, &sig).is_ok());
    }

    fn stream_request() -> LlmRequest {
        LlmRequest {
            model: "broker-model".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
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
            disable_prompt_cache: false,
        }
    }

    /// Run `BrokerClient::stream` against a broker that answers with these
    /// SSE `data:` payloads.
    async fn stream_events(events: &[serde_json::Value]) -> Result<LlmResponse, ProviderError> {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
        server.mock(|when, then| {
            when.method(POST).path("/v1/llm/stream");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(body);
        });
        let mut client = BrokerClient::new(
            server.base_url(),
            SigningKey::from_bytes(&[42u8; 32]),
            std::sync::Arc::new(rupu_netflow::NullSink),
        );
        client.stream(&stream_request(), &mut |_| {}).await
    }

    #[tokio::test]
    async fn stream_with_unparseable_tool_input_is_a_malformed_tool_call() {
        let resp = stream_events(&[
            serde_json::json!({"type": "text_delta", "text": "working"}),
            serde_json::json!({"type": "tool_use_start", "id": "t1", "name": "bash"}),
            serde_json::json!({"type": "input_json_delta", "json": "{\"command\": \"ls"}),
        ])
        .await
        .expect("a bad tool input is reported in the stop, not an error");
        assert_eq!(resp.stop.reason, StopReason::MalformedToolCall);
        let bad = &resp.stop.wire.details.as_ref().unwrap()["malformed_tool"];
        assert_eq!(bad["name"], "bash");
        assert_eq!(bad["id"], "t1");
        assert!(bad["error"].is_string());
        assert!(!resp
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. })));
        assert!(matches!(&resp.content[0], ContentBlock::Text { text } if text == "working"));
    }

    #[tokio::test]
    async fn stream_keeps_good_calls_and_empty_input_is_an_empty_object() {
        let resp = stream_events(&[
            serde_json::json!({"type": "tool_use_start", "id": "t1", "name": "list"}),
            serde_json::json!({"type": "tool_use_start", "id": "t2", "name": "bash"}),
            serde_json::json!({"type": "input_json_delta", "json": "{\"command\":\"ls\"}"}),
            serde_json::json!({"type": "brand_new_event", "x": 1}),
        ])
        .await
        .unwrap();
        assert_eq!(resp.stop.reason, StopReason::ToolUse);
        assert!(resp.stop.wire.details.is_none());
        let tools: Vec<_> = resp
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, input, .. } => Some((id.as_str(), input.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            tools,
            vec![
                ("t1", serde_json::json!({})),
                ("t2", serde_json::json!({"command": "ls"}))
            ]
        );
    }

    #[tokio::test]
    async fn stream_error_event_is_a_typed_stream_error() {
        let err = stream_events(&[
            serde_json::json!({"type": "text_delta", "text": "par"}),
            serde_json::json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
        ])
        .await
        .unwrap_err();
        let body = err.reply().expect("a typed reply error");
        assert_eq!(body.provider, "broker");
        assert_eq!(body.origin, ErrorOrigin::Stream);
        assert_eq!(body.class, crate::reply_error::ErrorClass::Overloaded);
        assert!(body.is_retryable());
    }

    #[tokio::test]
    async fn stream_flat_error_event_without_a_kind_is_retried() {
        let err =
            stream_events(&[serde_json::json!({"type": "error", "message": "broker hiccup"})])
                .await
                .unwrap_err();
        let body = err.reply().expect("a typed reply error");
        assert_eq!(body.kind, None);
        assert_eq!(body.class, crate::reply_error::ErrorClass::Unrecognized);
        assert!(body.is_retryable());

        let err = stream_events(&[serde_json::json!({
            "type": "error", "code": "rate_limit_exceeded", "message": "slow"})])
        .await
        .unwrap_err();
        let body = err.reply().expect("a typed reply error");
        assert_eq!(body.kind.as_deref(), Some("rate_limit_exceeded"));
        assert_eq!(body.class, crate::reply_error::ErrorClass::RateLimited);
    }

    /// The shared promotion rule: `EndTurn` or `Unreported` with tool blocks
    /// is `ToolUse`, and the wire value is kept; any other reason stays.
    #[test]
    fn broker_end_turn_with_tool_blocks_is_tool_use() {
        let tool = |stop: &str| {
            serde_json::json!({"content": [
                {"type": "tool_use", "id": "t1", "name": "bash", "input": {}}
            ], "stop_reason": stop})
        };
        let r = parse_broker_response(&tool("end_turn")).unwrap();
        assert_eq!(r.stop.reason, StopReason::ToolUse);
        assert_eq!(r.stop.wire.value.as_deref(), Some("end_turn"));

        let r = parse_broker_response(&tool("brand_new")).unwrap();
        assert_eq!(r.stop.reason, StopReason::Unrecognized);
        let r = parse_broker_response(&tool("max_tokens")).unwrap();
        assert_eq!(r.stop.reason, StopReason::MaxTokens);
    }
}
