//! `dispatch_agent` tool — invoke another agent as a tool call.
//!
//! Single-child synchronous dispatch. Hands the request off to the
//! [`AgentDispatcher`] wired onto the [`ToolContext`] by the
//! orchestrator's step factory; the dispatcher resolves the child
//! agent, allocates a sub-run, runs it to completion, and returns the
//! child's final assistant text. This tool then packages the outcome
//! into the JSON shape from the design spec § 3.1 and returns it to
//! the parent agent.
//!
//! Three gates are enforced before dispatch:
//!
//! 1. **Dispatcher present.** A run that wasn't wired with a
//!    dispatcher (bare `rupu run`, unit tests) errors out with
//!    `dispatcher_not_configured`.
//! 2. **Per-parent allowlist.** The requested agent must appear in
//!    the parent's `dispatchableAgents:` frontmatter list. Empty /
//!    unset list = no children allowed.
//! 3. **Recursion-depth ceiling.** Children at depth >= [`MAX_DEPTH`]
//!    can't dispatch grandchildren — this catches runaway recursion
//!    long before it becomes a real problem.
//!
//! Gate failures are surfaced as `Ok(ToolOutput { error: Some(...) })`
//! so the parent agent sees them as ordinary tool failures (and can
//! reason about them) rather than as runtime panics.
//!
//! See `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`.

use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

/// Hard ceiling on dispatch recursion depth. The design spec § 4.3
/// allows a per-agent override capped at a workspace ceiling of 8;
/// Plan 1 ships the workspace ceiling only.
pub const MAX_DEPTH: u32 = 5;

#[derive(Deserialize)]
struct Input {
    agent: String,
    prompt: String,
    /// Optional structured inputs the parent wants the child to see.
    /// For Plan 1 we serialize them into the child's user-message
    /// alongside `prompt`; the child still receives one consolidated
    /// turn-0 user message. Forward-compatible with Plan 2's
    /// per-agent template binding.
    #[serde(default)]
    inputs: Option<Value>,
}

/// `dispatch_agent` builtin. Registered in `default_tool_registry()`.
#[derive(Debug, Default, Clone)]
pub struct DispatchAgentTool;

