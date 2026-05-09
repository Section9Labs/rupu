//! `dispatch_agents_parallel` tool — fan-out N child agents concurrently.
//!
//! Mirrors the workflow-layer `parallel:` shape but at the agent layer.
//! Each request in the `agents` list is dispatched on its own
//! `tokio::spawn` task, throttled by an [`Arc<Semaphore>`] keyed on the
//! optional `max_parallel` cap (default = number of agents = full
//! parallelism). Children run on the same [`AgentDispatcher`] handle
//! that backs `dispatch_agent`, so the existing per-agent allowlist +
//! recursion-depth gates apply uniformly.
//!
//! The tool returns *after* every child has completed — child frames
//! render in the parent's printer in declared order once all
//! transcripts are on disk. Live interleaved streaming is a Plan 3
//! follow-up.
//!
//! Three gates are enforced **before** any child is spawned (so a
//! gate failure on the last child doesn't waste work on the earlier
//! ones):
//!
//! 1. **Dispatcher present** — same as `dispatch_agent`.
//! 2. **Per-parent allowlist** — every requested agent name must
//!    appear in the parent's `dispatchableAgents:` list.
//! 3. **Recursion-depth ceiling** — current depth must be strictly
//!    less than [`crate::dispatch_agent::MAX_DEPTH`].
//!
//! See `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`
//! § 3.1 for the on-the-wire request / response shape.

use crate::dispatch_agent::MAX_DEPTH;
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

#[derive(Deserialize)]
struct Input {
    agents: Vec<AgentRequest>,
    /// Concurrency cap. `None` ⇒ run every child concurrently.
    #[serde(default)]
    max_parallel: Option<usize>,
}

#[derive(Deserialize, Clone)]
struct AgentRequest {
    /// Caller-chosen identifier. Used as the key in the response
    /// `results` map and as the headline in the per-child callout.
    id: String,
    agent: String,
    prompt: String,
    /// Optional structured inputs forwarded into the child's prompt
    /// alongside `prompt`. Same shape and semantics as
    /// `dispatch_agent.inputs`.
    #[serde(default)]
    inputs: Option<Value>,
}

/// `dispatch_agents_parallel` builtin. Registered in `default_tool_registry()`.
#[derive(Debug, Default, Clone)]
pub struct DispatchAgentsParallelTool;

#[async_trait]
impl Tool for DispatchAgentsParallelTool {
    fn name(&self) -> &'static str {
        "dispatch_agents_parallel"
    }

    fn description(&self) -> &'static str {
        "Run several agents in parallel and aggregate their results. Provide a list of `agents`, each with `{ id, agent, prompt }`. Every agent must appear in this agent's dispatchableAgents allowlist. Returns a map keyed by `id` with each child's output, tokens, and transcript. Use this when N specialist reviews can run independently — for sequential or single-child dispatches use `dispatch_agent` instead."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agents": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Caller-chosen key. Distinguishes children in the result map and child-frame headline."
                            },
                            "agent": {
                                "type": "string",
                                "description": "Agent name. Must appear in dispatchableAgents."
                            },
                            "prompt": {
                                "type": "string",
                                "description": "Initial user message for the child agent."
                            },
                            "inputs": {
                                "type": "object",
                                "description": "Optional structured inputs forwarded alongside the prompt.",
                                "additionalProperties": true
                            }
                        },
                        "required": ["id", "agent", "prompt"]
                    }
                },
                "max_parallel": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Concurrency cap. Defaults to the number of agents (full parallelism)."
                }
            },
            "required": ["agents"]
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        if i.agents.is_empty() {
            return Ok(err_output(
                started,
                "no_agents: `agents` must be non-empty".to_string(),
            ));
        }

        // Reject duplicate `id`s up front — the response is a map
        // keyed on id, so duplicates would silently lose work.
        {
            let mut seen = std::collections::BTreeSet::new();
            for req in &i.agents {
                if !seen.insert(req.id.clone()) {
                    return Ok(err_output(
                        started,
                        format!("duplicate_id: `{}` appears more than once in `agents`", req.id),
                    ));
                }
            }
        }

        let dispatcher = match ctx.dispatcher.as_ref() {
            Some(d) => d.clone(),
            None => {
                return Ok(err_output(
                    started,
                    "dispatcher_not_configured: this run was not started with a dispatcher; \
                     `dispatch_agents_parallel` only works inside a `rupu workflow run` invocation"
                        .to_string(),
                ));
            }
        };

        let allowlist = ctx.dispatchable_agents.as_deref().unwrap_or(&[]);
        for req in &i.agents {
            if !allowlist.iter().any(|a| a == &req.agent) {
                return Ok(err_output(
                    started,
                    format!(
                        "agent_not_dispatchable: `{}` (id `{}`) is not in this agent's `dispatchableAgents` allowlist (allowed: [{}])",
                        req.agent,
                        req.id,
                        allowlist.join(", ")
                    ),
                ));
            }
        }

        if ctx.depth >= MAX_DEPTH {
            return Ok(err_output(
                started,
                format!(
                    "max_dispatch_depth_exceeded: current depth {} >= ceiling {MAX_DEPTH}",
                    ctx.depth
                ),
            ));
        }

        let parent_run_id = match ctx.parent_run_id.as_deref() {
            Some(id) => id.to_string(),
            None => {
                return Ok(err_output(
                    started,
                    "no_parent_run_id: dispatcher requires a parent run id to anchor sub-run \
                     directories; this run was not started by the workflow runner"
                        .to_string(),
                ));
            }
        };

        let max_parallel = i.max_parallel.unwrap_or(i.agents.len()).max(1);
        let semaphore = Arc::new(Semaphore::new(max_parallel));

        let parent_depth = ctx.depth;
        let mut handles = Vec::with_capacity(i.agents.len());
        for (idx, req) in i.agents.iter().cloned().enumerate() {
            let permit_sem = Arc::clone(&semaphore);
            let dispatcher = dispatcher.clone();
            let parent_run_id = parent_run_id.clone();
            let child_prompt = render_child_prompt(&req.prompt, req.inputs.as_ref());
            handles.push(tokio::spawn(async move {
                let _permit = permit_sem
                    .acquire_owned()
                    .await
                    .expect("semaphore not closed");
                let outcome = dispatcher
                    .dispatch(&req.agent, child_prompt, &parent_run_id, parent_depth)
                    .await;
                (idx, req, outcome)
            }));
        }

        let mut entries: Vec<(usize, AgentRequest, Result<_, _>)> = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(triple) => entries.push(triple),
                Err(e) => {
                    return Ok(err_output(started, format!("child_join_error: {e}")));
                }
            }
        }
        entries.sort_by_key(|(idx, _, _)| *idx);

        let mut all_succeeded = true;
        let mut results = Map::new();
        for (_idx, req, outcome) in entries {
            match outcome {
                Ok(o) => {
                    if !o.success {
                        all_succeeded = false;
                    }
                    results.insert(
                        req.id,
                        json!({
                            "ok": o.success,
                            "agent": o.agent,
                            "output": o.output,
                            "findings": [],
                            "tokens_used": o.tokens_used,
                            "duration_ms": o.duration_ms,
                            "transcript_path": o.transcript_path.display().to_string(),
                            "sub_run_id": o.sub_run_id,
                        }),
                    );
                }
                Err(e) => {
                    all_succeeded = false;
                    results.insert(
                        req.id,
                        json!({
                            "ok": false,
                            "agent": req.agent,
                            "error": e.to_string(),
                        }),
                    );
                }
            }
        }

        let body = json!({
            "ok": all_succeeded,
            "results": Value::Object(results),
            "all_succeeded": all_succeeded,
        });
        Ok(ToolOutput {
            stdout: serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
        })
    }
}

