use serde::{Deserialize, Serialize};

/// Role in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A content block within a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },

    /// Model reasoning/thinking, provider-agnostic.
    ///
    /// `raw` is the producing provider's original block, echoed back to that
    /// provider **byte-exact** on the next turn. It is never parsed, edited, or
    /// reconstructed — the API rejects modified blocks. `text` is the readable
    /// summary for the transcript/UI only.
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Canonical provider tag. Gates the echo: a provider emits `raw` iff
        /// this matches its own tag. Deliberately NOT gated on `model` —
        /// thinking blocks replay across models fine, and stripping them is
        /// what triggers ordering/signature 400s.
        provider: String,
        /// Informational only (transcript/debugging). Never an echo gate.
        model: String,
        raw: serde_json::Value,
    },

    /// Forward-compatibility catch-all: an unrecognized block type lands here
    /// instead of failing the whole turn's deserialization.
    #[serde(other)]
    Unknown,
}

/// A conversation message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: &str) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn assistant(text: &str) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn tool_result(tool_use_id: &str, content: &str, is_error: bool) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error,
            }],
        }
    }
}

/// A tool definition for the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
}

/// Token usage for a request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default, alias = "cache_read_input_tokens")]
    pub cached_tokens: u32,
}

/// A request to an LLM provider.
///
/// `Default` is derived so callers can use `..Default::default()`
/// to fill in fields they don't care about (especially the
/// optional reasoning / context / output-format / fast-mode
/// knobs). The `model` field defaults to an empty string — that's
/// not a valid send target, but no production path constructs
/// `LlmRequest::default()` and sends it; the derive is purely an
/// ergonomic convenience for partial literals and tests.
#[derive(Debug, Clone, Default)]
pub struct LlmRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub tools: Vec<ToolDefinition>,
    /// Identity of the requesting cell (for cost tracking and audit).
    pub cell_id: Option<String>,
    /// Distributed trace ID propagated from the originating request.
    pub trace_id: Option<String>,
    /// Unified reasoning/effort level. Providers that support reasoning
    /// translate this to their native format; others silently ignore it.
    pub thinking: Option<crate::model_tier::ThinkingLevel>,
    /// Desired context-window tier. Anthropic 1M-context is gated on
    /// the `context-1m-2025-08-07` beta header; setting this to
    /// `OneMillion` ensures the beta is sent on api-key requests (the
    /// OAuth path already includes it via the static beta CSV). Other
    /// providers currently ignore this.
    pub context_window: Option<crate::model_tier::ContextWindow>,
    /// Task type for smart routing. Set by TaskClassifier or explicitly by caller.
    pub task_type: Option<crate::task_classifier::TaskType>,
    /// Cross-provider output-format hint. Anthropic emits as
    /// `output_config.format`; OpenAI emits as `response_format.type`
    /// (with the right shape per their schema). `None` leaves the
    /// model free to choose. Other providers currently ignore this.
    pub output_format: Option<OutputFormat>,
    /// JSON Schema for Anthropic structured outputs. When present,
    /// Anthropic emits `output_config.format = {type: "json_schema",
    /// schema: <this value>}`, which guarantees schema-conforming
    /// output. Anthropic requires a real schema for `format` — there
    /// is no schema-less JSON mode — so this is `None` unless the
    /// agent declares an `outputSchema`. Other providers currently
    /// ignore this.
    pub output_schema: Option<serde_json::Value>,
    /// Anthropic-only soft cap on output tokens. Distinct from
    /// `max_tokens` (hard ceiling): the model self-paces toward this
    /// budget. Emitted as `output_config.task_budget`. Ignored by
    /// other providers.
    pub anthropic_task_budget: Option<u32>,
    /// Anthropic-only auto context-management strategy. Emitted as
    /// `context_management: { type: "tool_clearing", ... }`. Lets
    /// the server transparently drop earlier `tool_use` /
    /// `tool_result` blocks when the conversation would otherwise
    /// overflow. Ignored by other providers.
    pub anthropic_context_management: Option<ContextManagement>,
    /// Anthropic-only fast-mode toggle. Account-gated; sending
    /// `Speed::Fast` from an account that doesn't have the feature
    /// enabled returns 400. Emitted as the top-level `speed: "fast"`
    /// body field. Ignored by other providers.
    pub anthropic_speed: Option<Speed>,
}

/// Output-format hint passed to providers that support structured
/// outputs. Anthropic and OpenAI map this onto their respective
/// body fields; other providers ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Text,
    Json,
}

/// Anthropic auto context-management strategy. Today the server
/// supports `tool_clearing`; the enum is open to keep room for
/// future strategies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextManagement {
    /// Drop earlier `tool_use` / `tool_result` blocks when the
    /// conversation would otherwise overflow. Latest tool calls and
    /// the assistant's final reasoning are preserved.
    ToolClearing,
}