#[async_trait]
impl Tool for DispatchAgentTool {
    fn name(&self) -> &'static str {
        "dispatch_agent"
    }

    fn description(&self) -> &'static str {
        "Run another agent synchronously as a tool call. Provide the child agent's name (must appear in this agent's dispatchableAgents frontmatter) and a prompt. The child runs to completion in its own context; you receive its final assistant text plus token + duration accounting. Use this to delegate review, search, or specialist tasks to a focused sub-agent."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "description": "Name of the agent to dispatch. Must be in the parent's dispatchableAgents list."
                },
                "prompt": {
                    "type": "string",
                    "description": "Initial user message the child agent receives."
                },
                "inputs": {
                    "type": "object",
                    "description": "Optional structured inputs forwarded into the child's prompt. Renders alongside `prompt` as a JSON block.",
                    "additionalProperties": true
                }
            },
            "required": ["agent", "prompt"]
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        let dispatcher = match ctx.dispatcher.as_ref() {
            Some(d) => d.clone(),
            None => {
                return Ok(err_output(
                    started,
                    "dispatcher_not_configured: this run was not started with a dispatcher; \
                     `dispatch_agent` only works inside a `rupu workflow run` invocation"
                        .to_string(),
                ));
            }
        };

        let allowlist = ctx.dispatchable_agents.as_deref().unwrap_or(&[]);
        if !allowlist.iter().any(|a| a == &i.agent) {
            return Ok(err_output(
                started,
                format!(
                    "agent_not_dispatchable: `{}` is not in this agent's `dispatchableAgents` allowlist (allowed: [{}])",
                    i.agent,
                    allowlist.join(", ")
                ),
            ));
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
            Some(id) => id,
            None => {
                return Ok(err_output(
                    started,
                    "no_parent_run_id: dispatcher requires a parent run id to anchor the sub-run \
                     directory; this run was not started by the workflow runner"
                        .to_string(),
                ));
            }
        };

        let child_prompt = match i.inputs {
            Some(inputs) if !inputs.is_null() && !inputs.as_object().is_some_and(|o| o.is_empty()) => {
                let pretty = serde_json::to_string_pretty(&inputs).unwrap_or_else(|_| inputs.to_string());
                format!("{}\n\nInputs (JSON):\n{}", i.prompt, pretty)
            }
            _ => i.prompt,
        };

        match dispatcher
            .dispatch(&i.agent, child_prompt, parent_run_id, ctx.depth)
            .await
        {
            Ok(outcome) => {
                let body = json!({
                    "ok": outcome.success,
                    "agent": outcome.agent,
                    "output": outcome.output,
                    // Plan 1 doesn't surface findings; Plan 2 will populate
                    // when the child agent has `outputFormat: json` and
                    // emits a parseable findings array. Empty for now.
                    "findings": [],
                    "tokens_used": outcome.tokens_used,
                    "duration_ms": outcome.duration_ms,
                    "transcript_path": outcome.transcript_path.display().to_string(),
                    "sub_run_id": outcome.sub_run_id,
                });
                Ok(ToolOutput {
                    stdout: serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
                    error: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                })
            }
            Err(e) => Ok(err_output(started, format!("dispatch failed: {e}"))),
        }
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
    use std::sync::Arc;

    #[derive(Debug)]
    struct StubDispatcher {
        return_output: String,
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
            Ok(DispatchOutcome {
                agent: agent_name.to_string(),
                sub_run_id: "sub_TEST".into(),
                transcript_path: PathBuf::from("/tmp/sub_TEST/transcript.jsonl"),
                output: self.return_output.clone(),
                success: true,
                tokens_used: 42,
                duration_ms: 10,
            })
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
    async fn errors_when_dispatcher_missing() {
        let tool = DispatchAgentTool;
        let ctx = ctx_with(None, Some(vec!["reviewer".into()]), Some("run_X".into()), 0);
        let out = tool
            .invoke(json!({ "agent": "reviewer", "prompt": "hi" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_some(), "expected error, got {out:?}");
        assert!(out
            .error
            .as_deref()
            .unwrap()
            .contains("dispatcher_not_configured"));
    }

    #[tokio::test]
    async fn errors_when_agent_not_in_allowlist() {
        let tool = DispatchAgentTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher {
            return_output: "ok".into(),
        });
        let ctx = ctx_with(Some(disp), Some(vec!["reviewer".into()]), Some("run_X".into()), 0);
        let out = tool
            .invoke(json!({ "agent": "intruder", "prompt": "hi" }), &ctx)
            .await
            .unwrap();
        assert!(out
            .error
            .as_deref()
            .unwrap()
            .contains("agent_not_dispatchable"));
    }

    #[tokio::test]
    async fn errors_when_depth_at_ceiling() {
        let tool = DispatchAgentTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher {
            return_output: "ok".into(),
        });
        let ctx = ctx_with(
            Some(disp),
            Some(vec!["reviewer".into()]),
            Some("run_X".into()),
            MAX_DEPTH,
        );
        let out = tool
            .invoke(json!({ "agent": "reviewer", "prompt": "hi" }), &ctx)
            .await
            .unwrap();
        assert!(out
            .error
            .as_deref()
            .unwrap()
            .contains("max_dispatch_depth_exceeded"));
    }

    #[tokio::test]
    async fn returns_spec_shape_on_success() {
        let tool = DispatchAgentTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher {
            return_output: "child output".into(),
        });
        let ctx = ctx_with(Some(disp), Some(vec!["reviewer".into()]), Some("run_X".into()), 0);
        let out = tool
            .invoke(json!({ "agent": "reviewer", "prompt": "hi" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        let parsed: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(parsed["ok"], json!(true));
        assert_eq!(parsed["agent"], "reviewer");
        assert_eq!(parsed["output"], "child output");
        assert_eq!(parsed["sub_run_id"], "sub_TEST");
        assert_eq!(parsed["tokens_used"], 42);
        assert!(parsed["transcript_path"].as_str().unwrap().contains("sub_TEST"));
        assert_eq!(parsed["findings"], json!([]));
    }

    #[tokio::test]
    async fn merges_inputs_into_prompt() {
        let tool = DispatchAgentTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher {
            return_output: "ok".into(),
        });
        let ctx = ctx_with(Some(disp), Some(vec!["reviewer".into()]), Some("run_X".into()), 0);
        let out = tool
            .invoke(
                json!({ "agent": "reviewer", "prompt": "review", "inputs": { "subject": "x" } }),
                &ctx,
            )
            .await
            .unwrap();
        // We can't observe the actual child prompt from the stub's
        // recorded state without more plumbing, but the call must
        // succeed with inputs supplied — proves the merge path
        // doesn't fault.
        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
    }

    #[tokio::test]
    async fn errors_when_parent_run_id_missing() {
        let tool = DispatchAgentTool;
        let disp: Arc<dyn AgentDispatcher> = Arc::new(StubDispatcher {
            return_output: "ok".into(),
        });
        let ctx = ctx_with(Some(disp), Some(vec!["reviewer".into()]), None, 0);
        let out = tool
            .invoke(json!({ "agent": "reviewer", "prompt": "hi" }), &ctx)
            .await
            .unwrap();
        assert!(out.error.as_deref().unwrap().contains("no_parent_run_id"));
    }
}
