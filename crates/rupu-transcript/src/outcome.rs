//! Typed outcome records shared by every transcript reader (spec
//! 2026-10-01 response-outcomes §7.1). `class` / `reason` stay strings:
//! this crate is a leaf, and a newer writer's class must still parse.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// One classified outcome (a non-normal reply or a provider error).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeRecord {
    /// Unique within the run; `Recovery.outcome_id` points here.
    pub id: String,
    /// `refusal`, `safety`, `max_tokens`, `context_window_exceeded`,
    /// `pause_turn`, `malformed_tool_call`, `incomplete`, `empty_reply`,
    /// `unrecognized_stop`, `unreported_stop`, `provider_error`.
    pub class: String,
    pub severity: Severity,
    /// One line, e.g. `refused · cyber`.
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// For `provider_error`: the normalized `ErrorClass` (snake_case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    /// The provider's own words: the serialized `WireStop` or `ApiErrorBody`.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub wire: Value,
}

/// A turn's stop as recorded on `TurnEnd`. Deserializes from a serialized
/// `rupu_providers::Stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopRecord {
    pub reason: String,
    #[serde(default)]
    pub wire: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_by: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Continued,
    Retried,
    Compacted,
    FellBack,
    ServedByFallback,
    Skipped,
    Asked,
    Parked,
    Failed,
    #[serde(other)]
    Other,
}

impl RecoveryAction {
    /// The snake_case wire name (`Other` for an action this reader does not know).
    pub fn name(self) -> &'static str {
        match self {
            RecoveryAction::Continued => "continued",
            RecoveryAction::Retried => "retried",
            RecoveryAction::Compacted => "compacted",
            RecoveryAction::FellBack => "fell_back",
            RecoveryAction::ServedByFallback => "served_by_fallback",
            RecoveryAction::Skipped => "skipped",
            RecoveryAction::Asked => "asked",
            RecoveryAction::Parked => "parked",
            RecoveryAction::Failed => "failed",
            RecoveryAction::Other => "other",
        }
    }
}

/// The glyph-free body of an outcome row, `title — detail`, for renderers
/// that print their own status glyph.
pub fn outcome_body(o: &OutcomeRecord) -> String {
    match o.detail.as_deref().filter(|d| !d.is_empty()) {
        Some(detail) => format!("{} — {detail}", o.title),
        None => o.title.clone(),
    }
}

/// One-line rendering of an outcome: `✗ title — detail` (`✗` error, `!`
/// warning, `·` info). Plan 3 replaces this with the full presentation module.
pub fn outcome_line(o: &OutcomeRecord) -> String {
    let glyph = match o.severity {
        Severity::Error => '✗',
        Severity::Warning => '!',
        Severity::Info => '·',
    };
    format!("{glyph} {}", outcome_body(o))
}

/// The glyph-free body of a recovery row, `rung 1 · fell back to p/m`, for
/// renderers that print their own status glyph.
pub fn recovery_body(
    action: RecoveryAction,
    rung: u8,
    provider: Option<&str>,
    model: Option<&str>,
    attempt: Option<u32>,
    budget: Option<u32>,
    reason: Option<&str>,
) -> String {
    let target = || match (provider, model) {
        (Some(p), Some(m)) => format!("{p}/{m}"),
        (None, Some(m)) => m.to_string(),
        (Some(p), None) => p.to_string(),
        (None, None) => "?".to_string(),
    };
    let phrase = match action {
        RecoveryAction::Continued => match (attempt, budget) {
            (Some(a), Some(b)) => format!("continued {a}/{b}"),
            _ => "continued".to_string(),
        },
        RecoveryAction::FellBack => format!("fell back to {}", target()),
        RecoveryAction::ServedByFallback => {
            format!(
                "served by {}",
                model.map(str::to_string).unwrap_or_else(target)
            )
        }
        RecoveryAction::Skipped => match reason {
            Some(r) => format!("skipped {}: {r}", target()),
            None => format!("skipped {}", target()),
        },
        RecoveryAction::Failed => "no recovery left".to_string(),
        other => other.name().to_string(),
    };
    format!("rung {rung} · {phrase}")
}

/// One-line rendering of a recovery action: `↺ rung 1 · fell back to p/m`.
pub fn recovery_line(
    action: RecoveryAction,
    rung: u8,
    provider: Option<&str>,
    model: Option<&str>,
    attempt: Option<u32>,
    budget: Option<u32>,
    reason: Option<&str>,
) -> String {
    format!(
        "↺ {}",
        recovery_body(action, rung, provider, model, attempt, budget, reason)
    )
}

/// Cap on the JSON payload shown for an unrecognized event.
const UNKNOWN_DATA_MAX_CHARS: usize = 200;

/// One-line rendering of an event this reader does not know: its tag plus
/// the compact JSON of its data, truncated to 200 characters.
pub fn unknown_line(tag: &str, data: &Value) -> String {
    let head = format!("unrecognized event · {tag}");
    if data.is_null() {
        return head;
    }
    format!("{head} {}", cut_json(data))
}

/// One-line, glyph-free rendering of an `AssistantBlock` event's block (the
/// provider-neutral `ContentBlock` JSON):
///
/// - `fallback` → `served by fallback · <from> → <to>`
/// - `unknown` → `unrecognized block · <type> <raw JSON>`, where `<type>` is
///   the provider's own block type and the JSON is cut at 200 characters
///   like [`unknown_line`]
/// - anything else (an abandoned `tool_use` / `reasoning`) → its kind, and
///   the tool name for a tool call
///
/// An abandoned block is prefixed `abandoned · `. Plan 3 restyles these
/// rows; the text is shared by the CLI and (as a port) the web.
pub fn assistant_block_line(block: &Value, abandoned: bool) -> String {
    let str_at = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let kind = str_at(block, "type").unwrap_or_else(|| "?".to_string());
    let body = match kind.as_str() {
        "fallback" => format!(
            "served by fallback · {} → {}",
            str_at(block, "from_model").unwrap_or_else(|| "?".to_string()),
            str_at(block, "to_model").unwrap_or_else(|| "?".to_string()),
        ),
        "unknown" => {
            let raw = block.get("raw").unwrap_or(&Value::Null);
            let inner = str_at(raw, "type").unwrap_or_else(|| "?".to_string());
            let head = format!("unrecognized block · {inner}");
            if raw.is_null() {
                head
            } else {
                format!("{head} {}", cut_json(raw))
            }
        }
        "tool_use" => match str_at(block, "name") {
            Some(name) => format!("tool call · {name}"),
            None => "tool call".to_string(),
        },
        other => other.to_string(),
    };
    if abandoned {
        format!("abandoned · {body}")
    } else {
        body
    }
}

/// Compact JSON cut at [`UNKNOWN_DATA_MAX_CHARS`] characters.
fn cut_json(v: &Value) -> String {
    let json = v.to_string();
    if json.chars().count() <= UNKNOWN_DATA_MAX_CHARS {
        json
    } else {
        let cut: String = json.chars().take(UNKNOWN_DATA_MAX_CHARS - 1).collect();
        format!("{cut}…")
    }
}
