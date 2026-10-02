//! Classify every LLM reply and provider error into an [`Outcome`] (spec
//! 2026-10-01 response-outcomes §5). Pure: no I/O and no recovery decisions —
//! the runner decides what to do about an outcome.
//!
//! `classify_*` return `id: String::new()`; the runner sets the id from its
//! run-local counter (`oc_1`, `oc_2`, …) before writing the `Outcome` event.

use rupu_providers::reply_error::{ApiErrorBody, ErrorClass};
use rupu_providers::{ContentBlock, LlmResponse, ProviderError, Stop, StopReason};
use rupu_transcript::{OutcomeRecord, Severity};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeClass {
    PauseTurn,
    MaxTokens,
    ContextWindowExceeded,
    Refusal,
    Safety,
    MalformedToolCall,
    Incomplete,
    EmptyReply,
    UnrecognizedStop,
    UnreportedStop,
    ProviderError(ErrorClass),
}

impl OutcomeClass {
    /// The transcript `class` string.
    pub fn as_str(&self) -> &'static str {
        match self {
            OutcomeClass::PauseTurn => "pause_turn",
            OutcomeClass::MaxTokens => "max_tokens",
            OutcomeClass::ContextWindowExceeded => "context_window_exceeded",
            OutcomeClass::Refusal => "refusal",
            OutcomeClass::Safety => "safety",
            OutcomeClass::MalformedToolCall => "malformed_tool_call",
            OutcomeClass::Incomplete => "incomplete",
            OutcomeClass::EmptyReply => "empty_reply",
            OutcomeClass::UnrecognizedStop => "unrecognized_stop",
            OutcomeClass::UnreportedStop => "unreported_stop",
            OutcomeClass::ProviderError(_) => "provider_error",
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            OutcomeClass::PauseTurn => Severity::Info,
            OutcomeClass::UnrecognizedStop | OutcomeClass::UnreportedStop => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    /// Empty until the runner assigns the run-local id.
    pub id: String,
    pub class: OutcomeClass,
    pub title: String,
    pub detail: Option<String>,
    /// The provider's own words: a serialized `WireStop` or `ApiErrorBody`.
    pub wire: Value,
    /// A `MaxTokens` reply that cut a tool call off mid-arguments.
    pub truncated_tool: bool,
}

impl Outcome {
    pub fn severity(&self) -> Severity {
        self.class.severity()
    }

    pub fn record(&self) -> OutcomeRecord {
        OutcomeRecord {
            id: self.id.clone(),
            class: self.class.as_str().to_string(),
            severity: self.severity(),
            title: self.title.clone(),
            detail: self.detail.clone(),
            error_class: match &self.class {
                OutcomeClass::ProviderError(c) => Some(error_class_name(*c)),
                _ => None,
            },
            wire: self.wire.clone(),
        }
    }
}

/// The snake_case serde name of an [`ErrorClass`].
fn error_class_name(c: ErrorClass) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unrecognized".to_string())
}

fn detail_str<'a>(stop: &'a Stop, path: &[&str]) -> Option<&'a str> {
    let mut cur = stop.wire.details.as_ref()?;
    for key in path {
        cur = cur.get(*key)?;
    }
    cur.as_str()
}

fn has_usable_content(resp: &LlmResponse) -> bool {
    resp.content.iter().any(|b| match b {
        ContentBlock::Text { text } => !text.trim().is_empty(),
        ContentBlock::ToolUse { .. } => true,
        _ => false,
    })
}

