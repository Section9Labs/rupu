//! `InProcessDispatcher` — the [`AgentDispatcher`] the `dispatch_agent` /
//! `dispatch_agents_parallel` tools call into (moved out of `rupu-cli`, P9;
//! W7 renames it and moves it to `rupu-launch`).
//!
//! Spawns a child agent run in-process and synchronously: loads the agent,
//! allocates a sub-run under the parent's run directory, builds the child's
//! [`LaunchSpec`] (`Origin::SubAgent`) and runs it through the
//! [`RunAssembler`] — so a child gets `[bash]`, its parent's findings scope,
//! the root's usage ledger and an admission slot exactly as every other run
//! does — then reads the final assistant text out of the child's transcript.
//!
//! The run store and the parent's live-event sink live in
//! `rupu-orchestrator`, above this crate: they reach the dispatcher through
//! the [`SubRunStore`] and [`DispatchEvents`] ports.
//!
//! See `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use rupu_agent::{StreamOpts, UserTurn};
use rupu_tools::{AgentDispatcher, DispatchError, DispatchOutcome, RunIdentity, SpawnPermission};
use rupu_transcript::{Event as TxEvent, JsonlReader, JsonlWriter};

use crate::assembly::{
    AssembleError, LaunchServices, LaunchSpec, Origin, Overrides, ParentRun, RunAssembler,
    WorkspaceBinding,
};
use crate::usage_ledger::UsageLedger;

/// Allocates a child's sub-run directory and transcript under its parent's
/// run (`RunStore::create_sub_run`).
pub trait SubRunStore: Send + Sync {
    /// `(sub_run_id, transcript_path)` for a child of `parent_run_id`.
    fn create_sub_run(
        &self,
        parent_run_id: &str,
        agent: &str,
    ) -> std::io::Result<(String, PathBuf)>;
}

/// A child the dispatcher started, as the parent's live view shows it.
#[derive(Debug, Clone)]
pub struct DispatchedChild {
    pub sub_run_id: String,
    pub agent: String,
    pub transcript_path: PathBuf,
    pub codename: Option<String>,
    pub provider: String,
    pub model: String,
}

/// How a child ended, for the parent's live view.
#[derive(Debug, Clone)]
pub struct DispatchDone {
    pub sub_run_id: String,
    pub success: bool,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// The child's typed failure, when it failed on a classified outcome.
    pub cause: Option<rupu_transcript::OutcomeRecord>,
}

/// The parent run's live events (`DispatchStarted` / `DispatchCompleted` on
/// the workflow event sink). Best-effort: it never fails a dispatch.
pub trait DispatchEvents: Send + Sync {
    fn started(&self, parent_run_id: &str, child: &DispatchedChild);
    fn completed(&self, parent_run_id: &str, done: &DispatchDone);
}

/// What every child of one root run shares: the workspace, the root's usage
/// ledger and the root `rupu run`'s coverage stream.
#[derive(Clone, Debug, Default)]
pub struct DispatchRoot {
    pub workspace: WorkspaceBinding,
    pub usage: Option<UsageLedger>,
    pub coverage_stream: Option<PathBuf>,
}

/// The in-process dispatcher one root run (a `rupu run`, a workflow run, a
/// session turn) hands its runs. Children reuse it, so grandchildren up to
/// `MAX_DEPTH` dispatch the same way.
pub struct InProcessDispatcher {
    assembler: Arc<RunAssembler>,
    sub_runs: Arc<dyn SubRunStore>,
    root: DispatchRoot,
    events: Option<Arc<dyn DispatchEvents>>,
    /// The run's codename namer, shared with the orchestrator's `RunNaming`
    /// (see [`Self::set_namer`]) so sub-agent role words and `#n` counters
    /// come from — and persist to — the same `codenames.json` as the run's
    /// static slots. `None` until installed; [`Self::namer_for`] then falls
    /// back to an in-memory namer.
    namer: Mutex<Option<rupu_codename::SharedNamer>>,
    /// Self-reference as a trait object, so each child's tool context carries
    /// this dispatcher — without it grandchildren could not dispatch.
    self_dyn: OnceLock<Arc<dyn AgentDispatcher>>,
}

