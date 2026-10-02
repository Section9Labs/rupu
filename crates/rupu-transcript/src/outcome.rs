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
