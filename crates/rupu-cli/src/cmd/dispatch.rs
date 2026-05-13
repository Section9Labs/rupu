//! `CliAgentDispatcher` — the cli-side `AgentDispatcher` impl that the
//! `dispatch_agent` builtin tool calls into.
//!
//! Spawns a child agent run synchronously: loads the agent spec,
//! allocates a sub-run directory under the parent's run dir, builds a
//! provider via [`rupu_runtime::provider_factory`], threads the same
//! dispatcher Arc into the child's [`ToolContext`] (so grandchildren
//! up to `MAX_DEPTH` can dispatch too), runs the child to completion,
//! and reads the final assistant text out of the persisted transcript.
//!
//! See `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`.

use async_trait::async_trait;
use rupu_agent::runner::{run_agent, AgentRunOpts, BypassDecider, PermissionDecider};
use rupu_orchestrator::RunStore;
use rupu_runtime::provider_factory;
use rupu_tools::{AgentDispatcher, DispatchError, DispatchOutcome, ToolContext};
use rupu_transcript::{Event as TxEvent, JsonlReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// CLI-side dispatcher. Holds the shared run-store + auth + workspace
/// state needed to spawn a child run, and a self-reference so children
/// inherit the same dispatcher Arc on their tool context.
pub struct CliAgentDispatcher {
    global: PathBuf,
    project_root: Option<PathBuf>,
    workspace_id: String,
    workspace_path: PathBuf,
    resolver: Arc<rupu_auth::KeychainResolver>,
    parent_mode_str: String,
    mcp_registry: Arc<rupu_scm::Registry>,
    run_store: Arc<RunStore>,
    /// Self-reference as a trait object so each child's `ToolContext`
    /// can carry the same dispatcher Arc — without it, grandchildren
    /// would see `dispatcher: None` and fail with "no dispatcher".
    /// Populated by [`Self::new`] after Arc construction.
    self_dyn: OnceLock<Arc<dyn AgentDispatcher>>,
}

impl std::fmt::Debug for CliAgentDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliAgentDispatcher")
            .field("global", &self.global)
            .field("project_root", &self.project_root)
            .field("workspace_id", &self.workspace_id)
            .field("workspace_path", &self.workspace_path)
            .field("parent_mode_str", &self.parent_mode_str)
            .finish()
    }
}

impl CliAgentDispatcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        global: PathBuf,
        project_root: Option<PathBuf>,
        workspace_id: String,
        workspace_path: PathBuf,
        resolver: Arc<rupu_auth::KeychainResolver>,
        parent_mode_str: String,
        mcp_registry: Arc<rupu_scm::Registry>,
        run_store: Arc<RunStore>,
    ) -> Arc<Self> {
        let arc = Arc::new(Self {
            global,
            project_root,
            workspace_id,
            workspace_path,
            resolver,
            parent_mode_str,
            mcp_registry,
            run_store,
            self_dyn: OnceLock::new(),
        });
        let dyn_arc: Arc<dyn AgentDispatcher> = arc.clone();
        let _ = arc.self_dyn.set(dyn_arc);
        arc
    }

    fn self_arc_dyn(&self) -> Arc<dyn AgentDispatcher> {
        self.self_dyn
            .get()
            .expect("CliAgentDispatcher::new always populates self_dyn")
            .clone()
    }
}