fn render_child_prompt(prompt: &str, inputs: Option<&Value>) -> String {
    match inputs {
        Some(v) if !v.is_null() && !v.as_object().is_some_and(|o| o.is_empty()) => {
            let pretty = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
            format!("{prompt}\n\nInputs (JSON):\n{pretty}")
        }
        _ => prompt.to_string(),
    }
}

fn err_output(started: Instant, msg: String) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(msg),
        duration_ms: started.elapsed().as_millis() as u64,
        derived: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{AgentDispatcher, DispatchError, DispatchOutcome};
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct StubDispatcher {
        /// Per-agent canned outcome. `Ok` returns a synthetic outcome,
        /// `Err` simulates a dispatch-level failure (e.g. agent not found).
        scripted: Mutex<std::collections::BTreeMap<String, Result<DispatchOutcome, DispatchError>>>,
    }

    impl StubDispatcher {
        fn new(
            entries: impl IntoIterator<Item = (&'static str, Result<DispatchOutcome, DispatchError>)>,
        ) -> Self {
            Self {
                scripted: Mutex::new(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect()),
            }
        }

        fn ok(agent: &str, output: &str, tokens: u64) -> DispatchOutcome {
            DispatchOutcome {
                agent: agent.to_string(),
                sub_run_id: format!("sub_{agent}"),
                transcript_path: PathBuf::from(format!("/tmp/{agent}.jsonl")),
                output: output.to_string(),
                success: true,
                tokens_used: tokens,
                duration_ms: 1,
            }
        }
    }

    #[async_trait]
    impl AgentDispatcher for StubDispatcher {
        async fn dispatch(
            &self,
            agent_name: &str,
            _prompt: String,
            _parent_run_id: &str,
            _parent_depth: u32,
        ) -> Result<DispatchOutcome, DispatchError> {
            let mut scripted = self.scripted.lock().unwrap();
            // Each agent is dispatched at most once per test, so consume
            // the scripted entry. DispatchError isn't `Clone`, so we
            // can't get-and-cache.
            match scripted.remove(agent_name) {
                Some(o) => o,
                None => Err(DispatchError::AgentNotFound {
                    agent: agent_name.to_string(),
                }),
            }
        }
    }

    fn ctx_with(
        dispatcher: Option<Arc<dyn AgentDispatcher>>,
        allowlist: Option<Vec<String>>,
        parent_run_id: Option<String>,
        depth: u32,
    ) -> ToolContext {
        ToolContext {
            dispatcher,
            dispatchable_agents: allowlist,
            parent_run_id,
            depth,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn errors_when_agents_list_empty() {
        let tool = DispatchAgentsParallelTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher::new([]));
        let ctx = ctx_with(Some(disp), Some(vec!["sec".into()]), Some("run_X".into()), 0);
        let out = tool
            .invoke(json!({ "agents": [] }), &ctx)
            .await
            .unwrap();
        assert!(out.error.as_deref().unwrap().contains("no_agents"));
    }

    #[tokio::test]
    async fn errors_on_duplicate_id() {
        let tool = DispatchAgentsParallelTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher::new([(
            "sec",
            Ok(StubDispatcher::ok("sec", "ok", 1)),
        )]));
        let ctx = ctx_with(Some(disp), Some(vec!["sec".into()]), Some("run_X".into()), 0);
        let out = tool
            .invoke(
                json!({ "agents": [
                    { "id": "x", "agent": "sec", "prompt": "a" },
                    { "id": "x", "agent": "sec", "prompt": "b" },
                ]}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.error.as_deref().unwrap().contains("duplicate_id"));
    }

    #[tokio::test]
    async fn errors_when_any_agent_outside_allowlist() {
        let tool = DispatchAgentsParallelTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher::new([
            ("sec", Ok(StubDispatcher::ok("sec", "s", 1))),
            ("bad", Ok(StubDispatcher::ok("bad", "b", 1))),
        ]));
        // Only `sec` is in the allowlist — `bad` should trip the gate.
        let ctx = ctx_with(Some(disp), Some(vec!["sec".into()]), Some("run_X".into()), 0);
        let out = tool
            .invoke(
                json!({ "agents": [
                    { "id": "a", "agent": "sec", "prompt": "p" },
                    { "id": "b", "agent": "bad", "prompt": "p" },
                ]}),
                &ctx,
            )
            .await
            .unwrap();
        let msg = out.error.as_deref().unwrap();
        assert!(msg.contains("agent_not_dispatchable"), "got: {msg}");
        assert!(msg.contains("`bad`"), "should name the offender: {msg}");
    }

    #[tokio::test]
    async fn errors_when_depth_at_ceiling() {
        let tool = DispatchAgentsParallelTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher::new([(
            "sec",
            Ok(StubDispatcher::ok("sec", "ok", 1)),
        )]));
        let ctx = ctx_with(
            Some(disp),
            Some(vec!["sec".into()]),
            Some("run_X".into()),
            MAX_DEPTH,
        );
        let out = tool
            .invoke(
                json!({ "agents": [{ "id": "a", "agent": "sec", "prompt": "p" }]}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out
            .error
            .as_deref()
            .unwrap()
            .contains("max_dispatch_depth_exceeded"));
    }

    #[tokio::test]
    async fn returns_results_keyed_by_id_in_input_order() {
        let tool = DispatchAgentsParallelTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher::new([
            ("sec", Ok(StubDispatcher::ok("sec", "sec output", 100))),
            ("perf", Ok(StubDispatcher::ok("perf", "perf output", 200))),
        ]));
        let ctx = ctx_with(
            Some(disp),
            Some(vec!["sec".into(), "perf".into()]),
            Some("run_X".into()),
            0,
        );
        let out = tool
            .invoke(
                json!({ "agents": [
                    { "id": "s", "agent": "sec", "prompt": "review auth" },
                    { "id": "p", "agent": "perf", "prompt": "review perf" },
                ]}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        let parsed: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(parsed["ok"], json!(true));
        assert_eq!(parsed["all_succeeded"], json!(true));
        assert_eq!(parsed["results"]["s"]["output"], "sec output");
        assert_eq!(parsed["results"]["s"]["tokens_used"], 100);
        assert_eq!(parsed["results"]["p"]["output"], "perf output");
        assert_eq!(parsed["results"]["p"]["tokens_used"], 200);
    }

    #[tokio::test]
    async fn mixed_success_marks_all_succeeded_false_but_returns_others() {
        let tool = DispatchAgentsParallelTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher::new([
            ("sec", Ok(StubDispatcher::ok("sec", "ok", 5))),
            (
                "broken",
                Err(DispatchError::ChildRun("provider exploded".into())),
            ),
        ]));
        let ctx = ctx_with(
            Some(disp),
            Some(vec!["sec".into(), "broken".into()]),
            Some("run_X".into()),
            0,
        );
        let out = tool
            .invoke(
                json!({ "agents": [
                    { "id": "good", "agent": "sec", "prompt": "p" },
                    { "id": "bad", "agent": "broken", "prompt": "p" },
                ]}),
                &ctx,
            )
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(parsed["all_succeeded"], json!(false));
        assert_eq!(parsed["results"]["good"]["ok"], json!(true));
        assert_eq!(parsed["results"]["bad"]["ok"], json!(false));
        assert!(parsed["results"]["bad"]["error"]
            .as_str()
            .unwrap()
            .contains("provider exploded"));
    }
}
