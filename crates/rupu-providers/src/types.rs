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
}

/// Events emitted during streaming via the callback.
/// The complete `LlmResponse` is returned by `stream()` — not delivered via callback.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of text content.
    TextDelta(String),
    /// A tool use block is starting.
    ToolUseStart { id: String, name: String },
    /// A chunk of tool input JSON.
    InputJsonDelta(String),
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
}
