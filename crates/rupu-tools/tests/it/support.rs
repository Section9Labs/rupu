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
