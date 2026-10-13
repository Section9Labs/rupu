//! Calling a catalog tool the way an `action:` step and `rupu mcp serve` do:
//! resolve the name (canonical or alias), build the body for the context,
//! decide and invoke through [`rupu_tools::call::call`].

use rupu_tools::call::{call, CallOutcome};
use rupu_tools::{
    AliasScope, AllowAlways, PermissionMode, PermissionPolicy, Surface, ToolCatalog, ToolContext,
};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// A context with the SCM registry `reg` and nothing else.
pub fn scm_ctx(reg: rupu_scm::Registry) -> ToolContext {
    let mut ctx = ToolContext::default();
    ctx.services.scm = Some(Arc::new(reg));
    ctx
}

/// A workflow-run context in `workspace`, findings scoped `scope`, recording
/// under `profile`.
pub fn findings_ctx(
    workspace: &Path,
    scope: &str,
    run_id: &str,
    model: &str,
    profile: rupu_coverage::FindingProfile,
    codename: Option<&str>,
    provider: Option<&str>,
) -> ToolContext {
    let mut ctx = ToolContext::in_workspace(workspace);
    let id = ctx.identity_mut();
    id.run_id = run_id.into();
    id.model = model.into();
    id.surface = Surface::Workflow;
    id.scope_name = Some(scope.into());
    id.codename = codename.map(str::to_string);
    id.provider = provider.unwrap_or_default().into();
    ctx.services.findings =
        Some(rupu_coverage::FindingWriteOptions::default().with_profile(profile));
    ctx.services.scm = Some(Arc::new(rupu_scm::Registry::default()));
    ctx
}

/// Calls tools in one context under one mode.
pub struct Caller {
    pub ctx: ToolContext,
    pub mode: PermissionMode,
}

impl Caller {
    /// Bypass mode.
    pub fn new(ctx: ToolContext) -> Self {
        Self {
            ctx,
            mode: PermissionMode::Bypass,
        }
    }

    pub fn with_mode(mut self, mode: PermissionMode) -> Self {
        self.mode = mode;
        self
    }

    /// The call's text, or its failure / denial text.
    pub async fn call(&self, name: &str, args: Value) -> Result<String, String> {
        into_result(self.outcome_in(&self.ctx, name, args).await)
    }

    /// [`Self::call`] recording findings under `profile` instead of the
    /// context's (an `action:` step's own `findings_profile`).
    pub async fn call_with_findings_profile(
        &self,
        name: &str,
        args: Value,
        profile: rupu_coverage::FindingProfile,
    ) -> Result<String, String> {
        let mut ctx = self.ctx.clone();
        ctx.services
            .findings
            .get_or_insert_with(Default::default)
            .profile = profile;
        into_result(self.outcome_in(&ctx, name, args).await)
    }

    pub async fn outcome(&self, name: &str, args: Value) -> CallOutcome {
        self.outcome_in(&self.ctx, name, args).await
    }

    async fn outcome_in(&self, ctx: &ToolContext, name: &str, args: Value) -> CallOutcome {
        let Some(d) = ToolCatalog::builtin().resolve_name(name, AliasScope::Everywhere) else {
            return CallOutcome::Failed(format!("unknown tool: {name}"));
        };
        let Some(tool) = rupu_tools::bodies::body(d.name, ctx) else {
            return CallOutcome::Failed(format!("{name} is unavailable in this context"));
        };
        call(
            &*tool,
            args,
            ctx,
            &PermissionPolicy::unattended(self.mode),
            &mut AllowAlways::default(),
        )
        .await
    }
}

fn into_result(o: CallOutcome) -> Result<String, String> {
    match o {
        CallOutcome::Done(s) => Ok(s),
        CallOutcome::Denied(m) | CallOutcome::Failed(m) => Err(m),
    }
}

/// A [`rupu_tools::RunStatus`] answering with fixed reports.
#[derive(Default)]
pub struct StubStatus {
    pub goals: rupu_tools::ports::GoalReport,
    pub coverage: Option<rupu_tools::ports::GoalCoverageReport>,
    pub budget: Option<rupu_tools::ports::BudgetReport>,
}

impl rupu_tools::RunStatus for StubStatus {
    fn goals(&self) -> Result<rupu_tools::ports::GoalReport, rupu_tools::PortError> {
        Ok(self.goals.clone())
    }
    fn coverage(&self) -> Result<rupu_tools::ports::GoalCoverageReport, rupu_tools::PortError> {
        self.coverage
            .clone()
            .ok_or_else(|| rupu_tools::PortError::new("could not evaluate coverage: stub"))
    }
    fn budget(&self) -> Result<rupu_tools::ports::BudgetReport, rupu_tools::PortError> {
        Ok(self
            .budget
            .clone()
            .unwrap_or(rupu_tools::ports::BudgetReport::NoBudget))
    }
}

/// A [`rupu_tools::CatalogPort`] over fixed entries; `agents_error` makes
/// the agent listing fail the way one unparseable agent file does.
#[derive(Default)]
pub struct StubCatalog {
    pub agents: Vec<rupu_tools::ports::AgentEntry>,
    pub workflows: Vec<rupu_tools::ports::WorkflowEntry>,
    pub agents_error: Option<String>,
}

impl rupu_tools::CatalogPort for StubCatalog {
    fn agents(&self) -> Result<Vec<rupu_tools::ports::AgentEntry>, rupu_tools::PortError> {
        match &self.agents_error {
            Some(e) => Err(rupu_tools::PortError::new(e.clone())),
            None => Ok(self.agents.clone()),
        }
    }
    fn workflows(&self) -> Vec<rupu_tools::ports::WorkflowEntry> {
        self.workflows.clone()
    }
}

