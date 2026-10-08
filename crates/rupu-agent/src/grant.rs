//! A run's tool grant (W2), resolved once at assembly from the agent's
//! `tools:`, the step's `actions:`, the ambient grants and the services the
//! run's [`ToolContext`] provides. The run assembler
//! (`rupu_runtime::assembly`) is the one production caller; the agent loop
//! builds its registry from the result and nothing else.

use crate::tool_registry::tool_catalog;
use rupu_tools::{
    AliasScope, AmbientGrant, GrantError, GrantInputs, ResolvedGrant, Service, ServiceSet, Tool,
    ToolContext,
};
use std::sync::Arc;

/// What a run's grant is resolved from.
pub struct RunGrantInputs<'a> {
    /// The agent's `tools:`; `None` = [`rupu_tools::DEFAULT_GRANT`].
    pub declared: Option<&'a [String]>,
    /// The workflow step's `actions:` (connector narrowing); empty outside
    /// a step.
    pub step_actions: &'a [String],
    /// The run has a `concerns:` block (ambient `coverage.*`).
    pub concerns: bool,
    /// Pre-built tools injected into the run (ambient `origin:injected`).
    pub injected: &'a [Arc<dyn Tool>],
    /// The run's identity, workspace and services.
    pub tool_context: &'a ToolContext,
    pub alias_scope: AliasScope,
}

/// The services a run provides, for its grant. Exact by construction: a
/// service is here only when the agent loop can build every tool needing it
/// from `ctx` (and `concerns`).
pub fn run_services(ctx: &ToolContext, concerns: bool) -> ServiceSet {
    let services = &ctx.services;
    // The findings ledger lives in the workspace: every run can open it.
    let mut s = ServiceSet::new().with(Service::Findings);
    if concerns {
        s.insert(Service::Coverage);
    }
    if services
        .findings
        .as_ref()
        .is_some_and(|f| f.engagement.is_some())
    {
        s.insert(Service::Engagement);
    }
    if services.scm.is_some() {
        s.insert(Service::Scm);
    }
    if services.dispatcher.is_some() {
        s.insert(Service::AgentDispatcher);
    }
    if services.netflow_sink.is_some() {
        s.insert(Service::Netflow);
    }
    // The agentiflow services (message bus, run status, catalog, the unit
    // launcher, the workflow generator) have no body in the catalog until
    // W5/W7: their tools reach a run only injected, as self-served grants.
    s
}

/// Resolve a run's grant (W2): the agent's `tools:`, the step's `actions:`,
/// the ambient grants (`concerns:`, an engagement, injected tools) and the
/// run's services.
pub fn resolve_run_grant(inputs: RunGrantInputs<'_>) -> Result<ResolvedGrant, GrantError> {
    let ctx = inputs.tool_context;
    let injected: Vec<_> = inputs.injected.iter().map(|t| t.descriptor()).collect();
    let catalog = tool_catalog().with(injected.iter().copied());
    let engagement = ctx
        .services
        .findings
        .as_ref()
        .is_some_and(|f| f.engagement.is_some());
    let mut ambient = Vec::new();
    if inputs.concerns {
        ambient.push(AmbientGrant::concerns());
    }
    if engagement {
        ambient.push(AmbientGrant::engagement());
    }
    if !injected.is_empty() {
        ambient.push(AmbientGrant::injected(
            injected.iter().map(|d| d.name).collect(),
        ));
    }
    let grant = catalog.resolve_grant(GrantInputs {
        declared: inputs.declared,
        step_actions: inputs.step_actions,
        ambient: &ambient,
        available: &run_services(ctx, inputs.concerns),
        alias_scope: inputs.alias_scope,
    })?;
    for tool in &grant.actions_not_granted {
        tracing::warn!(
            agent = %ctx.identity.agent,
            step = ctx.identity.step_id.as_deref().unwrap_or(""),
            tool,
            "step `actions:` names `{tool}` but the agent's `tools:` grant does not cover it; \
             the step can't add it (no escalation), so this is very likely an authoring mistake"
        );
    }
    Ok(grant)
}

/// `opts` with its grant resolved from `declared` and `step_actions` over
/// its own tool context and concerns — what the run assembler does, for a
/// caller that builds [`AgentRunOpts`] itself (the agent loop's tests).
/// Production launches go through `rupu_runtime::assembly`.
///
/// [`AgentRunOpts`]: crate::runner::AgentRunOpts
pub fn with_grant(
    mut opts: crate::runner::AgentRunOpts,
    declared: Option<&[String]>,
    step_actions: &[String],
) -> Result<crate::runner::AgentRunOpts, GrantError> {
    opts.grant = resolve_run_grant(RunGrantInputs {
        declared,
        step_actions,
        concerns: opts.concerns.is_some(),
        injected: &opts.extra_tools,
        tool_context: &opts.tool_context,
        alias_scope: opts.alias_scope,
    })?;
    Ok(opts)
}