impl std::fmt::Debug for InProcessDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcessDispatcher")
            .field("root", &self.root)
            .finish()
    }
}

impl InProcessDispatcher {
    pub fn new(
        assembler: Arc<RunAssembler>,
        sub_runs: Arc<dyn SubRunStore>,
        root: DispatchRoot,
        events: Option<Arc<dyn DispatchEvents>>,
    ) -> Arc<Self> {
        let arc = Arc::new(Self {
            assembler,
            sub_runs,
            root,
            events,
            namer: Mutex::new(None),
            self_dyn: OnceLock::new(),
        });
        let dyn_arc: Arc<dyn AgentDispatcher> = arc.clone();
        let _ = arc.self_dyn.set(dyn_arc);
        arc
    }

    /// Install the run's codename namer (`RunNaming::namer()`), so
    /// sub-agents are named from the same allocator + counters as the run's
    /// static slots.
    pub fn set_namer(&self, namer: rupu_codename::SharedNamer) {
        if let Ok(mut g) = self.namer.lock() {
            *g = Some(namer);
        }
    }

    /// The run's namer, or — when none was installed — an in-memory one for
    /// the parent's crew, so sub-agents are always named.
    fn namer_for(&self, parent: &rupu_codename::Codename) -> Option<rupu_codename::SharedNamer> {
        let mut g = self.namer.lock().ok()?;
        Some(
            g.get_or_insert_with(|| {
                rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new(
                    parent.crew.clone(),
                ))
            })
            .clone(),
        )
    }

    fn self_arc_dyn(&self) -> Arc<dyn AgentDispatcher> {
        self.self_dyn
            .get()
            .expect("InProcessDispatcher::new always populates self_dyn")
            .clone()
    }

    fn emit_completed(&self, parent_run_id: &str, done: DispatchDone) {
        if let Some(events) = &self.events {
            events.completed(parent_run_id, &done);
        }
    }
}

