//! Shared helpers for building a tool's [`ToolOutput`] and reading its input.
//! One copy, used by every tool home.

use crate::tool::{ToolError, ToolOutput};
use serde_json::Value;
use std::time::Instant;

/// A successful result whose text the model reads.
pub fn ok(stdout: impl Into<String>) -> ToolOutput {
    ToolOutput {
        stdout: stdout.into(),
        error: None,
        duration_ms: 0,
        derived: None,
        structured: None,
    }
}

/// A successful result: `v` serialized compactly.
pub fn ok_json(v: Value) -> ToolOutput {
    ok(v.to_string())
}

/// A failure the model should see and can act on (not a run-aborting `Err`).
pub fn failed(msg: impl Into<String>) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(msg.into()),
        duration_ms: 0,
        derived: None,
        structured: None,
    }
}

impl ToolOutput {
    /// Stamp the time since `started` as this output's duration.
    pub fn timed(mut self, started: Instant) -> Self {
        self.duration_ms = started.elapsed().as_millis() as u64;
        self
    }
}

/// A required, non-blank string argument.
pub fn req_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    match input.get(key).and_then(Value::as_str).map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(ToolError::InvalidInput(format!(
            "{key} (non-empty string) required"
        ))),
    }
}

/// An optional string argument: absent, `null` and blank are all `None`; any
/// other non-string value is an error rather than silently ignored.
pub fn opt_str(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let s = s.trim();
            Ok((!s.is_empty()).then(|| s.to_string()))
        }
        Some(_) => Err(ToolError::InvalidInput(format!(
            "{key} must be a string when given"
        ))),
    }
}
