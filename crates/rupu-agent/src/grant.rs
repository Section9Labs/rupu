//! A run's tool grant (W2), resolved once at assembly from the agent's
//! `tools:`, the step's `actions:`, the ambient grants and the services the
//! run's [`ToolContext`] provides. The run assembler
//! (`rupu_runtime::assembly`) is the one production caller; the agent loop
//! builds its registry from the result and nothing else.

use rupu_tools::{
    AliasScope, AmbientGrant, GrantError, GrantInputs, ResolvedGrant, ToolCatalog, ToolContext,
};

/// What a run's grant is resolved from.
pub struct RunGrantInputs<'a> {
    /// The agent's `tools:`; `None` = [`rupu_tools::DEFAULT_GRANT`].
    pub declared: Option<&'a [String]>,
    /// The workflow step's `actions:` (connector narrowing); empty outside
    /// a step.
    pub step_actions: &'a [String],
    /// The connector cap a parent passed down (W7); `None` = no cap.
    pub connector_cap: Option<&'a [String]>,
    /// The run has a `concerns:` block (ambient `coverage.*`).
    pub concerns: bool,
    /// The grant the run's origin carries (an agentiflow lead's or unit's
    /// coordination set, `origin:flow_lead` / `origin:flow_unit`).
    pub origin: Option<&'a AmbientGrant>,
    /// The run's identity, workspace and services.
    pub tool_context: &'a ToolContext,
    pub alias_scope: AliasScope,
}

/// Resolve a run's grant (W2): the agent's `tools:`, the step's `actions:`,
/// the parent's connector cap, the ambient grants (`concerns:`, an
/// engagement, skills, the origin's grant) and the run's services.
pub fn resolve_run_grant(inputs: RunGrantInputs<'_>) -> Result<ResolvedGrant, GrantError> {
    let ctx = inputs.tool_context;
    let catalog = ToolCatalog::builtin();
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
    if ctx
        .services
        .skills
        .as_ref()
        .is_some_and(|s| s.has_loadable())
    {
        ambient.push(AmbientGrant::skills());
    }
    if let Some(origin) = inputs.origin {
        ambient.push(origin.clone());
    }
    let grant = catalog.resolve_grant(GrantInputs {
        declared: inputs.declared,
        step_actions: inputs.step_actions,
        connector_cap: inputs.connector_cap,
        ambient: &ambient,
        // `concerns` stands in for the concern catalog the loop flattens later.
        available: &ctx.services.provided(inputs.concerns),
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
        connector_cap: None,
        concerns: opts.concerns.is_some(),
        origin: None,
        tool_context: &opts.tool_context,
        alias_scope: opts.alias_scope,
    })?;
    Ok(opts)
}
