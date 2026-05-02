//! Step-prompt template rendering.
//!
//! Templates use minijinja syntax. Two top-level objects are
//! available:
//!
//! - `inputs.<key>` — values passed via CLI (e.g.,
//!   `rupu workflow run my-wf --input prompt="fix X"`).
//! - `steps.<step_id>.output` — the previous step's `stdout` (the
//!   agent's final assistant text).
//!
//! v0 uses minijinja's default undefined-handling: missing variables
//! render as empty strings. This is permissive but matches what
//! Okesu does and keeps templates pleasant during iteration.

use minijinja::{Environment, Value as MjValue};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("template: {0}")]
    Template(String),
}

/// Variable bag passed to the renderer.
#[derive(Debug, Default, Serialize, Clone)]
pub struct StepContext {
    pub inputs: BTreeMap<String, String>,
    pub steps: BTreeMap<String, StepOutput>,
}

/// The output record for a completed step, available as
/// `steps.<step_id>.output` in subsequent templates.
#[derive(Debug, Default, Serialize, Clone)]
pub struct StepOutput {
    pub output: String,
}

impl StepContext {
    /// Create an empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a workflow input value (builder style).
    pub fn with_input(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inputs.insert(key.into(), value.into());
        self
    }

    /// Record a prior step's output (builder style).
    pub fn with_step_output(
        mut self,
        step_id: impl Into<String>,
        output: impl Into<String>,
    ) -> Self {
        self.steps.insert(
            step_id.into(),
            StepOutput {
                output: output.into(),
            },
        );
        self
    }
}

/// Render `template` against `ctx`. Returns the rendered string or a
/// `RenderError` for invalid syntax. Missing variables become empty
/// strings (v0 default).
pub fn render_step_prompt(template: &str, ctx: &StepContext) -> Result<String, RenderError> {
    let mut env = Environment::new();
    env.add_template("step", template)
        .map_err(|e| RenderError::Template(e.to_string()))?;
    let tmpl = env
        .get_template("step")
        .map_err(|e| RenderError::Template(e.to_string()))?;
    let value = MjValue::from_serialize(ctx);
    tmpl.render(value)
        .map_err(|e| RenderError::Template(e.to_string()))
}