#[async_trait]
impl AgentDispatcher for InProcessDispatcher {
    async fn dispatch(
        &self,
        agent_name: &str,
        prompt: String,
        parent: &RunIdentity,
        permission: SpawnPermission,
    ) -> Result<DispatchOutcome, DispatchError> {
        // KNOWN LIMITATION (tool_audit design §4; W7 closes it): a step
        // narrowed by `actions:` whose agent keeps `dispatch_agent` runs its
        // CHILD with the child's own `tools:` grant, and the child's calls
        // are not in the parent step's `tool_audit` trail. Never silent:
        // warn on every dispatch, and leave a transcript-visible notice on
        // the child's own run (below).
        tracing::warn!(
            child_agent = agent_name,
            parent_run_id = %parent.run_id,
            "dispatch_agent bypasses step `actions:` narrowing: the child inherits its OWN \
             agent's `tools:` grant verbatim, not the parent step's narrowed roster, and its \
             tool calls are not covered by the parent step's tool_audit trail (known \
             limitation — see docs/superpowers/specs/2026-07-26-rupu-step-actions-enforcement-design.md)"
        );

        let ctx = self.assembler.context();
        let project_agents_parent = ctx.project_root.as_ref().map(|p| p.join(".rupu"));
        // Admission-paced (see `rupu_agent::fd_budget`): a parallel dispatch
        // near the open-file limit grows it or waits instead of failing.
        let spec = rupu_agent::load_agent_admitted(
            &ctx.global,
            project_agents_parent.as_deref(),
            agent_name,
        )
        .await
        .map_err(|e| match e {
            rupu_agent::AgentLoadError::NotFound(_) => DispatchError::AgentNotFound {
                agent: agent_name.to_string(),
            },
            // Surface the real cause (e.g. fd exhaustion, bad frontmatter)
            // instead of a misleading "not found".
            other => DispatchError::Io(std::io::Error::other(format!(
                "load agent `{agent_name}`: {other}"
            ))),
        })?;

        let (sub_run_id, transcript_path) = self
            .sub_runs
            .create_sub_run(&parent.run_id, agent_name)
            .map_err(|e| DispatchError::RunStore(e.to_string()))?;

        // `<parent>><role>#n`, from the run's shared namer. A parent with
        // no (or an unparseable) codename leaves the child unnamed.
        let codename = parent.codename.as_deref().and_then(|p| {
            let pc: rupu_codename::Codename = p.parse().ok()?;
            let namer = self.namer_for(&pc)?;
            child_codename(&namer, p, agent_name)
        });

        let launch = LaunchSpec {
            agent: spec,
            origin: Origin::SubAgent {
                parent: ParentRun {
                    identity: Arc::new(parent.clone()),
                    usage: self.root.usage.clone(),
                    coverage_stream: self.root.coverage_stream.clone(),
                },
            },
            run_id: sub_run_id.clone(),
            transcript_path: transcript_path.clone(),
            codename: codename.clone(),
            prompt: UserTurn::new(prompt),
            workspace: self.root.workspace.clone(),
            // The child's ceiling (D7): the parent's mode as the call's
            // permission decision set it, lowered — never raised — by the
            // child's own `permissionMode`; an interactive parent's operator
            // answers the child's prompts.
            mode: permission.ceiling,
            ceiling: Some(permission.ceiling),
            prompter: permission.prompter.clone(),
            overrides: Overrides::default(),
            // The parent's printer renders the child as a callout from the
            // `dispatch_agent` tool result.
            stream: StreamOpts {
                suppress_stdout: true,
                ..StreamOpts::default()
            },
            hooks: Default::default(),
            pause: None,
            services: LaunchServices {
                dispatcher: Some(self.self_arc_dyn()),
                netflow: None,
                extra_tools: Vec::new(),
                provider: None,
            },
            collectors: Vec::new(),
        };
        let (provider, model) = self
            .assembler
            .resolve_provider_model(&launch.agent, &launch.overrides);
        let assembled = self.assembler.assemble(launch).await;

        // Emitted once the child is assembled (or failed to be), so it can
        // carry who (codename) runs on what (provider · model). Emitted on
        // the failure path too, so the live view still shows the failed
        // child node that the `DispatchCompleted` below closes.
        if let Some(events) = &self.events {
            events.started(
                &parent.run_id,
                &DispatchedChild {
                    sub_run_id: sub_run_id.clone(),
                    agent: agent_name.to_string(),
                    transcript_path: transcript_path.clone(),
                    codename: codename.clone(),
                    provider,
                    model,
                },
            );
        }
        let failed = |sub_run_id: &str, cause| DispatchDone {
            sub_run_id: sub_run_id.to_string(),
            success: false,
            tokens_in: 0,
            tokens_out: 0,
            cause,
        };
        let assembled = match assembled {
            Ok(a) => a,
            Err(e) => {
                self.emit_completed(&parent.run_id, failed(&sub_run_id, None));
                return Err(match e {
                    AssembleError::UnknownProvider(_) | AssembleError::ProviderBuild { .. } => {
                        DispatchError::ProviderBuild(e.to_string())
                    }
                    AssembleError::Grant(_) => DispatchError::ChildRun(e.to_string()),
                });
            }
        };

        let started = std::time::Instant::now();
        let run_result = match assembled.run().await.result {
            Ok(r) => r,
            Err(e) => {
                self.emit_completed(&parent.run_id, failed(&sub_run_id, e.outcome().cloned()));
                return Err(DispatchError::ChildRun(e.to_string()));
            }
        };
        let duration_ms = started.elapsed().as_millis() as u64;

        write_delegation_narrowing_notice(&transcript_path, agent_name, &parent.run_id);

        // `Ok` does NOT mean the child did its job: `Err` is reserved for
        // failures that stopped the loop, so a child that burned its whole
        // `max_turns` budget comes back `Ok(RunResult { status: Error, .. })`.
        // A failed child is still an `Ok(DispatchOutcome)` (the parent gets
        // its sub-run id and transcript) with an honest `ok: false`, an empty
        // `output` (its last text is interim or cut off, spec 2026-10-01
        // §5.3), and `error` without the rung-3 hint: the parent model can't
        // `rupu run --continue` its child (`RunResult::failure_reason`).
        let terminal = run_result.terminal_error();
        if let Some(err) = &terminal {
            tracing::warn!(
                agent = %agent_name,
                sub_run_id = %sub_run_id,
                error = %err,
                "child agent run did not complete cleanly; reporting the dispatch as failed"
            );
        }
        let success = terminal.is_none();
        let output = if success {
            read_final_assistant_text(&transcript_path).unwrap_or_default()
        } else {
            String::new()
        };
        let error = run_result.failure_reason();

        self.emit_completed(
            &parent.run_id,
            DispatchDone {
                sub_run_id: sub_run_id.clone(),
                success,
                tokens_in: run_result.total_tokens_in,
                tokens_out: run_result.total_tokens_out,
                cause: terminal.as_ref().and_then(|e| e.outcome()).cloned(),
            },
        );

        Ok(DispatchOutcome {
            agent: agent_name.to_string(),
            sub_run_id,
            codename,
            transcript_path,
            output,
            success,
            error,
            tokens_used: run_result.total_tokens_in + run_result.total_tokens_out,
            duration_ms,
        })
    }
}

