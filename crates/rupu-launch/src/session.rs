//! Turn-bound durable orchestration. Each turn keeps its own run identity;
//! the session journal is the sole source of continuation ancestry. The
//! normal launch scheduler, child ledger, permission bounds and reaper do
//! the work. A normal end of a session turn checkpoints, rather than abandons,
//! its children. No autonomous work starts between session turns.
use async_trait::async_trait;
use fs2::FileExt;
use rupu_tools::{
    ChildHandle, ChildRequest, ChildStatus, JoinSet, LaunchError, Launcher, ParentEnd, RunIdentity,
    ToolContext, Until,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    sync::Arc,
    time::Duration,
};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    session_id: String,
    turns: Vec<String>,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Owns the lease until the turn and all its references have finished.
pub struct SessionLauncher {
    inner: Arc<dyn Launcher>,
    _lease: File,
}
impl SessionLauncher {
    /// Journal the current turn BEFORE resuming any children, so a crash
    /// during recovery is itself recoverable. Re-open on every turn/restart.
    /// Requires a session origin, matching scope and the actual turn directory.
    pub async fn open(
        dir: &Path,
        session_id: &str,
        ctx: &ToolContext,
        inner: Arc<dyn Launcher>,
    ) -> Result<Arc<Self>, LaunchError> {
        let err = |e: String| LaunchError::new("session_orchestration", e);
        if !safe_id(session_id)
            || !safe_id(&ctx.identity.run_id)
            || ctx.identity.surface != rupu_tools::Surface::Session
            || ctx.identity.scope_name.as_deref() != Some(session_id)
            || dir.file_name().and_then(|n| n.to_str()) != Some(session_id)
        {
            return Err(err("session orchestration identity mismatch".into()));
        }
        let lease = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("orchestration-worker.lock"))
            .map_err(|e| err(e.to_string()))?;
        lease
            .try_lock_exclusive()
            .map_err(|_| err("another turn owns this session's orchestration".into()))?;
        let mut journal: Journal = match fs::read(dir.join("orchestration-turns.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| err(e.to_string()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Journal {
                session_id: session_id.into(),
                turns: Vec::new(),
            },
            Err(e) => return Err(err(e.to_string())),
        };
        if journal.session_id != session_id
            || journal.turns.iter().any(|id| !safe_id(id))
            || journal.turns.contains(&ctx.identity.run_id)
        {
            return Err(err(
                "invalid or already used session orchestration ancestry".into(),
            ));
        }
        let turn_dir = ctx
            .identity
            .launch
            .children_dir
            .as_deref()
            .ok_or_else(|| err("session turn has no child directory".into()))?;
        if turn_dir.file_name().and_then(|n| n.to_str()) != Some(ctx.identity.run_id.as_str()) {
            return Err(err("session turn child directory mismatch".into()));
        }
        let runs = turn_dir
            .parent()
            .ok_or_else(|| err("session turn has no runs root".into()))?;
        for prior in &journal.turns {
            let owner = fs::read_to_string(runs.join(prior).join("session-orchestration-owner"))
                .map_err(|_| {
                    err("session journal references a turn without ownership evidence".into())
                })?;
            if owner != session_id {
                return Err(err(
                    "session journal references another session's turn".into()
                ));
            }
        }
        fs::create_dir_all(turn_dir).map_err(|e| err(e.to_string()))?;
        let owner_path = turn_dir.join("session-orchestration-owner");
        let mut owner = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(owner_path)
            .map_err(|e| err(format!("cannot claim turn ownership: {e}")))?;
        use std::io::Write;
        owner
            .write_all(session_id.as_bytes())
            .map_err(|e| err(e.to_string()))?;
        owner.sync_all().map_err(|e| err(e.to_string()))?;
        File::open(turn_dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| err(e.to_string()))?;
        if let Some(bus) = &ctx.services.message_bus {
            bus.persist_claims().map_err(|e| err(e.to_string()))?;
        }
        let prior = journal.turns.clone();
        journal.turns.push(ctx.identity.run_id.clone());
        rupu_runtime::session_orchestration::atomic_json(dir, "orchestration-turns.json", &journal)
            .map_err(|e| err(e.to_string()))?;
        let launcher = Arc::new(Self {
            inner,
            _lease: lease,
        });
        // Oldest first: each ledger's claim/continued_by entries prevent
        // duplicate dispatch after a crash halfway through this loop.
        for run in prior {
            launcher.inner.continue_children(&run, ctx).await;
        }
        Ok(launcher)
    }
}

