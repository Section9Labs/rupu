//! Event → workflow dispatch logic.
//!
//! Given a resolved rupu event id and the raw event payload, walk
//! the candidate workflows, evaluate each one's optional `filter:`
//! expression against `{ "event": <payload> }`, and dispatch every
//! one that matches via the caller-supplied [`WorkflowDispatcher`].
//!
//! `WorkflowDispatcher` is a trait so the receiver can be tested
//! without spinning up the full agent runtime — tests inject a
//! stub that records dispatch calls, production wires it to
//! `rupu_cli::cmd::workflow::run_by_name`.

use async_trait::async_trait;
use rupu_orchestrator::{TriggerKind, Workflow};
use serde_json::Value;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("filter render failed: {0}")]
    FilterRender(String),
}

/// Trait the receiver uses to actually run a workflow once the
/// event matched. Production impl wraps `rupu_cli::cmd::workflow::run_by_name`;
/// tests inject a stub.
#[async_trait]
pub trait WorkflowDispatcher: Send + Sync {
    async fn dispatch(&self, workflow_name: &str) -> anyhow::Result<()>;
}

/// Result row from [`dispatch_event`]: which workflows matched and
/// whether their dispatch calls succeeded. Used by the HTTP handler
/// to shape the response.
#[derive(Debug, Clone)]
pub struct DispatchedWorkflow {
    pub name: String,
    pub fired: bool,
    pub error: Option<String>,
}

/// Walk `candidates`, pick those whose `trigger.event:` equals
/// `event_id` AND whose `filter:` (if any) renders truthy against
/// the event payload, and dispatch them via `dispatcher`.
///
/// Returns one `DispatchedWorkflow` row per *matching* workflow
/// (not per candidate). Workflows whose filter rendered falsy are
/// silently skipped.
pub async fn dispatch_event(
    event_id: &str,
    payload: &Value,
    candidates: &[(String, Workflow)],
    dispatcher: &dyn WorkflowDispatcher,
) -> Vec<DispatchedWorkflow> {
    let mut out = Vec::new();
    for (name, wf) in candidates {
        if wf.trigger.on != TriggerKind::Event {
            continue;
        }
        if wf.trigger.event.as_deref() != Some(event_id) {
            continue;
        }
        if let Some(filter_expr) = &wf.trigger.filter {
            match render_filter(filter_expr, payload) {
                Ok(true) => {}
                Ok(false) => {
                    info!(workflow = %name, event = %event_id, "filter rejected; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(workflow = %name, error = %e, "filter render failed; skipping");
                    out.push(DispatchedWorkflow {
                        name: name.clone(),
                        fired: false,
                        error: Some(format!("filter render: {e}")),
                    });
                    continue;
                }
            }
        }
        info!(workflow = %name, event = %event_id, "dispatching");
        match dispatcher.dispatch(name).await {
            Ok(()) => out.push(DispatchedWorkflow {
                name: name.clone(),
                fired: true,
                error: None,
            }),
            Err(e) => {
                warn!(workflow = %name, error = %e, "dispatch failed");
                out.push(DispatchedWorkflow {
                    name: name.clone(),
                    fired: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }
    out
}

/// Render a `filter:` expression against `{ "event": <payload> }` and
/// reduce to bool using the same falsy literals as the orchestrator's
/// `when:` expression: `false` / `0` / `` / `no` / `off`. Anything else
/// is truthy.
fn render_filter(expr: &str, payload: &Value) -> Result<bool, DispatchError> {
    let mut env = minijinja::Environment::new();
    env.add_template("filter", expr)
        .map_err(|e| DispatchError::FilterRender(e.to_string()))?;
    let tmpl = env
        .get_template("filter")
        .map_err(|e| DispatchError::FilterRender(e.to_string()))?;
    let ctx = minijinja::Value::from_serialize(serde_json::json!({ "event": payload }));
    let rendered = tmpl
        .render(ctx)
        .map_err(|e| DispatchError::FilterRender(e.to_string()))?;
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
mod tests {
    use super::*;
    use rupu_orchestrator::Workflow;
    use serde_json::json;
    use std::sync::Mutex;

    struct RecordingDispatcher {
        calls: Mutex<Vec<String>>,
    }
    impl RecordingDispatcher {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl WorkflowDispatcher for RecordingDispatcher {
        async fn dispatch(&self, name: &str) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(name.to_string());
            Ok(())
        }
    }

    fn parse(s: &str) -> Workflow {
        Workflow::parse(s).expect("workflow parse")
    }

    fn wf(name: &str, event: &str, filter: Option<&str>) -> (String, Workflow) {
        let mut yaml = format!("name: {name}\ntrigger:\n  on: event\n  event: {event}\n");
        if let Some(f) = filter {
            yaml.push_str(&format!("  filter: \"{}\"\n", f.replace('"', "\\\"")));
        }
        yaml.push_str("steps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n");
        (name.into(), parse(&yaml))
    }

    #[tokio::test]
    async fn matching_event_with_no_filter_dispatches() {
        let candidates = vec![wf("review-pr", "github.pr.opened", None)];
        let d = RecordingDispatcher::new();
        let results = dispatch_event(
            "github.pr.opened",
            &json!({ "pull_request": { "number": 7 } }),
            &candidates,
            &d,
        )
        .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].fired);
        assert_eq!(d.calls(), vec!["review-pr"]);
    }

    #[tokio::test]
    async fn non_matching_event_id_is_skipped() {
        let candidates = vec![wf("review-pr", "github.pr.opened", None)];
        let d = RecordingDispatcher::new();
        let results = dispatch_event("github.pr.merged", &json!({}), &candidates, &d).await;
        assert!(results.is_empty());
        assert!(d.calls().is_empty());
    }

    #[tokio::test]
    async fn truthy_filter_dispatches() {
        let candidates = vec![wf(
            "review-pr",
            "github.pr.opened",
            Some("{{ event.repository.name == 'rupu' }}"),
        )];
        let d = RecordingDispatcher::new();
        let payload = json!({ "repository": { "name": "rupu" } });
        let results = dispatch_event("github.pr.opened", &payload, &candidates, &d).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].fired);
    }

    #[tokio::test]
    async fn falsy_filter_skips() {
        let candidates = vec![wf(
            "review-pr",
            "github.pr.opened",
            Some("{{ event.repository.name == 'other-repo' }}"),
        )];
        let d = RecordingDispatcher::new();
        let payload = json!({ "repository": { "name": "rupu" } });
        let results = dispatch_event("github.pr.opened", &payload, &candidates, &d).await;
        assert!(results.is_empty(), "filter should have rejected");
        assert!(d.calls().is_empty());
    }

    #[tokio::test]
    async fn multiple_workflows_can_match_same_event() {
        let candidates = vec![
            wf("review-pr", "github.pr.opened", None),
            wf("notify-slack", "github.pr.opened", None),
            wf("on-merge", "github.pr.merged", None),
        ];
        let d = RecordingDispatcher::new();
        let results = dispatch_event("github.pr.opened", &json!({}), &candidates, &d).await;
        assert_eq!(results.len(), 2);
        let names: Vec<_> = d.calls().into_iter().collect();
        assert!(names.contains(&"review-pr".to_string()));
        assert!(names.contains(&"notify-slack".to_string()));
    }
}
