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

use minijinja::{Environment, UndefinedBehavior, Value as MjValue};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("template: {0}")]
    Template(String),
}

/// Variable bag passed to the renderer.
///
/// `event` is populated when the workflow was kicked off by the
/// webhook receiver (`trigger.on: event`). It carries the verbatim
/// JSON payload the SCM vendor sent, so step prompts and `when:`
/// expressions can reference `{{event.pull_request.number}}`,
/// `{{event.repository.name}}`, etc. For manually-invoked or cron-
/// triggered runs, `event` is `None` and references render as the
/// minijinja default for missing values (empty string).
///
/// `item` and `loop_info` are populated only inside a fan-out
/// (`for_each:`) iteration — the per-item prompt template can read
/// `{{item}}` and `{{loop.index}}` (1-based). They're absent for
/// linear steps; chained access on the missing root is safe under
/// the chainable undefined behavior.
#[derive(Debug, Default, Serialize, Clone)]
pub struct StepContext {
    pub inputs: BTreeMap<String, String>,
    pub steps: BTreeMap<String, StepOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<serde_json::Value>,
    /// Renamed to `loop` in the serialized form so templates can
    /// reference `{{ loop.index }}` (Jinja convention). The Rust
    /// field name avoids the keyword.
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_info: Option<LoopInfo>,
}

/// Per-iteration metadata exposed to fan-out item prompts.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LoopInfo {
    /// 1-based index of the current item.
    pub index: usize,
    /// 0-based index — useful for templates that prefer it.
    pub index0: usize,
    /// Total number of items in the fan-out.
    pub length: usize,
    /// True on the first item.
    pub first: bool,
    /// True on the last item.
    pub last: bool,
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
///
/// For fan-out steps (`for_each:`):
/// - `output` is the JSON array of per-item outputs (so legacy
///   templates that read `steps.foo.output` still see something
///   structured),
/// - `results` is the per-item list bound as `steps.<id>.results[*]`,
/// - `success` is true iff every item finished without error.
#[derive(Debug, Default, Serialize, Clone)]
pub struct StepOutput {
    pub output: String,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub skipped: bool,
    /// Per-item outputs for fan-out steps. Empty for non-fan-out
    /// steps. Bound as `steps.<id>.results[*]` in subsequent step
    /// templates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<String>,
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
                results: Vec::new(),
            },
        );
        self
    }

    /// Record a prior fan-out step's per-item results (builder style).
    /// `output` is the aggregate JSON array; `results` is the list
    /// bound as `steps.<id>.results[*]`.
    pub fn with_step_results(
        mut self,
        step_id: impl Into<String>,
        output: impl Into<String>,
        results: Vec<String>,
    ) -> Self {
        self.steps.insert(
            step_id.into(),
            StepOutput {
                output: output.into(),
                success: true,
                skipped: false,
                results,
            },
        );
        self
    }

    /// Bind the event payload (builder style). For event-triggered
    /// workflows; the same JSON the webhook receiver passed through
    /// to the dispatcher.
    pub fn with_event(mut self, event: serde_json::Value) -> Self {
        self.event = Some(event);
        self
    }

    /// Bind a fan-out item + loop metadata into the context. The
    /// orchestrator clones the parent context per item and calls
    /// this so the item-prompt template can reference `{{item}}` /
    /// `{{loop.index}}`.
    pub fn with_item(mut self, item: serde_json::Value, loop_info: LoopInfo) -> Self {
        self.item = Some(item);
        self.loop_info = Some(loop_info);
        self
    }
}

/// Render `template` against `ctx`. Returns the rendered string or a
/// `RenderError` for invalid syntax. Missing variables become empty
/// strings (v0 default). We use [`UndefinedBehavior::Chainable`] so
/// chained accesses through a missing root (e.g. `{{ event.pull_request.number }}`
/// in a manually-triggered workflow where `event` is `None`) also
/// render empty rather than erroring — matching the permissive
/// philosophy stated in this module's docs.
pub fn render_step_prompt(template: &str, ctx: &StepContext) -> Result<String, RenderError> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
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
                results: Vec::new(),
            },
        );
        let v = render_when_expression("{{ steps.review.output }}", &ctx).expect("render");
        assert!(!v);
        let v = render_when_expression("{{ steps.review.success }}", &ctx).expect("render");
        assert!(v);
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_fields_render_in_prompt() {
        let ctx = StepContext::new().with_event(json!({
            "pull_request": { "number": 42, "title": "Fix flaky test" },
            "repository": { "name": "rupu", "full_name": "Section9Labs/rupu" }
        }));
        let out = render_step_prompt(
            "PR #{{ event.pull_request.number }} in {{ event.repository.full_name }}: {{ event.pull_request.title }}",
            &ctx,
        )
        .expect("render");
        assert_eq!(out, "PR #42 in Section9Labs/rupu: Fix flaky test");
    }

    #[test]
    fn missing_event_renders_empty_string() {
        let ctx = StepContext::new();
        let out =
            render_step_prompt("repo={{ event.repository.name }}!", &ctx).expect("render");
        assert_eq!(out, "repo=!");
    }

    #[test]
    fn event_can_gate_when_expression() {
        let ctx = StepContext::new().with_event(json!({
            "pull_request": { "merged": true }
        }));
        let take = render_when_expression("{{ event.pull_request.merged }}", &ctx).expect("render");
        assert!(take, "merged=true should be truthy");

        let ctx2 = StepContext::new().with_event(json!({
            "pull_request": { "merged": false }
        }));
        let take = render_when_expression("{{ event.pull_request.merged }}", &ctx2).expect("render");
        assert!(!take, "merged=false should be falsy");
    }
}
