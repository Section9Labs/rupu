//! BrokerClient: credential-brokered LLM access.
//!
//! Implements `LlmProvider` by signing requests with the cell's Ed25519 key
//! and sending them to the Credential Broker for proxying. Spec Phase 3B item 6.

use async_trait::async_trait;
use ed25519_dalek::{Signer, SigningKey};
use reqwest::Client;

use crate::broker_types::{BrokerRequest, LlmRequestWire};
use crate::error::ProviderError;
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;
use crate::sse::SseParser;
use crate::types::{ContentBlock, LlmRequest, LlmResponse, StopReason, StreamEvent, Usage};

/// Client that sends signed LLM requests to the Credential Broker.
pub struct BrokerClient {
    client: Client,
    broker_url: String,
    signing_key: SigningKey,
    nonce: u64,
}

impl BrokerClient {
    pub fn new(broker_url: String, signing_key: SigningKey) -> Self {
        // Random nonce seed so each process session produces disjoint nonce ranges.
        // Prevents nonce collision across process restarts (Spec Compliance HIGH).
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self {
            client: Client::new(),
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
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let body: serde_json::Value = response.json().await?;
        let resp = body
            .get("response")
            .ok_or_else(|| ProviderError::Json("missing 'response' field".into()))?;

        Ok(LlmResponse {
            id: resp["id"].as_str().unwrap_or_default().to_string(),
            model: resp["model"].as_str().unwrap_or_default().to_string(),
            content: serde_json::from_value(resp["content"].clone())
                .map_err(|e| ProviderError::Json(e.to_string()))?,
            stop_reason: serde_json::from_value(resp["stop_reason"].clone())
                .ok()
                .flatten(),
            usage: serde_json::from_value(resp["usage"].clone()).unwrap_or_default(),
        })
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
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let mut parser = SseParser::new();
        let mut text_acc = String::new();
        let mut tool_blocks: Vec<ContentBlock> = Vec::new();
        let mut current_tool_id: Option<String> = None;
        let mut current_tool_name: Option<String> = None;
        let mut current_tool_input = String::new();
        let mut usage = Usage::default();
        let response_id = String::new();
        let mut response = response;

        while let Some(chunk) = response.chunk().await? {
            let events = parser.feed(&chunk)?;
            for event in events {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                    match data["type"].as_str() {
                        Some("text_delta") => {
                            if let Some(text) = data["text"].as_str() {
                                text_acc.push_str(text);
                                on_event(StreamEvent::TextDelta(text.to_string()));
                            }
                        }
                        Some("tool_use_start") => {
                            // Finalize previous tool if any
                            if let (Some(id), Some(name)) =
                                (current_tool_id.take(), current_tool_name.take())
                            {
                                let input: serde_json::Value = if current_tool_input.is_empty() {
                                    serde_json::Value::Object(serde_json::Map::new())
                                } else {
                                    serde_json::from_str(&current_tool_input).map_err(|e| {
                                        ProviderError::Json(format!(
                                            "malformed tool input JSON: {e}"
                                        ))
                                    })?
                                };
                                tool_blocks.push(ContentBlock::ToolUse { id, name, input });
                                current_tool_input.clear();
                            }
                            let id = data["id"].as_str().unwrap_or_default().to_string();
                            let name = data["name"].as_str().unwrap_or_default().to_string();
                            current_tool_id = Some(id.clone());
                            current_tool_name = Some(name.clone());
                            on_event(StreamEvent::ToolUseStart { id, name });
                        }
                        Some("input_json_delta") => {
                            if let Some(json) = data["json"].as_str() {
                                current_tool_input.push_str(json);
                                on_event(StreamEvent::InputJsonDelta(json.to_string()));
                            }
                        }
                        Some("cost") => {
                            usage.input_tokens = data["input_tokens"].as_u64().unwrap_or(0) as u32;
                            usage.output_tokens =
                                data["output_tokens"].as_u64().unwrap_or(0) as u32;
                        }
                        _ => {}
                    }
                }
            }
        }

        // Finalize last tool block if pending
        if let (Some(id), Some(name)) = (current_tool_id.take(), current_tool_name.take()) {
            let input: serde_json::Value = if current_tool_input.is_empty() {
                serde_json::Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(&current_tool_input)
                    .map_err(|e| ProviderError::Json(format!("malformed tool input JSON: {e}")))?
            };
            tool_blocks.push(ContentBlock::ToolUse { id, name, input });
        }

        // Determine stop reason
        let stop_reason = if !tool_blocks.is_empty() {
            Some(StopReason::ToolUse)
        } else {
            Some(StopReason::EndTurn)
        };

        // Build content blocks
        let mut content = Vec::new();
        if !text_acc.is_empty() {
            content.push(ContentBlock::Text { text: text_acc });
        }
        content.extend(tool_blocks);

        Ok(LlmResponse {
            id: response_id,
            model: request.model.clone(),
            content,
            stop_reason,
            usage,
        })
    }

    fn default_model(&self) -> &str {
        "claude-sonnet-4-6"
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;
    use ed25519_dalek::Verifier;

    #[test]
    fn test_broker_client_signs_request() {
        let sk = SigningKey::from_bytes(&[42u8; 32]);
        let mut client = BrokerClient::new("http://localhost:9901".into(), sk);

        let request = LlmRequest {
            model: "claude-sonnet-4-6-20250514".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
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
        let mut client = BrokerClient::new("http://localhost:9901".into(), sk);
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
        let mut client = BrokerClient::new("http://localhost:9901".into(), sk);

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
}
