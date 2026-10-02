//! How an LLM reply ended, typed (spec 2026-10-01 response-outcomes §4.1).
//!
//! Every provider parses its own stop/finish signal into a [`Stop`]. The
//! provider's exact words always survive in [`Stop::wire`], so a reason rupu
//! doesn't know is still rendered — it becomes [`StopReason::Unrecognized`],
//! never an error and never a silent default.

use serde::{Deserialize, Serialize};

/// Normalized reason a reply ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    StopSequence,
    MaxTokens,
    ContextWindowExceeded,
    PauseTurn,
    Refusal,
    Safety,
    MalformedToolCall,
    Incomplete,
    /// The provider reported no stop reason at all.
    Unreported,
    /// A value rupu doesn't know; [`Stop::wire`] carries it.
    #[serde(other)]
    Unrecognized,
}

impl StopReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::ToolUse => "tool_use",
            StopReason::StopSequence => "stop_sequence",
            StopReason::MaxTokens => "max_tokens",
            StopReason::ContextWindowExceeded => "context_window_exceeded",
            StopReason::PauseTurn => "pause_turn",
            StopReason::Refusal => "refusal",
            StopReason::Safety => "safety",
            StopReason::MalformedToolCall => "malformed_tool_call",
            StopReason::Incomplete => "incomplete",
            StopReason::Unreported => "unreported",
            StopReason::Unrecognized => "unrecognized",
        }
    }
}

/// The provider's own words for how the reply ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireStop {
    /// `ProviderId`'s display string (`anthropic`, `openai-codex`, …), or
    /// `mock` / `local` / `broker`.
    pub provider: String,
    /// e.g. `refusal`, `content_filter`, `SAFETY`, `max_output_tokens`.
    /// `None` when the provider sent nothing.
    pub value: Option<String>,
    /// Provider extras: `stop_sequence`, `finishMessage`, `safetyRatings`,
    /// `incomplete_details`, `blockReason`, `truncated_tool`, `malformed_tool`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalSource {
    /// A provider safety classifier declined (Anthropic `stop_details`).
    Classifier,
    /// The model itself declined (an OpenAI `refusal` field/part).
    Model,
    /// A provider policy error code (OpenAI `bio_policy`, …).
    Policy,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefusalDetail {
    /// `cyber`, `bio`, … — or `None`, a valid permanent value.
    pub category: Option<String>,
    /// Display text only; never parsed.
    pub explanation: Option<String>,
    pub recommended_model: Option<String>,
    pub source: RefusalSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackHop {
    pub from_model: String,
    pub to_model: String,
}

/// Anthropic server-side fallback: which model actually served the reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServedBy {
    pub model: String,
    pub hops: Vec<FallbackHop>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub reason: StopReason,
    pub wire: WireStop,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<RefusalDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_by: Option<ServedBy>,
}

impl Stop {
    /// A stop parsed off a provider's wire. `value` is exactly what it sent.
    pub fn from_wire(reason: StopReason, provider: &str, value: Option<&str>) -> Stop {
        Stop {
            reason,
            wire: WireStop {
                provider: provider.to_string(),
                value: value.map(str::to_string),
                details: None,
            },
            refusal: None,
            served_by: None,
        }
    }

    /// A stop no provider sent — mock scripts and providers whose wire has
    /// no stop signal. Its wire value is the reason's own name.
    pub fn synthetic(reason: StopReason, provider: &str) -> Stop {
        let value = reason.as_str();
        Stop::from_wire(reason, provider, Some(value))
    }

    /// Insert `key` into `wire.details` (created as an object when absent).
    pub fn set_detail(&mut self, key: &str, value: serde_json::Value) {
        let details = self
            .wire
            .details
            .get_or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(obj) = details.as_object_mut() {
            obj.insert(key.to_string(), value);
        }
    }

    /// The provider's value, or the reason's name when it sent none —
    /// what `TurnEnd.stop_reason` records.
    pub fn wire_value_or_reason(&self) -> String {
        self.wire
            .value
            .clone()
            .unwrap_or_else(|| self.reason.as_str().to_string())
    }
}

