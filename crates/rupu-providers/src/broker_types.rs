//! Wire types for Credential Broker ↔ BrokerClient communication.
//!
//! Shared by both sides. Kept in rupu-providers to avoid circular deps.

use serde::{Deserialize, Serialize};

use crate::types::LlmRequest;

/// Request from a cell to the Credential Broker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerRequest {
    /// The LLM request to proxy.
    pub request: LlmRequestWire,
    /// Cell identity: hex-encoded Ed25519 public key.
    pub public_key: String,
    /// Ed25519 signature over the signable bytes (hex-encoded).
    pub signature: String,
    /// Nonce for replay protection.
    pub nonce: u64,
}

/// Wire-serializable LLM request (mirrors LlmRequest but with Serialize/Deserialize).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequestWire {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<crate::types::Message>,
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<crate::types::ToolDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<crate::model_tier::ThinkingLevel>,
}

impl From<&LlmRequest> for LlmRequestWire {
    fn from(r: &LlmRequest) -> Self {
        Self {
            model: r.model.clone(),
            system: r.system.clone(),
            messages: r.messages.clone(),
            max_tokens: r.max_tokens,
            tools: r.tools.clone(),
            cell_id: r.cell_id.clone(),
            trace_id: r.trace_id.clone(),
            thinking: r.thinking,
        }
    }
}

impl From<LlmRequestWire> for LlmRequest {
    fn from(w: LlmRequestWire) -> Self {
        Self {
            model: w.model,
            system: w.system,
            messages: w.messages,
            max_tokens: w.max_tokens,
            tools: w.tools,
            cell_id: w.cell_id,
            trace_id: w.trace_id,
            thinking: w.thinking,
            context_window: None,
            task_type: None,
        }
    }
}

impl BrokerRequest {
    /// Compute the bytes that are signed: JSON of the wire request + nonce.
    /// Returns Err if the request cannot be serialized.
    pub fn signable_bytes(request: &LlmRequestWire, nonce: u64) -> Result<Vec<u8>, String> {
        let mut bytes = serde_json::to_vec(request)
            .map_err(|e| format!("failed to serialize request for signing: {e}"))?;
        bytes.extend_from_slice(&nonce.to_le_bytes());
        Ok(bytes)
    }
}

/// Error response from the Credential Broker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerError {
    pub error: String,
    pub code: String,
}

/// Budget status for a cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetStatus {
    pub cell_id: String,
    pub daily_limit_usd: f64,
    pub spent_today_usd: f64,
    pub remaining_usd: f64,
    pub calls_today: u64,
}

/// Cost of a single LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallCost {
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_request_wire_roundtrip() {
        let wire = LlmRequestWire {
            model: "claude-sonnet-4-6-20250514".into(),
            system: Some("Be helpful.".into()),
            messages: vec![crate::types::Message::user("hi")],
            max_tokens: 1024,
            tools: vec![],
            cell_id: Some("test-cell".into()),
            trace_id: None,
            thinking: None,
        };
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: LlmRequestWire = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, "claude-sonnet-4-6-20250514");
        assert_eq!(parsed.max_tokens, 1024);
    }

    #[test]
    fn test_broker_request_signable_bytes_deterministic() {
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
        let bytes1 = BrokerRequest::signable_bytes(&wire, 42).unwrap();
        let bytes2 = BrokerRequest::signable_bytes(&wire, 42).unwrap();
        assert_eq!(bytes1, bytes2);
        let bytes3 = BrokerRequest::signable_bytes(&wire, 43).unwrap();
        assert_ne!(bytes1, bytes3);
    }

    #[test]
    fn test_budget_status_serde() {
        let status = BudgetStatus {
            cell_id: "ed25519:abc".into(),
            daily_limit_usd: 5.0,
            spent_today_usd: 1.23,
            remaining_usd: 3.77,
            calls_today: 10,
        };
        let json = serde_json::to_string(&status).unwrap();
        let parsed: BudgetStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.calls_today, 10);
    }

    #[test]
    fn test_call_cost_serde() {
        let cost = CallCost {
            model: "claude-sonnet-4-6-20250514".into(),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.001,
        };
        let json = serde_json::to_string(&cost).unwrap();
        let parsed: CallCost = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.cost_usd, 0.001);
    }

    #[test]
    fn test_llm_request_wire_from_llm_request() {
        let request = LlmRequest {
            model: "claude-sonnet-4-6-20250514".into(),
            system: Some("sys".into()),
            messages: vec![crate::types::Message::user("hi")],
            max_tokens: 1024,
            tools: vec![],
            cell_id: Some("cell".into()),
            trace_id: Some("trace".into()),
            thinking: None,
            context_window: None,
            task_type: None,
        };
        let wire = LlmRequestWire::from(&request);
        assert_eq!(wire.model, request.model);
        assert_eq!(wire.system, request.system);

        let back: LlmRequest = wire.into();
        assert_eq!(back.model, "claude-sonnet-4-6-20250514");
        assert_eq!(back.cell_id, Some("cell".into()));
    }

    #[test]
    fn test_llm_request_wire_thinking_roundtrip() {
        use crate::model_tier::ThinkingLevel;
        let wire = LlmRequestWire {
            model: "claude-opus-4-6".into(),
            system: None,
            messages: vec![],
            max_tokens: 32000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(ThinkingLevel::High),
        };
        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains("high"), "thinking level should be serialized");
        let parsed: LlmRequestWire = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.thinking, Some(ThinkingLevel::High));

        // Verify LlmRequest ↔ LlmRequestWire roundtrip
        let req = LlmRequest::from(wire);
        assert_eq!(req.thinking, Some(ThinkingLevel::High));
        let wire2 = LlmRequestWire::from(&req);
        assert_eq!(wire2.thinking, Some(ThinkingLevel::High));
    }
}