/// Classify a reply. `None` is a normal reply.
pub fn classify_response(resp: &LlmResponse) -> Option<Outcome> {
    let stop = &resp.stop;
    let wire = serde_json::to_value(&stop.wire).unwrap_or(Value::Null);
    let mut truncated_tool = false;
    let wire_value = stop.wire.value.as_deref();

    let (class, title, detail) = match stop.reason {
        StopReason::EndTurn | StopReason::ToolUse | StopReason::StopSequence => {
            if has_usable_content(resp) {
                return None;
            }
            (OutcomeClass::EmptyReply, "empty reply".to_string(), None)
        }
        StopReason::Unrecognized | StopReason::Unreported if !has_usable_content(resp) => {
            (OutcomeClass::EmptyReply, "empty reply".to_string(), None)
        }
        StopReason::Unrecognized => (
            OutcomeClass::UnrecognizedStop,
            format!(
                "unrecognized stop reason · {} \"{}\"",
                stop.wire.provider,
                wire_value.unwrap_or("unreported")
            ),
            None,
        ),
        StopReason::Unreported => (
            OutcomeClass::UnreportedStop,
            format!("no stop reason reported · {}", stop.wire.provider),
            None,
        ),
        StopReason::PauseTurn => (
            OutcomeClass::PauseTurn,
            "paused by the provider (server tool loop)".to_string(),
            None,
        ),
        StopReason::MaxTokens => {
            truncated_tool = stop
                .wire
                .details
                .as_ref()
                .is_some_and(|d| d.get("truncated_tool").is_some());
            let detail = if truncated_tool {
                let name = detail_str(stop, &["truncated_tool", "name"]).unwrap_or("unknown");
                Some(format!("truncated tool call {name}"))
            } else {
                None
            };
            (
                OutcomeClass::MaxTokens,
                "truncated · output limit".to_string(),
                detail,
            )
        }
        StopReason::ContextWindowExceeded => (
            OutcomeClass::ContextWindowExceeded,
            "truncated · context window full".to_string(),
            None,
        ),
        StopReason::Refusal => {
            let category = stop.refusal.as_ref().and_then(|r| r.category.as_deref());
            let title = match category {
                Some(c) => format!("refused · {c}"),
                None => "refused".to_string(),
            };
            let detail = stop.refusal.as_ref().and_then(|r| r.explanation.clone());
            (OutcomeClass::Refusal, title, detail)
        }
        StopReason::Safety => {
            let title = match wire_value {
                Some(v) => format!("blocked by a safety filter · {v}"),
                None => "blocked by a safety filter".to_string(),
            };
            let detail = detail_str(stop, &["finishMessage"]).map(str::to_string);
            (OutcomeClass::Safety, title, detail)
        }
        StopReason::MalformedToolCall => {
            let name = detail_str(stop, &["malformed_tool", "name"]).unwrap_or("unknown");
            let detail = detail_str(stop, &["malformed_tool", "error"]).map(str::to_string);
            (
                OutcomeClass::MalformedToolCall,
                format!("malformed tool call · {name}"),
                detail,
            )
        }
        StopReason::Incomplete => (
            OutcomeClass::Incomplete,
            format!("incomplete reply · {}", wire_value.unwrap_or("unreported")),
            None,
        ),
    };

    Some(Outcome {
        id: String::new(),
        class,
        title,
        detail,
        wire,
        truncated_tool,
    })
}