#[async_trait]
impl AgentDispatcher for CliAgentDispatcher {
    async fn dispatch(
        &self,
        agent_name: &str,
        prompt: String,
        parent_run_id: &str,
        parent_depth: u32,
    ) -> Result<DispatchOutcome, DispatchError> {
        let project_agents_parent = self.project_root.as_ref().map(|p| p.join(".rupu"));
        let spec =
            rupu_agent::load_agent(&self.global, project_agents_parent.as_deref(), agent_name)
                .map_err(|_| DispatchError::AgentNotFound {
                    agent: agent_name.to_string(),
                })?;

        let (sub_run_id, transcript_path) = self
            .run_store
            .create_sub_run(parent_run_id, agent_name)
            .map_err(|e| DispatchError::RunStore(e.to_string()))?;

        let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
        let model = spec
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".into());
        let provider = match provider_factory::build_for_provider(
            &provider_name,
            &model,
            spec.auth,
            self.resolver.as_ref(),
        )
        .await
        {
            Ok((_resolved, p)) => p,
            Err(e) => return Err(DispatchError::ProviderBuild(e.to_string())),
        };

        let child_mode_str = spec
            .permission_mode
            .clone()
            .unwrap_or_else(|| self.parent_mode_str.clone());
        let child_depth = parent_depth + 1;

        let child_tool_ctx = ToolContext {
            workspace_path: self.workspace_path.clone(),
            bash_env_allowlist: Vec::new(),
            bash_timeout_secs: 120,
            dispatcher: Some(self.self_arc_dyn()),
            dispatchable_agents: spec.dispatchable_agents.clone(),
            parent_run_id: Some(sub_run_id.clone()),
            depth: child_depth,
        };

        let opts = AgentRunOpts {
            agent_name: spec.name.clone(),
            agent_system_prompt: spec.system_prompt.clone(),
            agent_tools: spec.tools.clone(),
            provider,
            provider_name,
            model,
            run_id: sub_run_id.clone(),
            workspace_id: self.workspace_id.clone(),
            workspace_path: self.workspace_path.clone(),
            transcript_path: transcript_path.clone(),
            max_turns: spec.max_turns.unwrap_or(50),
            decider: Arc::new(BypassDecider) as Arc<dyn PermissionDecider>,
            tool_context: child_tool_ctx,
            user_message: prompt,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: child_mode_str,
            no_stream: false,
            // The parent's printer renders the child as a callout from
            // the `dispatch_agent` tool result; suppress the child's
            // own stdout writes so they don't double up.
            suppress_stream_stdout: true,
            mcp_registry: Some(Arc::clone(&self.mcp_registry)),
            effort: spec.effort,
            context_window: spec.context_window,
            output_format: spec.output_format,
            anthropic_task_budget: spec.anthropic_task_budget,
            anthropic_context_management: spec.anthropic_context_management,
            anthropic_speed: spec.anthropic_speed,
            parent_run_id: Some(parent_run_id.to_string()),
            depth: child_depth,
            dispatchable_agents: spec.dispatchable_agents.clone(),
            step_id: String::new(),
            on_tool_call: None,
        };

        let started = std::time::Instant::now();
        let run_result = run_agent(opts)
            .await
            .map_err(|e| DispatchError::ChildRun(e.to_string()))?;
        let duration_ms = started.elapsed().as_millis() as u64;

        let output = read_final_assistant_text(&transcript_path).unwrap_or_default();

        Ok(DispatchOutcome {
            agent: agent_name.to_string(),
            sub_run_id,
            transcript_path,
            output,
            success: true,
            tokens_used: run_result.total_tokens_in + run_result.total_tokens_out,
            duration_ms,
        })
    }
}

/// Walk the persisted transcript and return the last non-empty
/// `AssistantMessage.content`. Used as the child's `output` in the
/// dispatch tool's return payload — same shape as a top-level run's
/// final assistant text.
fn read_final_assistant_text(path: &Path) -> Option<String> {
    let iter = JsonlReader::iter(path).ok()?;
    let mut last: Option<String> = None;
    for ev in iter {
        if let Ok(TxEvent::AssistantMessage { content, .. }) = ev {
            if !content.trim().is_empty() {
                last = Some(content);
            }
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_transcript::{Event, JsonlWriter, RunMode, RunStatus};
    use tempfile::TempDir;

    #[test]
    fn read_final_assistant_text_returns_last_non_empty_assistant_chunk() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut w = JsonlWriter::create(&path).unwrap();
        w.write(&Event::RunStart {
            run_id: "r".into(),
            workspace_id: "ws".into(),
            agent: "a".into(),
            provider: "anthropic".into(),
            model: "m".into(),
            started_at: chrono::Utc::now(),
            mode: RunMode::Bypass,
        })
        .unwrap();
        w.write(&Event::AssistantMessage {
            content: "first".into(),
            thinking: None,
        })
        .unwrap();
        w.write(&Event::AssistantMessage {
            content: "  ".into(),
            thinking: None,
        })
        .unwrap();
        w.write(&Event::AssistantMessage {
            content: "final answer".into(),
            thinking: None,
        })
        .unwrap();
        w.write(&Event::RunComplete {
            run_id: "r".into(),
            status: RunStatus::Ok,
            total_tokens: 0,
            duration_ms: 0,
            error: None,
        })
        .unwrap();
        w.flush().unwrap();

        assert_eq!(
            read_final_assistant_text(&path),
            Some("final answer".to_string())
        );
    }

    #[test]
    fn read_final_assistant_text_returns_none_for_empty_transcript() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        assert_eq!(read_final_assistant_text(&path), None);
    }
}