/// Per-provider wire value → [`StopReason`] tables (spec §4.2). Pure, so
/// each row is pinned by a test. Tool-use precedence (a function call in
/// the output) is applied by the providers, not here.
pub mod map {
    use super::StopReason;

    pub fn anthropic(value: &str) -> StopReason {
        match value {
            "end_turn" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "stop_sequence" => StopReason::StopSequence,
            "max_tokens" => StopReason::MaxTokens,
            "model_context_window_exceeded" => StopReason::ContextWindowExceeded,
            "pause_turn" => StopReason::PauseTurn,
            "refusal" => StopReason::Refusal,
            _ => StopReason::Unrecognized,
        }
    }

    /// Chat-completions `finish_reason` (OpenAI, Copilot, vLLM, local).
    pub fn chat_finish(value: &str) -> StopReason {
        match value {
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            "tool_calls" | "function_call" => StopReason::ToolUse,
            "content_filter" => StopReason::Safety,
            "abort" => StopReason::Incomplete,
            _ => StopReason::Unrecognized,
        }
    }

    /// OpenAI Responses `incomplete_details.reason`.
    pub fn responses_incomplete(reason: Option<&str>) -> StopReason {
        match reason {
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Safety,
            _ => StopReason::Incomplete,
        }
    }

    /// Gemini `candidate.finishReason`.
    pub fn gemini_finish(value: &str) -> StopReason {
        match value {
            "STOP" => StopReason::EndTurn,
            "MAX_TOKENS" => StopReason::MaxTokens,
            "SAFETY"
            | "RECITATION"
            | "LANGUAGE"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "IMAGE_SAFETY"
            | "IMAGE_PROHIBITED_CONTENT"
            | "IMAGE_RECITATION"
            | "IMAGE_OTHER"
            | "NO_IMAGE" => StopReason::Safety,
            "MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL" | "TOO_MANY_TOOL_CALLS" => {
                StopReason::MalformedToolCall
            }
            "OTHER" => StopReason::Incomplete,
            "FINISH_REASON_UNSPECIFIED" => StopReason::Unreported,
            _ => StopReason::Unrecognized,
        }
    }

