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
///
/// `success` and `skipped` are added so downstream `when:` gates can
/// branch on whether a prior step ran cleanly. The convention:
/// - `success = true, skipped = false` → step ran and finished without
///   error
/// - `success = false, skipped = false` → step errored (and was
///   tolerated via `continue_on_error`)
/// - `success = false, skipped = true` → step was skipped because its
///   own `when:` evaluated false
#[derive(Debug, Default, Serialize, Clone)]
pub struct StepOutput {
    pub output: String,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub skipped: bool,
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
                success: true,
                skipped: false,
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

/// Evaluate a `when:` expression against the step context and reduce
/// it to a boolean. Renders the expression with the same minijinja
/// environment as `render_step_prompt`, then trims and matches the
/// result against falsy literals (case-insensitive: `false`, `0`, ``,
/// `no`, `off`); anything else is truthy. This matches what most
/// workflow engines do — and lets agents emit `success: true` /
/// `success: false` JSON in their final assistant message and have
/// downstream steps gate on it via `{{steps.foo.output | trim}}`.
pub fn render_when_expression(template: &str, ctx: &StepContext) -> Result<bool, RenderError> {
    let rendered = render_step_prompt(template, ctx)?;
    Ok(is_truthy(&rendered))
}

fn is_truthy(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    !matches!(
        t.to_ascii_lowercase().as_str(),
        "false" | "0" | "no" | "off"
    )
}

#[cfg(test)]
mod when_tests {
    use super::*;

    #[test]
    fn falsy_values_skip_step() {
        for s in ["false", "FALSE", "0", "", "no", "OFF", "  false  "] {
            assert!(!is_truthy(s), "{s:?} should be falsy");
        }
    }

    #[test]
    fn truthy_values_run_step() {
        for s in ["true", "1", "yes", "on", "anything-else", "found-issues"] {
            assert!(is_truthy(s), "{s:?} should be truthy");
        }
    }

    #[test]
    fn render_when_expression_evaluates_step_output() {
        let mut ctx = StepContext::new();
        ctx.steps.insert(
            "review".into(),
            StepOutput {
                output: "false".into(),
                success: true,
                skipped: false,
            },
        );
        let v = render_when_expression("{{ steps.review.output }}", &ctx).expect("render");
        assert!(!v);
        let v = render_when_expression("{{ steps.review.success }}", &ctx).expect("render");
        assert!(v);
    }
}