/// A `ResolvedSkills` holding the global skills named in `grants`, all
/// installed under `<global>/skills/` by the caller.
pub fn resolved(global: &Path, grants: &[&str]) -> Arc<rupu_skills::ResolvedSkills> {
    let cat =
        rupu_skills::SkillCatalog::discover(&rupu_skills::LayerDirs::for_layers(global, None))
            .unwrap();
    let g: Vec<String> = grants.iter().map(|s| s.to_string()).collect();
    Arc::new(
        rupu_skills::resolve_run_skills(
            &cat,
            &[rupu_skills::SkillRequest {
                reason: rupu_skills::SkillReason::Agent,
                grants: &g,
                preload: &[],
            }],
        )
        .unwrap(),
    )
}

/// A [`rupu_tools::Launcher`] whose children are scripted: every launch is
/// recorded and gets the next handle; `join` answers each handle with the
/// status the test scripted for its name (`Done` with `"<name> done"` when
/// none was), and `check` refuses the names in `refuse`.
#[derive(Default)]
pub struct ScriptedLauncher {
    pub launched: std::sync::Mutex<Vec<rupu_tools::ChildRequest>>,
    pub statuses: std::collections::BTreeMap<String, rupu_tools::ChildStatus>,
    pub refuse: Vec<String>,
    pub address: Option<String>,
}

impl ScriptedLauncher {
    /// A finished child of `name`, as a sub-agent run reports it.
    pub fn done(name: &str, output: &str, success: bool) -> rupu_tools::ChildStatus {
        rupu_tools::ChildStatus {
            handle: String::new(),
            kind: rupu_tools::ChildKind::Subagent,
            name: name.into(),
            run_id: "sub_01GOLDEN".into(),
            codename: Some("cobalt-harbor/heron#1>lynx#1".into()),
            address: None,
            transcript: Some("/runs/run_P/sub/sub_01GOLDEN/transcript.jsonl".into()),
            state: rupu_tools::ChildState::Done,
            success: Some(success),
            output: Some(output.into()),
            error: (!success).then(|| "refused · no fallback left".to_string()),
            cause: None,
            tokens: Some(rupu_tools::ChildTokens {
                input: 40,
                output: 2,
            }),
            duration_ms: Some(7),
            elapsed: std::time::Duration::ZERO,
        }
    }

    fn handle_of(i: usize) -> String {
        format!("ch_{i}")
    }

    fn name_of(req: &rupu_tools::ChildRequest) -> String {
        req.target.label()
    }
}

#[async_trait::async_trait]
impl rupu_tools::Launcher for ScriptedLauncher {
    async fn check(
        &self,
        req: &rupu_tools::ChildRequest,
        _from: &ToolContext,
    ) -> Result<(), rupu_tools::LaunchError> {
        let name = Self::name_of(req);
        if self.refuse.contains(&name) {
            return Err(rupu_tools::LaunchError::new(
                "agent_not_dispatchable",
                format!("`{name}` is not dispatchable"),
            ));
        }
        Ok(())
    }

    async fn launch(
        &self,
        req: rupu_tools::ChildRequest,
        from: &ToolContext,
    ) -> Result<rupu_tools::ChildHandle, rupu_tools::LaunchError> {
        self.check(&req, from).await?;
        let mut launched = self.launched.lock().unwrap();
        launched.push(req.clone());
        Ok(rupu_tools::ChildHandle {
            handle: Self::handle_of(launched.len() - 1),
            kind: req.kind,
            name: Self::name_of(&req),
            run_id: format!("run_{}", launched.len()),
            codename: None,
            address: self.address.clone(),
            transcript: None,
        })
    }

    async fn join(
        &self,
        _from: &rupu_tools::RunIdentity,
        which: rupu_tools::JoinSet,
        _timeout: std::time::Duration,
        _until: rupu_tools::Until,
    ) -> Vec<rupu_tools::ChildStatus> {
        let launched = self.launched.lock().unwrap().clone();
        let handles: Vec<String> = match which {
            rupu_tools::JoinSet::All => (0..launched.len()).map(Self::handle_of).collect(),
            rupu_tools::JoinSet::Handles(h) => h,
        };
        handles
            .into_iter()
            .map(|h| {
                let i: usize = h.trim_start_matches("ch_").parse().unwrap_or(usize::MAX);
                let Some(req) = launched.get(i) else {
                    return rupu_tools::ChildStatus {
                        handle: h,
                        state: rupu_tools::ChildState::Failed,
                        error: Some("unknown handle".into()),
                        ..Self::done("?", "", false)
                    };
                };
                let name = Self::name_of(req);
                let mut s = self
                    .statuses
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| Self::done(&name, &format!("{name} done"), true));
                s.handle = h;
                s.kind = req.kind;
                s
            })
            .collect()
    }

    async fn finish(&self, _from: &rupu_tools::RunIdentity, _how: rupu_tools::ParentEnd) {}

    fn children(&self, _from: &rupu_tools::RunIdentity) -> Vec<rupu_tools::ChildStatus> {
        Vec::new()
    }
}

/// A [`rupu_tools::WorkflowGenerator`] that "saves" one fixed workflow.
pub struct StubGenerator;

#[async_trait::async_trait]
impl rupu_tools::WorkflowGenerator for StubGenerator {
    async fn generate(
        &self,
        _description: &str,
        _from: &ToolContext,
    ) -> Result<rupu_tools::GeneratedWorkflow, rupu_tools::LaunchError> {
        Ok(rupu_tools::GeneratedWorkflow {
            file: "/runs/run_P/generated/triage-01.yaml".into(),
            name: "triage".into(),
            steps: 2,
            input_keys: vec!["target".into()],
        })
    }
}