    /// Gemini `promptFeedback.blockReason` values the API documents.
    pub fn gemini_block_reason_is_known(value: &str) -> bool {
        matches!(
            value,
            "BLOCK_REASON_UNSPECIFIED"
                | "SAFETY"
                | "OTHER"
                | "BLOCKLIST"
                | "PROHIBITED_CONTENT"
                | "IMAGE_SAFETY"
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reason_round_trips_snake_case_and_unknown_is_unrecognized() {
        for r in [
            StopReason::EndTurn,
            StopReason::ToolUse,
            StopReason::StopSequence,
            StopReason::MaxTokens,
            StopReason::ContextWindowExceeded,
            StopReason::PauseTurn,
            StopReason::Refusal,
            StopReason::Safety,
            StopReason::MalformedToolCall,
            StopReason::Incomplete,
            StopReason::Unreported,
            StopReason::Unrecognized,
        ] {
            let v = serde_json::to_value(&r).unwrap();
            assert_eq!(v, serde_json::json!(r.as_str()));
            assert_eq!(serde_json::from_value::<StopReason>(v).unwrap(), r);
        }
        let r: StopReason = serde_json::from_value(serde_json::json!("brand_new_reason")).unwrap();
        assert_eq!(r, StopReason::Unrecognized);
    }

    #[test]
    fn anthropic_table_covers_every_documented_value() {
        use map::anthropic as a;
        assert_eq!(a("end_turn"), StopReason::EndTurn);
        assert_eq!(a("tool_use"), StopReason::ToolUse);
        assert_eq!(a("stop_sequence"), StopReason::StopSequence);
        assert_eq!(a("max_tokens"), StopReason::MaxTokens);
        assert_eq!(
            a("model_context_window_exceeded"),
            StopReason::ContextWindowExceeded
        );
        assert_eq!(a("pause_turn"), StopReason::PauseTurn);
        assert_eq!(a("refusal"), StopReason::Refusal);
        assert_eq!(a("brand_new_reason"), StopReason::Unrecognized);
    }

    #[test]
    fn chat_finish_table() {
        use map::chat_finish as c;
        assert_eq!(c("stop"), StopReason::EndTurn);
        assert_eq!(c("length"), StopReason::MaxTokens);
        assert_eq!(c("tool_calls"), StopReason::ToolUse);
        assert_eq!(c("function_call"), StopReason::ToolUse);
        assert_eq!(c("content_filter"), StopReason::Safety);
        assert_eq!(c("abort"), StopReason::Incomplete);
        assert_eq!(c("eos_but_new"), StopReason::Unrecognized);
    }

    #[test]
    fn responses_incomplete_table() {
        use map::responses_incomplete as r;
        assert_eq!(r(Some("max_output_tokens")), StopReason::MaxTokens);
        assert_eq!(r(Some("content_filter")), StopReason::Safety);
        assert_eq!(r(Some("max_messages")), StopReason::Incomplete);
        assert_eq!(r(Some("steered")), StopReason::Incomplete);
        assert_eq!(r(Some("something_new")), StopReason::Incomplete);
        assert_eq!(r(None), StopReason::Incomplete);
    }

    #[test]
    fn gemini_finish_table() {
        use map::gemini_finish as g;
        assert_eq!(g("STOP"), StopReason::EndTurn);
        assert_eq!(g("MAX_TOKENS"), StopReason::MaxTokens);
        for s in [
            "SAFETY",
            "RECITATION",
            "LANGUAGE",
            "BLOCKLIST",
            "PROHIBITED_CONTENT",
            "SPII",
            "IMAGE_SAFETY",
            "IMAGE_PROHIBITED_CONTENT",
            "IMAGE_RECITATION",
            "IMAGE_OTHER",
            "NO_IMAGE",
        ] {
            assert_eq!(g(s), StopReason::Safety, "{s}");
        }
        for s in [
            "MALFORMED_FUNCTION_CALL",
            "UNEXPECTED_TOOL_CALL",
            "TOO_MANY_TOOL_CALLS",
        ] {
            assert_eq!(g(s), StopReason::MalformedToolCall, "{s}");
        }
        assert_eq!(g("OTHER"), StopReason::Incomplete);
        assert_eq!(g("FINISH_REASON_UNSPECIFIED"), StopReason::Unreported);
        // Not Gemini values (the old table invented them): unrecognized now.
        assert_eq!(g("STOP_SEQUENCE"), StopReason::Unrecognized);
        assert_eq!(g("FUNCTION_CALLING"), StopReason::Unrecognized);
    }

    #[test]
    fn synthetic_and_from_wire() {
        let s = Stop::synthetic(StopReason::EndTurn, "mock");
        assert_eq!(s.wire.value.as_deref(), Some("end_turn"));
        assert_eq!(s.wire.provider, "mock");
        let w = Stop::from_wire(StopReason::Unreported, "openai-compatible", None);
        assert_eq!(w.wire.value, None);
        assert_eq!(w.wire_value_or_reason(), "unreported");
        let mut d = Stop::from_wire(StopReason::Safety, "google-gemini-cli", Some("SAFETY"));
        d.set_detail("finishMessage", serde_json::json!("blocked"));
        d.set_detail("prompt_blocked", serde_json::json!(false));
        assert_eq!(d.wire.details.as_ref().unwrap()["finishMessage"], "blocked");
        assert_eq!(d.wire_value_or_reason(), "SAFETY");
    }

    #[test]
    fn stop_serializes_compactly() {
        let s = Stop::from_wire(StopReason::EndTurn, "anthropic", Some("end_turn"));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "reason": "end_turn",
                "wire": {"provider": "anthropic", "value": "end_turn"}
            })
        );
        let back: Stop = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }
}