/// Classify a provider error. Every error is an outcome.
pub fn classify_error(e: &ProviderError) -> Outcome {
    let class = e.class();
    let wire = match e.reply() {
        Some(body) => serde_json::to_value::<&ApiErrorBody>(body).unwrap_or(Value::Null),
        None => serde_json::json!({ "message": e.to_string() }),
    };
    Outcome {
        id: String::new(),
        title: format!("provider error · {}", error_class_name(class)),
        class: OutcomeClass::ProviderError(class),
        detail: Some(e.to_string()),
        wire,
        truncated_tool: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_providers::stop::{RefusalDetail, RefusalSource};
    use rupu_providers::Usage;
    use serde_json::json;

    fn text(s: &str) -> ContentBlock {
        ContentBlock::Text {
            text: s.to_string(),
        }
    }

    fn tool_use() -> ContentBlock {
        ContentBlock::ToolUse {
            id: "tu_1".into(),
            name: "bash".into(),
            input: json!({"command": "ls"}),
        }
    }

    fn resp(stop: Stop, content: Vec<ContentBlock>) -> LlmResponse {
        LlmResponse {
            id: "r1".into(),
            model: "m".into(),
            content,
            stop,
            usage: Usage::default(),
        }
    }

    fn synth(reason: StopReason) -> Stop {
        Stop::synthetic(reason, "anthropic")
    }

    #[test]
    fn normal_stops_with_content_are_not_outcomes() {
        for reason in [
            StopReason::EndTurn,
            StopReason::ToolUse,
            StopReason::StopSequence,
        ] {
            assert!(classify_response(&resp(synth(reason), vec![text("hello")])).is_none());
        }
    }

    #[test]
    fn tool_use_with_tool_blocks_and_no_text_is_normal() {
        let r = resp(synth(StopReason::ToolUse), vec![tool_use()]);
        assert!(classify_response(&r).is_none());
    }

    #[test]
    fn empty_content_end_turn_is_empty_reply() {
        let o = classify_response(&resp(synth(StopReason::EndTurn), vec![])).unwrap();
        assert_eq!(o.class, OutcomeClass::EmptyReply);
        assert_eq!(o.title, "empty reply");
        assert_eq!(o.severity(), Severity::Error);
        assert!(o.id.is_empty());
    }

    #[test]
    fn whitespace_only_text_is_empty_reply() {
        let o = classify_response(&resp(synth(StopReason::EndTurn), vec![text("  \n\t")])).unwrap();
        assert_eq!(o.class, OutcomeClass::EmptyReply);
    }

    #[test]
    fn stop_sequence_and_tool_use_without_content_are_empty_replies() {
        for reason in [StopReason::StopSequence, StopReason::ToolUse] {
            let o = classify_response(&resp(synth(reason), vec![])).unwrap();
            assert_eq!(o.class, OutcomeClass::EmptyReply);
        }
    }

    #[test]
    fn unrecognized_and_unreported_without_content_are_empty_replies() {
        for reason in [StopReason::Unrecognized, StopReason::Unreported] {
            let o = classify_response(&resp(synth(reason), vec![])).unwrap();
            assert_eq!(o.class, OutcomeClass::EmptyReply);
        }
    }

    #[test]
    fn pause_turn_is_info() {
        let o = classify_response(&resp(synth(StopReason::PauseTurn), vec![text("x")])).unwrap();
        assert_eq!(o.class, OutcomeClass::PauseTurn);
        assert_eq!(o.severity(), Severity::Info);
        assert_eq!(o.title, "paused by the provider (server tool loop)");
        assert_eq!(o.class.as_str(), "pause_turn");
    }

    #[test]
    fn max_tokens_plain() {
        let o = classify_response(&resp(synth(StopReason::MaxTokens), vec![text("x")])).unwrap();
        assert_eq!(o.class, OutcomeClass::MaxTokens);
        assert_eq!(o.severity(), Severity::Error);
        assert_eq!(o.title, "truncated · output limit");
        assert!(!o.truncated_tool);
        assert!(o.detail.is_none());
    }

    #[test]
    fn max_tokens_with_truncated_tool() {
        let mut stop = synth(StopReason::MaxTokens);
        stop.set_detail(
            "truncated_tool",
            json!({"name": "write_file", "id": "tu_9", "error": "EOF"}),
        );
        let o = classify_response(&resp(stop, vec![text("x")])).unwrap();
        assert!(o.truncated_tool);
        assert_eq!(o.detail.as_deref(), Some("truncated tool call write_file"));
    }

    #[test]
    fn context_window_exceeded() {
        let o = classify_response(&resp(synth(StopReason::ContextWindowExceeded), vec![])).unwrap();
        assert_eq!(o.class, OutcomeClass::ContextWindowExceeded);
        assert_eq!(o.severity(), Severity::Error);
        assert_eq!(o.title, "truncated · context window full");
        assert_eq!(o.class.as_str(), "context_window_exceeded");
    }

    #[test]
    fn refusal_with_category_and_explanation() {
        let mut stop = synth(StopReason::Refusal);
        stop.refusal = Some(RefusalDetail {
            category: Some("cyber".into()),
            explanation: Some("declined by a classifier".into()),
            recommended_model: None,
            source: RefusalSource::Classifier,
        });
        let o = classify_response(&resp(stop, vec![])).unwrap();
        assert_eq!(o.class, OutcomeClass::Refusal);
        assert_eq!(o.severity(), Severity::Error);
        assert_eq!(o.title, "refused · cyber");
        assert_eq!(o.detail.as_deref(), Some("declined by a classifier"));
    }

    #[test]
    fn refusal_without_category() {
        let o = classify_response(&resp(synth(StopReason::Refusal), vec![])).unwrap();
        assert_eq!(o.title, "refused");
        assert!(o.detail.is_none());
    }

    #[test]
    fn safety_with_finish_message() {
        let mut stop = Stop::from_wire(StopReason::Safety, "google-gemini", Some("SAFETY"));
        stop.set_detail("finishMessage", json!("blocked for reasons"));
        let o = classify_response(&resp(stop, vec![])).unwrap();
        assert_eq!(o.class, OutcomeClass::Safety);
        assert_eq!(o.severity(), Severity::Error);
        assert_eq!(o.title, "blocked by a safety filter · SAFETY");
        assert_eq!(o.detail.as_deref(), Some("blocked for reasons"));
    }

    #[test]
    fn malformed_tool_call() {
        let mut stop = synth(StopReason::MalformedToolCall);
        stop.set_detail(
            "malformed_tool",
            json!({"name": "read_file", "id": "tu_2", "error": "bad json"}),
        );
        let o = classify_response(&resp(stop, vec![])).unwrap();
        assert_eq!(o.class, OutcomeClass::MalformedToolCall);
        assert_eq!(o.severity(), Severity::Error);
        assert_eq!(o.title, "malformed tool call · read_file");
        assert_eq!(o.detail.as_deref(), Some("bad json"));
    }

    #[test]
    fn incomplete_with_and_without_wire_value() {
        let stop = Stop::from_wire(StopReason::Incomplete, "openai", Some("max_output_tokens"));
        let o = classify_response(&resp(stop, vec![text("x")])).unwrap();
        assert_eq!(o.class, OutcomeClass::Incomplete);
        assert_eq!(o.severity(), Severity::Error);
        assert_eq!(o.title, "incomplete reply · max_output_tokens");

        let stop = Stop::from_wire(StopReason::Incomplete, "openai", None);
        let o = classify_response(&resp(stop, vec![text("x")])).unwrap();
        assert_eq!(o.title, "incomplete reply · unreported");
    }

    #[test]
    fn unrecognized_stop_with_content_is_a_warning() {
        let stop = Stop::from_wire(StopReason::Unrecognized, "anthropic", Some("novel_reason"));
        let o = classify_response(&resp(stop, vec![text("x")])).unwrap();
        assert_eq!(o.class, OutcomeClass::UnrecognizedStop);
        assert_eq!(o.severity(), Severity::Warning);
        assert_eq!(
            o.title,
            "unrecognized stop reason · anthropic \"novel_reason\""
        );
        assert_eq!(o.class.as_str(), "unrecognized_stop");
    }

    #[test]
    fn unreported_stop_with_content_is_a_warning() {
        let stop = Stop::from_wire(StopReason::Unreported, "local", None);
        let o = classify_response(&resp(stop, vec![tool_use()])).unwrap();
        assert_eq!(o.class, OutcomeClass::UnreportedStop);
        assert_eq!(o.severity(), Severity::Warning);
        assert_eq!(o.title, "no stop reason reported · local");
        assert_eq!(o.class.as_str(), "unreported_stop");
    }

    #[test]
    fn response_wire_is_the_serialized_wire_stop() {
        let stop = Stop::from_wire(StopReason::Unrecognized, "anthropic", Some("novel"));
        let o = classify_response(&resp(stop.clone(), vec![text("x")])).unwrap();
        assert_eq!(o.wire, serde_json::to_value(&stop.wire).unwrap());
        assert_eq!(o.wire["value"], "novel");
    }

    #[test]
    fn class_strings_and_severities() {
        let cases = [
            (OutcomeClass::PauseTurn, "pause_turn", Severity::Info),
            (OutcomeClass::MaxTokens, "max_tokens", Severity::Error),
            (
                OutcomeClass::ContextWindowExceeded,
                "context_window_exceeded",
                Severity::Error,
            ),
            (OutcomeClass::Refusal, "refusal", Severity::Error),
            (OutcomeClass::Safety, "safety", Severity::Error),
            (
                OutcomeClass::MalformedToolCall,
                "malformed_tool_call",
                Severity::Error,
            ),
            (OutcomeClass::Incomplete, "incomplete", Severity::Error),
            (OutcomeClass::EmptyReply, "empty_reply", Severity::Error),
            (
                OutcomeClass::UnrecognizedStop,
                "unrecognized_stop",
                Severity::Warning,
            ),
            (
                OutcomeClass::UnreportedStop,
                "unreported_stop",
                Severity::Warning,
            ),
            (
                OutcomeClass::ProviderError(ErrorClass::Server),
                "provider_error",
                Severity::Error,
            ),
        ];
        for (class, name, sev) in cases {
            assert_eq!(class.as_str(), name);
            assert_eq!(class.severity(), sev);
        }
    }

    #[test]
    fn classify_error_overloaded_reply() {
        let e = ProviderError::api(
            "anthropic",
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        let o = classify_error(&e);
        assert_eq!(o.class, OutcomeClass::ProviderError(ErrorClass::Overloaded));
        assert_eq!(o.title, "provider error · overloaded");
        assert_eq!(o.detail.as_deref(), Some(e.to_string().as_str()));
        assert_eq!(o.severity(), Severity::Error);
        assert!(o.id.is_empty());
        assert_eq!(o.wire, serde_json::to_value(e.reply().unwrap()).unwrap());
        assert_eq!(o.record().error_class.as_deref(), Some("overloaded"));
    }

    #[test]
    fn classify_error_non_reply_wire_is_the_message() {
        let e = ProviderError::Transient(anyhow::anyhow!("connection reset"));
        let o = classify_error(&e);
        assert_eq!(o.class, OutcomeClass::ProviderError(ErrorClass::Server));
        assert_eq!(o.title, "provider error · server");
        assert_eq!(o.wire, json!({ "message": e.to_string() }));
    }

    #[test]
    fn record_maps_every_field() {
        let o = Outcome {
            id: "oc_3".into(),
            class: OutcomeClass::Refusal,
            title: "refused · cyber".into(),
            detail: Some("why".into()),
            wire: json!({"provider": "anthropic"}),
            truncated_tool: false,
        };
        let r = o.record();
        assert_eq!(r.id, "oc_3");
        assert_eq!(r.class, "refusal");
        assert_eq!(r.severity, Severity::Error);
        assert_eq!(r.title, "refused · cyber");
        assert_eq!(r.detail.as_deref(), Some("why"));
        assert_eq!(r.error_class, None);
        assert_eq!(r.wire, json!({"provider": "anthropic"}));
    }
}