/// Anthropic fast-mode toggle. Currently a single-variant enum
/// because that's all the server accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Fast,
}

/// A complete response from an LLM provider.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub id: String,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
}

impl LlmResponse {
    /// Extract the text content from the response, if any.
    pub fn text(&self) -> Option<&str> {
        self.content.iter().find_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
    }

    /// Extract all tool use blocks from the response.
    pub fn tool_calls(&self) -> Vec<&ContentBlock> {
        self.content
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .collect()
    }

    /// Concatenated readable reasoning for this turn, if any.
    ///
    /// Blocks with no readable text (redacted, or `display: "omitted"`) are
    /// skipped here but still round-trip via their `raw` payload.
    pub fn reasoning_text(&self) -> Option<String> {
        let parts: Vec<&str> = self
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Reasoning { text: Some(t), .. } if !t.is_empty() => Some(t.as_str()),
                _ => None,
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }
}

/// Events emitted during streaming via the callback.
/// The complete `LlmResponse` is returned by `stream()` — not delivered via callback.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of text content.
    TextDelta(String),
    /// Current provider-reported usage snapshot while streaming.
    UsageSnapshot(Usage),
    /// A tool use block is starting.
    ToolUseStart { id: String, name: String },
    /// A chunk of tool input JSON.
    InputJsonDelta(String),
    /// A chunk of reasoning/thinking text.
    ReasoningDelta(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_user_constructor() {
        let msg = Message::user("hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected Text block"),
        }
    }

    #[test]
    fn test_message_assistant_constructor() {
        let msg = Message::assistant("hi there");
        assert_eq!(msg.role, Role::Assistant);
    }

    #[test]
    fn test_message_tool_result_constructor() {
        let msg = Message::tool_result("tool_123", "result data", false);
        assert_eq!(msg.role, Role::User);
        match &msg.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_123");
                assert_eq!(content, "result data");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult block"),
        }
    }

    #[test]
    fn test_content_block_text_serde() {
        let block = ContentBlock::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        let roundtripped: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(block, roundtripped);
    }

    #[test]
    fn test_content_block_tool_use_serde() {
        let block = ContentBlock::ToolUse {
            id: "toolu_123".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/test.txt"}),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"tool_use\""));
        let roundtripped: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(block, roundtripped);
    }

    #[test]
    fn test_stop_reason_serde() {
        let sr = StopReason::ToolUse;
        let json = serde_json::to_string(&sr).unwrap();
        assert_eq!(json, "\"tool_use\"");
        let roundtripped: StopReason = serde_json::from_str(&json).unwrap();
        assert_eq!(sr, roundtripped);
    }

    #[test]
    fn test_role_serde() {
        let role = Role::Assistant;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"assistant\"");
    }

    #[test]
    fn test_llm_response_text_helper() {
        let response = LlmResponse {
            id: "msg_123".into(),
            model: "claude-sonnet-4-6".into(),
            content: vec![ContentBlock::Text {
                text: "Hello!".into(),
            }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
        };
        assert_eq!(response.text(), Some("Hello!"));
        assert!(response.tool_calls().is_empty());
    }

    #[test]
    fn test_llm_response_tool_calls_helper() {
        let response = LlmResponse {
            id: "msg_456".into(),
            model: "claude-sonnet-4-6".into(),
            content: vec![
                ContentBlock::Text {
                    text: "Let me check.".into(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({}),
                },
            ],
            stop_reason: Some(StopReason::ToolUse),
            usage: Usage::default(),
        };
        assert_eq!(response.tool_calls().len(), 1);
    }

    #[test]
    fn test_content_block_tool_result_serde_roundtrip() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_789".into(),
            content: "ok".into(),
            is_error: false,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"tool_result\""));
        let roundtripped: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(block, roundtripped);
    }

    #[test]
    fn test_content_block_tool_result_is_error_true() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "t1".into(),
            content: "Error: not found".into(),
            is_error: true,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"is_error\":true"));
        let roundtripped: ContentBlock = serde_json::from_str(&json).unwrap();
        match roundtripped {
            ContentBlock::ToolResult { is_error, .. } => assert!(is_error),
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_content_block_tool_result_default_is_error() {
        let json = r#"{"type":"tool_result","tool_use_id":"t1","content":"ok"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        match block {
            ContentBlock::ToolResult { is_error, .. } => assert!(!is_error),
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_stop_reason_all_variants_serde() {
        for (variant, expected_str) in [
            (StopReason::EndTurn, "\"end_turn\""),
            (StopReason::MaxTokens, "\"max_tokens\""),
            (StopReason::StopSequence, "\"stop_sequence\""),
            (StopReason::ToolUse, "\"tool_use\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_str);
            let roundtripped: StopReason = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, roundtripped);
        }
    }

    #[test]
    fn test_llm_response_text_returns_none_when_empty() {
        let response = LlmResponse {
            id: "msg_1".into(),
            model: "m".into(),
            content: vec![],
            stop_reason: None,
            usage: Usage::default(),
        };
        assert_eq!(response.text(), None);
        assert!(response.tool_calls().is_empty());
    }

    #[test]
    fn test_tool_definition_serde() {
        let tool = ToolDefinition {
            name: "read_file".into(),
            description: "Read a file from disk".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path"}
                },
                "required": ["path"]
            }),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let roundtripped: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtripped.name, "read_file");
        assert_eq!(roundtripped.description, "Read a file from disk");
    }

    #[test]
    fn test_provider_error_display_messages() {
        use crate::error::ProviderError;

        let e = ProviderError::MissingAuth {
            provider: "anthropic".into(),
            env_hint: "ANTHROPIC_API_KEY".into(),
        };
        assert!(e.to_string().contains("ANTHROPIC_API_KEY"));
        assert!(e.to_string().contains("anthropic"));

        let e = ProviderError::Api {
            status: 401,
            message: "Unauthorized".into(),
        };
        let s = e.to_string();
        assert!(s.contains("401"));
        assert!(s.contains("Unauthorized"));

        let e = ProviderError::SseParse("bad utf8".into());
        assert!(e.to_string().contains("bad utf8"));

        let e = ProviderError::UnexpectedEndOfStream;
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn test_provider_error_from_serde_json() {
        use crate::error::ProviderError;
        let json_err = serde_json::from_str::<i32>("not-a-number").unwrap_err();
        let e = ProviderError::from(json_err);
        assert!(matches!(e, ProviderError::Json(_)));
    }

    fn test_response(content: Vec<ContentBlock>) -> LlmResponse {
        LlmResponse {
            id: "msg_1".into(),
            model: "m".into(),
            content,
            stop_reason: None,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cached_tokens: 0,
            },
        }
    }

    #[test]
    fn reasoning_block_serde_round_trip() {
        let block = ContentBlock::Reasoning {
            text: Some("weighing options".into()),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            raw: serde_json::json!({"type": "thinking", "thinking": "weighing options", "signature": "abc123"}),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "reasoning");
        let back: ContentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn reasoning_block_round_trips_with_empty_text() {
        // display:"omitted" returns thinking blocks whose text is empty; they must
        // survive the round trip so they can be echoed back unchanged.
        let block = ContentBlock::Reasoning {
            text: None,
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            raw: serde_json::json!({"type": "thinking", "thinking": "", "signature": "sig"}),
        };
        let back: ContentBlock =
            serde_json::from_value(serde_json::to_value(&block).unwrap()).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn unknown_block_type_deserializes_instead_of_erroring() {
        // Regression guard: a strict tagged enum used to fail the whole turn.
        let json = serde_json::json!({"type": "some_future_block", "payload": 1});
        let block: ContentBlock =
            serde_json::from_value(json).expect("unknown block must not error");
        assert_eq!(block, ContentBlock::Unknown);
    }

    #[test]
    fn reasoning_does_not_leak_into_text_or_tool_calls() {
        let resp = test_response(vec![
            ContentBlock::Reasoning {
                text: Some("hmm".into()),
                provider: "anthropic".into(),
                model: "m".into(),
                raw: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "answer".into(),
            },
        ]);
        assert_eq!(resp.text(), Some("answer"));
        assert!(resp.tool_calls().is_empty());
    }

    #[test]
    fn reasoning_text_concatenates_blocks_and_skips_textless() {
        let resp = test_response(vec![
            ContentBlock::Reasoning {
                text: Some("first".into()),
                provider: "anthropic".into(),
                model: "m".into(),
                raw: serde_json::json!({}),
            },
            ContentBlock::Reasoning {
                text: None, // redacted: opaque, nothing readable
                provider: "anthropic".into(),
                model: "m".into(),
                raw: serde_json::json!({}),
            },
            ContentBlock::Reasoning {
                text: Some("second".into()),
                provider: "anthropic".into(),
                model: "m".into(),
                raw: serde_json::json!({}),
            },
        ]);
        assert_eq!(resp.reasoning_text().as_deref(), Some("first\n\nsecond"));
    }

    #[test]
    fn reasoning_text_is_none_without_reasoning_blocks() {
        let resp = test_response(vec![ContentBlock::Text { text: "hi".into() }]);
        assert_eq!(resp.reasoning_text(), None);
    }
}
