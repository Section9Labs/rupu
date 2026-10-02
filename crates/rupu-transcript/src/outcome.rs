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

/// One-line rendering of an outcome: `✗ title — detail` (`✗` error, `!`
/// warning, `·` info). Plan 3 replaces this with the full presentation module.
pub fn outcome_line(o: &OutcomeRecord) -> String {
    let glyph = match o.severity {
        Severity::Error => '✗',
        Severity::Warning => '!',
        Severity::Info => '·',
    };
    match o.detail.as_deref().filter(|d| !d.is_empty()) {
        Some(detail) => format!("{glyph} {} — {detail}", o.title),
        None => format!("{glyph} {}", o.title),
    }
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
    format!("↺ rung {rung} · {phrase}")
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
    let json = data.to_string();
    if json.chars().count() <= UNKNOWN_DATA_MAX_CHARS {
        format!("{head} {json}")
    } else {
        let cut: String = json.chars().take(UNKNOWN_DATA_MAX_CHARS - 1).collect();
        format!("{head} {cut}…")
    }
}