#[async_trait]
impl Launcher for SessionLauncher {
    async fn check(&self, req: &ChildRequest, ctx: &ToolContext) -> Result<(), LaunchError> {
        self.inner.check(req, ctx).await
    }
    async fn launch(
        &self,
        req: ChildRequest,
        ctx: &ToolContext,
    ) -> Result<ChildHandle, LaunchError> {
        self.inner.launch(req, ctx).await
    }
    async fn join(
        &self,
        from: &RunIdentity,
        which: JoinSet,
        timeout: Duration,
        until: Until,
    ) -> Vec<ChildStatus> {
        self.inner.join(from, which, timeout, until).await
    }
    async fn finish(&self, from: &RunIdentity, _how: ParentEnd) {
        self.inner.finish(from, ParentEnd::Paused).await;
    }
    fn children(&self, from: &RunIdentity) -> Vec<ChildStatus> {
        self.inner.children(from)
    }
}

/// Called only after successful assembly and admission. Set the exact current
/// permission ceiling before continuation (ordinary tool calls do this in the
/// runner). Assembly's grant and the launcher's prior bounds still apply.
async fn attach(
    run: &mut rupu_runtime::assembly::AdmittedRun,
    dir: &Path,
    session_id: &str,
) -> Result<(), LaunchError> {
    let opts = run.options();
    let mut context = opts.tool_context.clone();
    let Some(inner) = context.services.launcher.clone() else {
        return Err(LaunchError::new(
            "session_orchestration",
            "launcher missing",
        ));
    };
    context.call.spawn_ceiling = Some(opts.permission.mode());
    let wrapper = SessionLauncher::open(dir, session_id, &context, inner).await?;
    run.replace_launcher(wrapper);
    Ok(())
}

/// A generated workflow remains dispatchable by its owning session on later
/// turns, but cannot be imported from another session or an Agentiflow run.
pub(crate) fn generated_by_session(global: &Path, id: &RunIdentity, file: &Path) -> bool {
    if id.surface != rupu_tools::Surface::Session {
        return false;
    }
    let Some(session) = id.scope_name.as_deref().filter(|id| safe_id(id)) else {
        return false;
    };
    let dir = global.join("sessions").join(session);
    let Ok(bytes) = fs::read(dir.join("orchestration-turns.json")) else {
        return false;
    };
    let Ok(journal) = serde_json::from_slice::<Journal>(&bytes) else {
        return false;
    };
    journal.session_id == session
        && journal.turns.contains(&id.run_id)
        && journal
            .turns
            .iter()
            .filter(|run| safe_id(run))
            .any(|run| crate::launcher::is_generated_file(&global.join("runs").join(run), file))
}

pub(crate) async fn check_continuation(
    launcher: &crate::RunLauncher,
    prior: &crate::ledger::Started,
    ctx: &ToolContext,
) -> Result<(), LaunchError> {
    let request = prior
        .request
        .as_ref()
        .ok_or_else(|| LaunchError::new("missing_request", "child has no recorded request"))?;
    let target = if request.generated {
        rupu_tools::ChildTarget::File(request.workflow.clone().ok_or_else(|| {
            LaunchError::new("missing_workflow", "generated child has no workflow")
        })?)
    } else {
        rupu_tools::ChildTarget::Name(prior.name.clone())
    };
    launcher
        .check(
            &ChildRequest {
                kind: prior.kind,
                target,
                prompt: request.prompt.clone(),
                inputs: request
                    .inputs
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            },
            ctx,
        )
        .await
}

/// Run an assembled session turn, recording a preflight failure through the
/// normal session finalization path if durable ownership cannot be acquired.
pub async fn run(
    run: rupu_runtime::assembly::AssembledRun,
    dir: &Path,
    session_id: &str,
) -> rupu_agent::RunExit {
    let mut run = run.admit().await;
    if let Err(error) = attach(&mut run, dir, session_id).await {
        let opts = run.options();
        let id = opts.identity();
        let error = rupu_agent::record_unstarted_run(
            rupu_agent::UnstartedRun {
                launch_args: &[],
                transcript_path: &opts.transcript_path,
                run_id: &id.run_id,
                workspace_id: &opts.tool_context.workspace.id,
                agent: &id.agent,
                provider: &id.provider,
                model: &id.model,
                mode: opts.permission.mode(),
                codename: id.codename.as_deref(),
                customer: opts.tool_context.services.customer.as_deref(),
                system_prompt: Some(&opts.system_prompt),
                prompt: &opts.prompt,
                launch_skills: &[],
                launch_preload_skills: &[],
            },
            rupu_providers::ProviderError::Preflight(error.to_string()),
        );
        return rupu_agent::RunExit {
            result: Err(error),
            final_limits: opts.limits.clone(),
            messages: Vec::new(),
            loaded_skills: opts.prompt.loaded_skills.clone(),
            unseen_messages: opts.prompt.unseen_messages.clone(),
        };
    }
    run.run().await
}
