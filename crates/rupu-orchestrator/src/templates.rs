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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Permissive,
    Strict,
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
///
/// `issue` is populated when the run was kicked off with an issue
/// run-target (e.g. `rupu workflow run triage --target
/// github:owner/repo/issues/42`). It carries the fetched issue
/// payload — `{{issue.title}}`, `{{issue.body}}`, `{{issue.labels}}`,
/// `{{issue.number}}`, `{{issue.author}}`, `{{issue.state}}`. For
/// runs without an issue target it's `None` and chained access
/// renders empty under the chainable undefined behavior.
#[derive(Debug, Default, Serialize, Clone)]
pub struct StepContext {
    pub inputs: BTreeMap<String, String>,
    pub steps: BTreeMap<String, StepOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<serde_json::Value>,
    /// Pre-fetched issue payload bound at run-start when the
    /// run-target is an issue. See struct-level docs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<serde_json::Value>,
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
/// For fan-out steps (`for_each:` / `parallel:`):
/// - `output` is the JSON array of per-unit outputs (so legacy
///   templates that read `steps.foo.output` still see something
///   structured),
/// - `results` is the per-unit list of strings bound as
///   `steps.<id>.results[*]` (in declared order — items for
///   `for_each:`, sub-steps for `parallel:`),
/// - `sub_results` is the per-sub-step name-keyed map bound as
///   `steps.<id>.sub_results.<sub_id>` (only populated for
///   `parallel:`),
/// - `success` is true iff every unit finished without error.
#[derive(Debug, Serialize, Clone)]
pub struct StepOutput {
    pub output: String,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub skipped: bool,
    /// Per-unit outputs (strings) for fan-out steps. Empty for
    /// non-fan-out steps. Bound as `steps.<id>.results[*]`.
    ///
    /// Always serialized — `skip_serializing_if` would make empty
    /// vecs *absent* in the template context, causing
    /// `{{ steps.x.results | length }}` to fail with "undefined" on
    /// any step that legitimately produced zero results. Workflow
    /// authors should never need defensive `default([])` plumbing
    /// for fields the engine guarantees exist.
    #[serde(default)]
    pub results: Vec<String>,
    /// Per-sub-step map for `parallel:` steps. Empty for `for_each:`
    /// and linear steps. Bound as
    /// `steps.<id>.sub_results.<sub_id>.{output,success}`.
    /// Always serialized — see `results` for rationale.
    #[serde(default)]
    pub sub_results: BTreeMap<String, SubResult>,
    /// Aggregated findings for `panel:` steps. Empty for non-panel
    /// steps. Bound as `steps.<id>.findings[*]` and
    /// `steps.<id>.max_severity` ("critical" / "high" / "medium" /
    /// "low" / "" when there are no findings).
    /// Always serialized — see `results` for rationale.
    #[serde(default)]
    pub findings: Vec<FindingView>,
    /// Highest severity in `findings`. Empty string when no
    /// findings. Convenient for `when:` gates: e.g.
    /// `when: "{{ steps.panel.max_severity == 'critical' }}"`.
    /// Always serialized — see `results` for rationale.
    #[serde(default)]
    pub max_severity: String,
    /// Iteration count for panel steps with a `gate:` loop. `0` for
    /// non-panel steps and panel steps without a gate (which run a
    /// single iteration).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub iterations: u32,
    /// `true` when a panel step's gate cleared (or no gate was set).
    /// `false` when `max_iterations` was hit with findings still
    /// above the threshold. Always `true` for non-panel steps.
    #[serde(skip_serializing_if = "is_true")]
    pub resolved: bool,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

fn is_true(b: &bool) -> bool {
    *b
}

/// Template-facing view of a single finding. Same shape as the
/// runtime [`crate::runner::Finding`] but lives in this module so
/// `StepOutput` can render cleanly without pulling the runner type
/// into the template surface.
#[derive(Debug, Clone, Serialize)]
pub struct FindingView {
    /// Panelist agent that emitted this finding.
    pub source: String,
    /// "low" / "medium" / "high" / "critical".
    pub severity: String,
    pub title: String,
    pub body: String,
}

impl Default for StepOutput {
    fn default() -> Self {
        Self {
            output: String::new(),
            success: false,
            skipped: false,
            results: Vec::new(),
            sub_results: BTreeMap::new(),
            findings: Vec::new(),
            max_severity: String::new(),
            iterations: 0,
            // Non-panel steps that complete normally are "resolved";
            // panel steps overwrite this when they decide.
            resolved: true,
        }
    }
}

/// One sub-step's published output for `parallel:` steps. Carries
/// enough surface for `when:` chains to branch on and for downstream
/// prompts to quote.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SubResult {
    pub output: String,
    pub success: bool,
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
                sub_results: BTreeMap::new(),
                ..Default::default()
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
                sub_results: BTreeMap::new(),
                ..Default::default()
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