/// Mint a dispatched child's codename: `<parent>><role>#n`, where `role` is
/// the agent def's canonical word in the crew and `n` the next
/// per-(parent, role) instance. `None` when `parent` is not a codename.
pub fn child_codename(
    namer: &rupu_codename::SharedNamer,
    parent: &str,
    agent: &str,
) -> Option<String> {
    let parent: rupu_codename::Codename = parent.parse().ok()?;
    Some(namer.with(|n| {
        let role = n.canonical_role(agent);
        let k = n.next_instance(&parent, &role);
        parent.child(&role, Some(k)).to_string()
    }))
}

/// Append a transcript-visible notice to the CHILD's OWN transcript recording
/// that its tool calls are not covered by the parent step's `actions:`
/// narrowing or `tool_audit` trail. Written AFTER the run returns (the loop
/// truncates its transcript when it starts).
///
/// Deliberately reuses the plain `ToolCall`/`ToolResult` shapes (already
/// rendered by the CP transcript panel) rather than overloading
/// `Event::ToolAudit`: this call site cannot know whether the parent step was
/// actually narrowed, and inventing audit fields would be a false record.
///
/// Best-effort: a write failure is logged and swallowed, never allowed to
/// fail the dispatch it annotates.
fn write_delegation_narrowing_notice(
    transcript_path: &Path,
    child_agent: &str,
    parent_run_id: &str,
) {
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

/// The final turn's text of a persisted transcript
/// ([`rupu_transcript::final_turn_text`]): the child's `output`, by the same
/// rule a workflow step's output uses.
pub fn read_final_assistant_text(path: &Path) -> Option<String> {
    let iter = JsonlReader::iter(path).ok()?;
    rupu_transcript::final_turn_text(iter.flatten())
}

impl RunAssembler {
    /// The [`DispatchRoot`] a root run of `origin` hands its children: its
    /// workspace, the usage ledger its own [`crate::assembly::defaults_for`]
    /// row names, and its coverage stream.
    pub fn dispatch_root(
        &self,
        origin: &Origin,
        run_id: &str,
        workspace: WorkspaceBinding,
    ) -> DispatchRoot {
        let (usage, coverage_stream) = self.root_ledger_and_stream(origin, run_id);
        DispatchRoot {
            workspace,
            usage,
            coverage_stream,
        }
    }
}
