//! Structured output and adapter port for bounded, read-only CLI commands.
use serde::{Deserialize, Serialize};
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CommandOutput {
    pub output: String,
    pub stderr: String,
    pub exit_code: i32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
impl CommandOutput {
    pub fn text(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            ..Default::default()
        }
    }
    pub fn bound(&mut self) {
        for stream in [&mut self.output, &mut self.stderr] {
            if stream.len() > MAX_OUTPUT_BYTES {
                let mut end = MAX_OUTPUT_BYTES;
                while !stream.is_char_boundary(end) {
                    end -= 1;
                }
                stream.truncate(end);
                self.truncated = true;
            }
        }
        if self
            .data
            .as_ref()
            .is_some_and(|v| v.to_string().len() > MAX_OUTPUT_BYTES)
        {
            self.data = None;
            self.truncated = true;
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
}
