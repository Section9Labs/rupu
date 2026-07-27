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
use rupu_transcript::{Event as TxEvent, JsonlReader, JsonlWriter};
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
    /// `default_provider` from `config.toml`. Used when the dispatched
    /// agent pins no `provider:`. `None` falls back to
    /// `provider_factory::FALLBACK_PROVIDER`.
    default_provider: Option<String>,
    /// `default_model` from `config.toml`. Used when the dispatched agent
    /// pins no `model:` — without it a sub-agent resolved a different model
    /// than the very same agent would as a top-level `rupu run` or a
    /// workflow step (ISSUES.md I-8, the fourth I-1/I-2 site).
    default_model: Option<String>,
    /// OpenAI-compatible provider params resolved from `config.toml`, keyed
    /// by provider name. Lets a dispatched sub-agent reach a config-declared
    /// `[providers.<name>] kind = "openai-compatible"` endpoint the same way
    /// `rupu run` and workflow steps do. Empty when none are declared.
    openai_compatible: std::collections::HashMap<String, provider_factory::OpenAiCompatibleParams>,
    /// Resolved `[providers.<name>]` runtime knobs, keyed by provider name.
    /// Lets a dispatched sub-agent honor `timeout_ms` / `max_retries` /
    /// `max_concurrency` / `org_id` exactly as `rupu run` does
    /// (ISSUES.md I-9…I-12). Empty ⇒ documented defaults.
    provider_tuning: std::collections::HashMap<String, rupu_providers::ProviderTuning>,
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
        default_provider: Option<String>,
        default_model: Option<String>,
        openai_compatible: std::collections::HashMap<
            String,
            provider_factory::OpenAiCompatibleParams,
        >,
        provider_tuning: std::collections::HashMap<String, rupu_providers::ProviderTuning>,
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
            default_provider,
            default_model,
            openai_compatible,
            provider_tuning,
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
        // KNOWN LIMITATION (tool_audit design §4/review IMPORTANT 4): the
        // `AgentDispatcher::dispatch` trait (rupu-tools) takes no narrowed
        // tool roster and no audit callback, and this is the ONLY
        // production impl — `dispatch_agent`/`dispatch_agents_parallel`
        // call straight into it via `ctx.dispatcher`. So a step narrowed by
        // `actions:` (e.g. read-only `issues.*`) whose agent retains
        // `dispatch_agent` (a builtin, itself correctly exempt from
        // narrowing per spec §2) can have its CHILD agent run with the
        // child's OWN unrestricted `tools:` grant — `agent_tools:
        // spec.tools.clone()` below never intersects with whatever the
        // calling step narrowed — and every catalog call the child makes
        // is invisible to this run's `tool_audit` trail (the child gets
        // its own transcript/audit machinery, but nothing here ties it
        // back to the parent step's narrowing). Threading the narrowed
        // roster + an audit callback through would require changing the
        // `AgentDispatcher` trait signature and `ToolContext` (rupu-tools,
        // a port crate `rupu-agent` itself must not gain a reverse
        // dependency on) plus every impl/mock — out of scope for this
        // fix. Never silent: warn on every dispatch, and leave a
        // transcript-visible notice on the child's own run (below).
        tracing::warn!(
            child_agent = agent_name,
            parent_run_id,
            "dispatch_agent bypasses step `actions:` narrowing: the child inherits its OWN \
             agent's `tools:` grant verbatim, not the parent step's narrowed roster, and its \
             tool calls are not covered by the parent step's tool_audit trail (known \
             limitation — see docs/superpowers/specs/2026-07-26-rupu-step-actions-enforcement-design.md)"
        );

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

        // Provider/model resolution goes through the SHARED resolvers, the
        // same sequence `rupu run` (`cmd/run.rs`), `rupu session` and
        // `DefaultStepFactory` use. This used to hardcode
        // `anthropic`/`claude-sonnet-4-6`, so a dispatched sub-agent that
        // pinned neither ignored `default_provider`/`default_model` and could
        // never reach a config-declared openai-compatible provider — the same
        // agent resolved differently depending on how it was launched
        // (ISSUES.md I-8, the fourth I-1/I-2 call site).
        let provider_name = provider_factory::resolve_provider_name(
            spec.provider.as_deref(),
            self.default_provider.as_deref(),
        );
        let oai_params = self.openai_compatible.get(&provider_name).cloned();
        // Prefer the agent's pinned model; for an openai-compatible provider
        // fall back to its configured default_model.
        let model = provider_factory::resolve_model(
            spec.model.as_deref(),
            self.default_model.as_deref(),
            oai_params.as_ref().map(|p| p.default_model.as_str()),
        );
        let provider_config = provider_factory::ProviderConfig {
            anthropic_oauth_system_prefix: spec.anthropic_oauth_prefix,
            openai_compatible: oai_params,
            tuning: self.provider_tuning.get(&provider_name).cloned(),
        };
        let provider = match provider_factory::build_for_provider_with_config(
            &provider_name,
            &model,
            spec.auth,
            self.resolver.as_ref(),
            &provider_config,
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

        write_delegation_narrowing_notice(&transcript_path, agent_name, parent_run_id);

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

/// Append a transcript-visible notice to the CHILD's OWN transcript
/// recording that its tool calls are not covered by the parent step's
/// `actions:` narrowing or `tool_audit` trail (IMPORTANT 4's fallback —
/// see the doc comment on `dispatch()`). Written AFTER `run_agent`
/// returns (appending before it would be wiped: `run_agent` truncates
/// `transcript_path` via `JsonlWriter::create` as its very first step).
///
/// Deliberately reuses the plain `ToolCall`/`ToolResult` shapes (already
/// rendered by the CP transcript panel with zero new frontend code)
/// rather than overloading `Event::ToolAudit`'s `declared`/`granted`/
/// `blocked`/`restricted` fields — this call site has no way to know
/// whether the parent step was actually narrowed, so inventing values
/// for those fields would itself be a false record in an audit trail
/// that must never lie. No `error:` set either: this is a known
/// architectural limitation, not necessarily evidence anything went
/// wrong on this particular call, so it renders as a neutral note, not
/// an alarm.
///
/// Best-effort: a write failure is logged and swallowed, same as every
/// other observability side-channel in this arc — never allowed to
/// fail the dispatch it's annotating.
fn write_delegation_narrowing_notice(transcript_path: &Path, child_agent: &str, parent_run_id: &str) {
    let call_id = format!("delegation_narrowing_notice_{child_agent}");
    let note = format!(
        "KNOWN LIMITATION: this child agent (`{child_agent}`, dispatched from parent run \
         `{parent_run_id}`) was launched via `dispatch_agent` with its OWN agent's `tools:` \
         grant verbatim. If the parent workflow step declared a non-empty `actions:` \
         allowlist, that narrowing was NOT applied to this child, and none of the child's \
         own tool calls are covered by the parent step's `tool_audit` trail. See \
         docs/superpowers/specs/2026-07-26-rupu-step-actions-enforcement-design.md."
    );
    match JsonlWriter::append(transcript_path) {
        Ok(mut w) => {
            let write_result = w
                .write(&TxEvent::ToolCall {
                    call_id: call_id.clone(),
                    tool: "dispatch_agent_narrowing_notice".to_string(),
                    input: serde_json::json!({ "child_agent": child_agent, "parent_run_id": parent_run_id }),
                })
                .and_then(|_| {
                    w.write(&TxEvent::ToolResult {
                        call_id,
                        output: note,
                        error: None,
                        duration_ms: 0,
                        structured: None,
                    })
                });
            if let Err(e) = write_result {
                tracing::warn!(error = %e, "failed to write delegation-narrowing notice");
            } else if let Err(e) = w.flush() {
                tracing::warn!(error = %e, "failed to flush delegation-narrowing notice");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open child transcript for delegation-narrowing notice");
        }
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
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
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
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
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

    /// Regression for ISSUES.md I-8: `dispatch()` was the FOURTH I-1/I-2
    /// call site and hardcoded `anthropic` / `claude-sonnet-4-6`, ignoring
    /// `default_provider` / `default_model` from `config.toml` entirely (the
    /// I-1/I-2 fix only covered `cmd/run.rs`, `cmd/session.rs` and
    /// `step_factory.rs`). The child agent here declares NEITHER `provider:`
    /// nor `model:`, so the config-derived defaults threaded into the
    /// dispatcher must supply both.
    ///
    /// Observation seam: `run_agent` writes the resolved pair verbatim into
    /// the child's own transcript as `Event::RunStart { provider, model }`
    /// (`rupu-agent/src/runner.rs:739`), so the assertion reads real
    /// persisted output rather than any mock internals.
    #[tokio::test]
    async fn dispatch_honors_config_default_provider_and_model() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        // NOTE: no `provider:` and no `model:` in the frontmatter.
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nmaxTurns: 3\n---\nyou are a child agent.",
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
            Some("cfg-provider".to_string()),
            Some("cfg-model".to_string()),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
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

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let (provider, model) = events
            .iter()
            .find_map(|e| match e {
                Event::RunStart {
                    provider, model, ..
                } => Some((provider.clone(), model.clone())),
                _ => None,
            })
            .expect("child transcript must carry a RunStart");

        assert_eq!(
            model, "cfg-model",
            "dispatch must resolve the model through config's default_model, not the hardcoded fallback"
        );
        assert_eq!(
            provider, "cfg-provider",
            "dispatch must resolve the provider through config's default_provider, not the hardcoded fallback"
        );
    }

    /// Agent frontmatter still wins over the config defaults — the
    /// precedence `resolve_provider_name`/`resolve_model` encode.
    #[tokio::test]
    async fn dispatch_agent_frontmatter_overrides_config_defaults() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: pinned-provider\nmodel: pinned-model\nmaxTurns: 3\n---\nyou are a child agent.",
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
            Some("cfg-provider".to_string()),
            Some("cfg-model".to_string()),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
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

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let (provider, model) = events
            .iter()
            .find_map(|e| match e {
                Event::RunStart {
                    provider, model, ..
                } => Some((provider.clone(), model.clone())),
                _ => None,
            })
            .expect("child transcript must carry a RunStart");

        assert_eq!(model, "pinned-model");
        assert_eq!(provider, "pinned-provider");
    }

    /// A config-declared `kind = "openai-compatible"` provider must be
    /// reachable from a dispatched sub-agent: the params come from the
    /// threaded-in map, and its `default_model` is the last fallback before
    /// `FALLBACK_MODEL` when neither the agent nor `default_model` pins one.
    #[tokio::test]
    async fn dispatch_resolves_openai_compatible_provider_default_model() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: oracle\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let mut oai = std::collections::HashMap::new();
        oai.insert(
            "oracle".to_string(),
            provider_factory::OpenAiCompatibleParams {
                base_url: "https://example.invalid/v1".to_string(),
                default_model: "oracle-default".to_string(),
                stream: false,
                models: Vec::new(),
            },
        );

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
            None,
            None,
            oai,
            std::collections::HashMap::new(),
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

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let (provider, model) = events
            .iter()
            .find_map(|e| match e {
                Event::RunStart {
                    provider, model, ..
                } => Some((provider.clone(), model.clone())),
                _ => None,
            })
            .expect("child transcript must carry a RunStart");

        assert_eq!(provider, "oracle");
        assert_eq!(model, "oracle-default");
    }

    /// IMPORTANT 4 fallback: `dispatch()` cannot thread the parent step's
    /// `actions:` narrowing (or an audit callback) into the child launch
    /// (see the doc comment on `dispatch()` for why), so it must never be
    /// a SILENT bypass. Assert the child's own transcript carries a
    /// visible notice recording the limitation.
    #[tokio::test]
    async fn dispatch_leaves_a_visible_delegation_narrowing_notice_on_the_child_transcript() {
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
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
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

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let notice_call = events.iter().any(|e| {
            matches!(
                e,
                Event::ToolCall { tool, .. } if tool == "dispatch_agent_narrowing_notice"
            )
        });
        assert!(
            notice_call,
            "expected a dispatch_agent_narrowing_notice ToolCall on the child transcript; got {events:?}"
        );
        let notice_result = events.iter().any(|e| {
            matches!(
                e,
                Event::ToolResult { output, error, .. }
                    if error.is_none() && output.contains("KNOWN LIMITATION")
            )
        });
        assert!(
            notice_result,
            "expected the paired ToolResult carrying the limitation text; got {events:?}"
        );

        // The real final-answer extraction must be unaffected by the
        // notice (it only scans AssistantMessage events).
        assert_eq!(outcome.output, "child done");
    }
}
