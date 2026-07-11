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
use rupu_orchestrator::executor::{Event as OrchEvent, EventSink};
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
    /// The parent run's event sink, if one is wired up. Lets `dispatch()`
    /// emit `DispatchStarted`/`DispatchCompleted` so the live view can
    /// render the child as a node under the active step. `None` in
    /// contexts with no run-level events.jsonl (e.g. some test harnesses)
    /// — emission is then a no-op and behavior is unchanged.
    event_sink: Option<Arc<dyn EventSink>>,
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
        event_sink: Option<Arc<dyn EventSink>>,
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
            event_sink,
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

    /// Best-effort `DispatchCompleted` emission — guards the `Option` and
    /// never fails the child (or parent) run. Called from every exit
    /// path of `dispatch()` reached after the matching `DispatchStarted`
    /// was emitted.
    fn emit_dispatch_completed(
        &self,
        parent_run_id: &str,
        sub_run_id: &str,
        success: bool,
        tokens_in: u64,
        tokens_out: u64,
    ) {
        if let Some(sink) = &self.event_sink {
            sink.emit(
                parent_run_id,
                &OrchEvent::DispatchCompleted {
                    run_id: parent_run_id.to_string(),
                    sub_run_id: sub_run_id.to_string(),
                    success,
                    tokens_in,
                    tokens_out,
                },
            );
        }
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

        if let Some(sink) = &self.event_sink {
            sink.emit(
                parent_run_id,
                &OrchEvent::DispatchStarted {
                    run_id: parent_run_id.to_string(),
                    sub_run_id: sub_run_id.clone(),
                    agent: Some(agent_name.to_string()),
                    transcript_path: transcript_path.clone(),
                },
            );
        }

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
            Err(e) => {
                self.emit_dispatch_completed(parent_run_id, &sub_run_id, false, 0, 0);
                return Err(DispatchError::ProviderBuild(e.to_string()));
            }
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
            coverage_writer: None,
            surface_tag: None,
            run_id: None,
            model: None,
            tool_mappings: None,
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
            output_schema: spec.output_schema.clone(),
            anthropic_task_budget: spec.anthropic_task_budget,
            anthropic_context_management: spec.anthropic_context_management,
            anthropic_speed: spec.anthropic_speed,
            parent_run_id: Some(parent_run_id.to_string()),
            depth: child_depth,
            dispatchable_agents: spec.dispatchable_agents.clone(),
            step_id: String::new(),
            on_tool_call: None,
            on_stream_event: None,
            concerns: spec.concerns.clone(),
            max_tokens: spec
                .max_tokens
                .unwrap_or(rupu_agent::runner::DEFAULT_MAX_TOKENS),
            scope_name: None,
            surface_tag: None,
            context_window_tokens: spec.context_window_tokens,
            compact_at_percent: spec.compact_at_percent,
            pause: None,
        };

        let started = std::time::Instant::now();
        let run_result = match run_agent(opts).await {
            Ok(r) => r,
            Err(e) => {
                self.emit_dispatch_completed(parent_run_id, &sub_run_id, false, 0, 0);
                return Err(DispatchError::ChildRun(e.to_string()));
            }
        };
        let duration_ms = started.elapsed().as_millis() as u64;

        let output = read_final_assistant_text(&transcript_path).unwrap_or_default();

        self.emit_dispatch_completed(
            parent_run_id,
            &sub_run_id,
            true,
            run_result.total_tokens_in,
            run_result.total_tokens_out,
        );

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

    /// Records every emitted event as `(run_id, Event)` for assertion.
    #[derive(Default)]
    struct CapturingSink {
        events: std::sync::Mutex<Vec<(String, OrchEvent)>>,
    }

    impl EventSink for CapturingSink {
        fn emit(&self, run_id: &str, ev: &OrchEvent) {
            self.events
                .lock()
                .unwrap()
                .push((run_id.to_string(), ev.clone()));
        }
    }

    /// Exercises `CliAgentDispatcher::dispatch()` end to end against the
    /// `RUPU_MOCK_PROVIDER_SCRIPT` seam (the same test-only provider
    /// factory hook `rupu-cli`'s own CLI integration tests use — see
    /// `tests/cli_run.rs`) so the child's agent loop runs for real
    /// without any network access. Asserts `DispatchStarted` lands
    /// before `DispatchCompleted`, both carrying the same `sub_run_id`
    /// as the returned `DispatchOutcome`, and that token counts flow
    /// through to `DispatchCompleted`.
    #[tokio::test]
    async fn dispatch_emits_started_then_completed_with_matching_sub_run_id() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let sink = Arc::new(CapturingSink::default());
        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            Some(sink.clone() as Arc<dyn EventSink>),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        let outcome = result.expect("dispatch should succeed against the mock provider");

        let events = sink.events.lock().unwrap().clone();
        assert_eq!(
            events.len(),
            2,
            "expected exactly Started + Completed, got {events:?}"
        );

        match &events[0] {
            (
                run_id,
                OrchEvent::DispatchStarted {
                    sub_run_id,
                    agent,
                    transcript_path,
                    ..
                },
            ) => {
                assert_eq!(run_id, "parent_run_1");
                assert_eq!(sub_run_id, &outcome.sub_run_id);
                assert_eq!(agent.as_deref(), Some("child"));
                assert_eq!(transcript_path, &outcome.transcript_path);
            }
            other => panic!("expected DispatchStarted first, got {other:?}"),
        }

        match &events[1] {
            (
                run_id,
                OrchEvent::DispatchCompleted {
                    sub_run_id,
                    success,
                    tokens_in,
                    tokens_out,
                    ..
                },
            ) => {
                assert_eq!(run_id, "parent_run_1");
                assert_eq!(sub_run_id, &outcome.sub_run_id);
                assert!(*success);
                assert_eq!(*tokens_in, 1);
                assert_eq!(*tokens_out, 1);
            }
            other => panic!("expected DispatchCompleted second, got {other:?}"),
        }
    }

    /// `event_sink: None` (the harness other dispatch tests already use)
    /// must not change `dispatch()`'s behavior — it's a pure no-op path.
    #[tokio::test]
    async fn dispatch_with_no_sink_still_succeeds() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            None,
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        assert!(
            result.is_ok(),
            "dispatch with no event sink should behave exactly as before"
        );
    }
}