    /// Bind the issue payload (builder style). For runs whose
    /// run-target resolved to an issue (`<platform>:<owner>/<repo>/issues/<N>`);
    /// the orchestrator pre-fetches via `IssueConnector::get_issue`
    /// and serializes the result into JSON.
    pub fn with_issue(mut self, issue: serde_json::Value) -> Self {
        self.issue = Some(issue);
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

/// Maximum diff size (bytes) embedded verbatim into the PR event
/// context before truncation. PR diffs can run into the megabytes for
/// large refactors; without a cap a single huge diff would balloon
/// every step prompt that references `event.pull_request.diff` and
/// risks blowing a provider's context window. 100 KiB comfortably
/// covers the vast majority of real-world PRs while keeping a hard
/// ceiling on the pathological case.
pub const MAX_PR_DIFF_BYTES: usize = 100 * 1024;

/// Build the `event` payload for a PR-triggered autoflow run.
///
/// Returns the JSON value to pass to [`StepContext::with_event`] — the
/// `event` key itself comes from that struct field (see the doc
/// comment on `StepContext::event`), so the value produced here is the
/// *content* of `event`, not a further-wrapped `{"event": ...}`
/// object. Callers do:
///
/// ```
/// # use rupu_orchestrator::templates::{StepContext, pr_event_context};
/// let ctx = StepContext::new().with_event(pr_event_context(
///     42, "Fix flaky test", "main", "feature/fix", "abc123",
///     "octocat", "https://github.com/owner/repo/pull/42",
///     "diff --git a/x b/x\n+hello\n", "owner/repo",
/// ));
/// ```
///
/// and templates then read `{{ event.pull_request.number }}`,
/// `{{ event.pull_request.diff }}`, `{{ event.repository.full_name }}`,
/// etc. — mirroring the existing webhook `event` shape used by
/// `event_tests` above.
///
/// Takes primitive fields rather than `rupu_scm::Pr` / `Diff` so this
/// template-rendering module doesn't need to reach into SCM connector
/// types for a handful of scalar values; callers that already hold a
/// `Pr` + `Diff` destructure them at the call site.
///
/// `diff` is bounded to [`MAX_PR_DIFF_BYTES`]: larger diffs are
/// truncated with a trailing note so a single oversized PR can't
/// balloon every step prompt that references `event.pull_request.diff`
/// or blow a provider's context window.
#[allow(clippy::too_many_arguments)]
pub fn pr_event_context(
    number: u64,
    title: &str,
    base: &str,
    head: &str,
    head_sha: &str,
    author: &str,
    url: &str,
    diff: &str,
    repo_full_name: &str,
) -> serde_json::Value {
    let diff = truncate_diff(diff);
    serde_json::json!({
        "pull_request": {
            "number": number,
            "title": title,
            "base": base,
            "head": head,
            "head_sha": head_sha,
            "author": author,
            "url": url,
            "diff": diff,
        },
        "repository": {
            "full_name": repo_full_name,
        }
    })
}

/// Truncate `diff` to at most [`MAX_PR_DIFF_BYTES`], appending a
/// trailing note when truncation happened. Cuts on a UTF-8 char
/// boundary at or before the byte cap so a multi-byte sequence is
/// never split.
fn truncate_diff(diff: &str) -> String {
    if diff.len() <= MAX_PR_DIFF_BYTES {
        return diff.to_string();
    }
    let mut end = MAX_PR_DIFF_BYTES;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = diff[..end].to_string();
    out.push_str("\n… (diff truncated)");
    out
}

/// Render `template` against `ctx`. Returns the rendered string or a
/// `RenderError` for invalid syntax. Missing variables become empty
/// strings (v0 default). We use [`UndefinedBehavior::Chainable`] so
/// chained accesses through a missing root (e.g. `{{ event.pull_request.number }}`
/// in a manually-triggered workflow where `event` is `None`) also
/// render empty rather than erroring — matching the permissive
/// philosophy stated in this module's docs.
pub fn render_step_prompt(
    template: &str,
    ctx: &StepContext,
    mode: RenderMode,
) -> Result<String, RenderError> {
    let mut env = Environment::new();
    env.set_undefined_behavior(match mode {
        RenderMode::Permissive => UndefinedBehavior::Chainable,
        RenderMode::Strict => UndefinedBehavior::Strict,
    });
    // `read_file('path')` — read a workspace file's contents into the template,
    // resolved relative to the run's working directory. Lets control flow be
    // driven by a file a prior step wrote (e.g.
    // `for_each: "{{ read_file('reports/units.json') }}"`) rather than by the
    // agent's chat output, which is far more deterministic. Errors loudly if the
    // file is missing so a fan-out never silently runs over nothing.
    env.add_function(
        "read_file",
        |path: String| -> Result<String, minijinja::Error> {
            std::fs::read_to_string(&path).map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("read_file({path:?}) failed: {e}"),
                )
            })
        },
    );
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
pub fn render_when_expression(
    template: &str,
    ctx: &StepContext,
    mode: RenderMode,
) -> Result<bool, RenderError> {
    let rendered = render_step_prompt(template, ctx, mode)?;
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
                sub_results: BTreeMap::new(),
                ..Default::default()
            },
        );
        let v = render_when_expression("{{ steps.review.output }}", &ctx, RenderMode::Permissive)
            .expect("render");
        assert!(!v);
        let v = render_when_expression("{{ steps.review.success }}", &ctx, RenderMode::Permissive)
            .expect("render");
        assert!(v);
    }

    #[test]
    fn empty_collection_fields_render_as_empty_not_undefined() {
        // Regression: `findings` / `results` / `sub_results` /
        // `max_severity` previously had `skip_serializing_if = "*::is_empty"`
        // which made empty values absent from the template context —
        // `{{ steps.x.findings | length }}` then errored with
        // "cannot calculate length of value of type undefined" on any
        // step that legitimately produced zero findings. The fields
        // are now always serialized so workflow authors don't need
        // defensive `default([])` plumbing.
        let mut ctx = StepContext::new();
        ctx.steps.insert(
            "panel".into(),
            StepOutput {
                output: "ok".into(),
                success: true,
                skipped: false,
                results: Vec::new(),
                sub_results: BTreeMap::new(),
                findings: Vec::new(),
                max_severity: String::new(),
                ..Default::default()
            },
        );

        // `findings | length` must return 0, not error on undefined.
        let prompt = render_step_prompt(
            "findings={{ steps.panel.findings | length }}",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("findings | length should not error on empty");
        assert_eq!(prompt, "findings=0");

        // `results | length` same property.
        let prompt = render_step_prompt(
            "results={{ steps.panel.results | length }}",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("results | length should not error on empty");
        assert_eq!(prompt, "results=0");

        // `sub_results | length` same property.
        let prompt = render_step_prompt(
            "subs={{ steps.panel.sub_results | length }}",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("sub_results | length should not error on empty");
        assert_eq!(prompt, "subs=0");

        // `max_severity` should render as empty string, not undefined.
        let prompt = render_step_prompt(
            "sev=<{{ steps.panel.max_severity }}>",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("render");
        assert_eq!(prompt, "sev=<>");

        // `for r in results` over empty must loop zero times silently.
        let prompt = render_step_prompt(
            "items=[{% for r in steps.panel.results %}{{ r }};{% endfor %}]",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("for-loop over empty should be a no-op");
        assert_eq!(prompt, "items=[]");
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
            RenderMode::Permissive,
        )
        .expect("render");
        assert_eq!(out, "PR #42 in Section9Labs/rupu: Fix flaky test");
    }

    #[test]
    fn missing_event_renders_empty_string() {
        let ctx = StepContext::new();
        let out = render_step_prompt(
            "repo={{ event.repository.name }}!",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("render");
        assert_eq!(out, "repo=!");
    }

    #[test]
    fn event_can_gate_when_expression() {
        let ctx = StepContext::new().with_event(json!({
            "pull_request": { "merged": true }
        }));
        let take = render_when_expression(
            "{{ event.pull_request.merged }}",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("render");
        assert!(take, "merged=true should be truthy");

        let ctx2 = StepContext::new().with_event(json!({
            "pull_request": { "merged": false }
        }));
        let take = render_when_expression(
            "{{ event.pull_request.merged }}",
            &ctx2,
            RenderMode::Permissive,
        )
        .expect("render");
        assert!(!take, "merged=false should be falsy");
    }
}

#[cfg(test)]
mod pr_event_tests {
    use super::*;

    #[test]
    fn pr_event_fields_render_in_prompt() {
        let ctx = StepContext::new().with_event(pr_event_context(
            42,
            "Fix flaky test",
            "main",
            "feature/fix",
            "abc123",
            "octocat",
            "https://github.com/Section9Labs/rupu/pull/42",
            "diff --git a/x b/x\n+hello\n",
            "Section9Labs/rupu",
        ));
        let out = render_step_prompt(
            "{{ event.pull_request.number }} {{ event.pull_request.head_sha }} {{ event.pull_request.author }} {{ event.pull_request.base }}",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("render");
        assert_eq!(out, "42 abc123 octocat main");
    }

    #[test]
    fn pr_event_context_carries_full_shape() {
        let ctx = StepContext::new().with_event(pr_event_context(
            7,
            "Title",
            "main",
            "feat",
            "sha1",
            "author",
            "https://example.com/pr/7",
            "diff content",
            "owner/repo",
        ));
        let out = render_step_prompt(
            "{{ event.pull_request.title }}|{{ event.pull_request.head }}|{{ event.pull_request.url }}|{{ event.pull_request.diff }}|{{ event.repository.full_name }}",
            &ctx,
            RenderMode::Permissive,
        )
        .expect("render");
        assert_eq!(
            out,
            "Title|feat|https://example.com/pr/7|diff content|owner/repo"
        );
    }

    #[test]
    fn oversized_diff_is_truncated() {
        let huge = "x".repeat(MAX_PR_DIFF_BYTES + 500);
        let value = pr_event_context(1, "t", "b", "h", "sha", "a", "u", &huge, "r");
        let diff = value["pull_request"]["diff"].as_str().expect("diff string");
        assert!(
            diff.contains("(diff truncated)"),
            "should contain truncation note: {diff}"
        );
        assert!(diff.len() < huge.len(), "should be shorter than original");
        assert!(
            diff.len() <= MAX_PR_DIFF_BYTES + 64,
            "should be bounded near threshold + note length, got {}",
            diff.len()
        );
    }

    #[test]
    fn undersized_diff_is_not_truncated() {
        let small = "small diff".to_string();
        let value = pr_event_context(1, "t", "b", "h", "sha", "a", "u", &small, "r");
        let diff = value["pull_request"]["diff"].as_str().expect("diff string");
        assert_eq!(diff, "small diff");
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn strict_mode_errors_on_missing_variable() {
        let ctx = StepContext::new();
        let err = render_step_prompt("{{ issue.title }}", &ctx, RenderMode::Strict).unwrap_err();
        assert!(err.to_string().contains("template:"));
    }
}

#[cfg(test)]
mod read_file_tests {
    use super::*;

    #[test]
    fn read_file_returns_contents() {
        let path = std::env::temp_dir().join(format!("rupu_read_file_{}.json", std::process::id()));
        std::fs::write(&path, "[\"services/a\",\"services/b\"]").expect("write fixture");
        let ctx = StepContext::new();
        let rendered = render_step_prompt(
            &format!("{{{{ read_file({:?}) }}}}", path.to_string_lossy()),
            &ctx,
            RenderMode::Permissive,
        )
        .expect("render");
        assert_eq!(rendered, "[\"services/a\",\"services/b\"]");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_file_errors_loudly_on_missing_file() {
        let ctx = StepContext::new();
        let err = render_step_prompt(
            "{{ read_file('/no/such/rupu/units.json') }}",
            &ctx,
            RenderMode::Permissive,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("read_file"),
            "error should name read_file: {err}"
        );
    }
}

#[cfg(test)]
mod tojson_tests {
    use super::*;
    use serde_json::json;

    // Regression: the `tojson` filter is gated behind minijinja's `json`
    // feature. Without it, a workflow step using `{{ x | tojson }}` fails at
    // render with "unknown filter: tojson" — which only surfaced once a step
    // past an approval gate was reached (e.g. on resume). The dep now enables
    // the `json` feature so the built-in filter is registered.
    #[test]
    fn tojson_filter_serializes_a_value() {
        let ctx = StepContext::new().with_event(json!({ "name": "rupu" }));
        let out = render_step_prompt("{{ event | tojson }}", &ctx, RenderMode::Permissive)
            .expect("tojson should be a registered filter");
        assert_eq!(out, "{\"name\":\"rupu\"}");
    }
}
