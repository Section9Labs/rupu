//! A read-only projection of one successfully assembled turn.
//!
//! This is evidence of the supplied run, not a prediction of the next turn or
//! a connectivity check. Inspecting it never calls a provider, tool, prompter,
//! launcher, credential resolver, or external service.

use std::collections::BTreeMap;

use rupu_agent::AgentRunOpts;
use rupu_scm::Platform;
use rupu_tools::{Effect, PermissionMode};
use serde::{Deserialize, Serialize};

/// Owned wire representation suitable for storing beside a session record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReadiness {
    pub schema_version: u32,
    pub run_id: String,
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub observed_at: String,
    /// Always `assembled_turn`; the owning session adds its frozen revisions.
    pub applies: String,
    pub permission_mode: String,
    pub approval_channel_available: bool,
    pub tools: Vec<ToolReadiness>,
    pub skills: Vec<SkillReadiness>,
    pub connectors: Vec<ConnectorReadiness>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReadiness {
    pub name: String,
    pub effect: String,
    /// Present in an offered, narrowed, unavailable, or skipped grant entry.
    /// This includes ambient grants; it does not mean explicitly declared.
    pub selected: bool,
    /// Exactly membership in the assembled grant offered to the model.
    pub offered: bool,
    /// The resolver retains reasons for offered entries only.
    pub grant_reasons: Vec<String>,
    pub narrowed: bool,
    pub unavailable: bool,
    pub skipped: bool,
    pub required_services: Vec<String>,
    pub missing_services: Vec<String>,
    pub service_available: bool,
    /// Effect-level policy, independent of whether the tool was offered:
    /// allowed, denied_readonly, approval_required, approval_unavailable,
    /// or spawn_with_ceiling. It is not permission for a particular target.
    pub permission: String,
    /// The generic legacy policy allows Ask calls without a prompter.
    /// Strict session assembly rejects that mode before producing this DTO.
    pub permission_degraded: bool,
    pub child_ceiling: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorReadiness {
    pub platform: String,
    /// This inventory covers repository handles, not every vendor feature.
    pub capability: String,
    /// The SCM registry object exists, even if it holds no connector handles.
    pub registry_service_constructed: bool,
    /// Exact names of constructed repository handles, never credentials.
    pub known_accounts: Vec<String>,
    /// Always `unverified`: no target or operation was exercised here.
    pub target_access: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReadiness {
    pub qualified_name: String,
    pub reason: String,
    pub indexed: bool,
    pub preload: bool,
    pub model_loadable: bool,
    /// The baseline the assembled run compares subsequent skill edits against.
    /// Inspection does not reread the installed skill or rehash its files.
    pub baseline_stamp: String,
}

/// Inspect the actual grant, services, and permission policy of an assembled
/// run. No speculative service handles are substituted for absent services.
pub fn inspect(opts: &AgentRunOpts) -> RunReadiness {
    let catalog = &opts.grant.catalog;
    let services = opts.tool_context.services.provided(opts.concerns.is_some());
    // Include unselected builtins so the distinction between selection and
    // model exposure remains visible. An offered descriptor is authoritative
    // even when a caller supplied a smaller or extended test catalog.
    let mut descriptors: BTreeMap<_, _> = catalog
        .definitions()
        .map(|descriptor| (descriptor.name.clone(), descriptor))
        .collect();
    for entry in opts.grant.entries.values() {
        descriptors.insert(entry.canonical.clone(), entry.descriptor.clone());
    }
    let mode = opts.permission.mode();
    let approval_channel_available = opts.permission.prompter().is_some();
    let tools = descriptors
        .into_iter()
        .map(|(name, descriptor)| {
            let entry = opts.grant.entries.get(&name);
            let narrowed = opts.grant.narrowed.contains(&name);
            let unavailable = opts.grant.unavailable.iter().any(|u| u.tool == name);
            let skipped = opts.grant.skipped.iter().any(|u| u.tool == name);
            let missing_services: Vec<_> = descriptor
                .needs
                .iter()
                .copied()
                .filter(|service| !services.contains(*service))
                .map(|service| service.as_str().to_owned())
                .collect();
            // Do not use PermissionPolicy::decide: Ask may invoke a prompter.
            // Match the policy's effect-level branches without making a call
            // or consulting a run's mutable per-tool allow-always decisions.
            let permission = match descriptor.effect {
                Effect::Read | Effect::Record => "allowed",
                Effect::Spawn => "spawn_with_ceiling",
                Effect::Write | Effect::External => match mode {
                    PermissionMode::Readonly => "denied_readonly",
                    PermissionMode::Bypass => "allowed",
                    PermissionMode::Ask if approval_channel_available => "approval_required",
                    PermissionMode::Ask => "approval_unavailable",
                },
            };
            ToolReadiness {
                name: name.to_owned(),
                effect: descriptor.effect.as_str().to_owned(),
                selected: entry.is_some() || narrowed || unavailable || skipped,
                offered: entry.is_some(),
                grant_reasons: entry
                    .map(|entry| entry.reasons.iter().map(ToString::to_string).collect())
                    .unwrap_or_default(),
                narrowed,
                unavailable,
                skipped,
                required_services: descriptor
                    .needs
                    .iter()
                    .map(|service| service.as_str().to_owned())
                    .collect(),
                service_available: missing_services.is_empty(),
                missing_services,
                permission: permission.to_owned(),
                permission_degraded: mode == PermissionMode::Ask
                    && !approval_channel_available
                    && matches!(descriptor.effect, Effect::Write | Effect::External),
                child_ceiling: (descriptor.effect == Effect::Spawn)
                    .then(|| opts.permission.ceiling_for_child(None).as_str().to_owned()),
            }
        })
        .collect();
    let mut skills: Vec<_> = opts
        .tool_context
        .services
        .skills
        .iter()
        .flat_map(|resolved| resolved.entries())
        .map(|granted| SkillReadiness {
            qualified_name: format!("{}:{}", granted.skill.layer.as_str(), granted.name()),
            reason: granted.reason.as_str().to_owned(),
            indexed: granted.indexed,
            preload: granted.preload,
            model_loadable: granted.model_loadable(),
            baseline_stamp: granted.baseline_stamp.hex(),
        })
        .collect();
    skills.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    let registry = opts.tool_context.services.scm.as_deref();
    let connectors = [Platform::Github, Platform::Gitlab]
        .into_iter()
        .map(|platform| ConnectorReadiness {
            platform: platform.to_string(),
            capability: "repository".to_owned(),
            registry_service_constructed: registry.is_some(),
            known_accounts: registry
                .map(|registry| {
                    registry
                        .accounts_for(platform)
                        .into_iter()
                        .map(|account| account.to_string())
                        .collect()
                })
                .unwrap_or_default(),
            target_access: "unverified".to_owned(),
        })
        .collect();
    let mut limitations = vec![
        "This snapshot describes the assembled turn; later settings and service changes are not reflected."
            .to_owned(),
        "Offered tools still require an allowed call, valid inputs, and any target-specific checks."
            .to_owned(),
        "Constructed connector accounts do not verify authentication, target access, or operation permissions."
            .to_owned(),
        "Known accounts cover repository connectors only; issue-tracker and vendor extras capabilities are checked when called."
            .to_owned(),
        "Child target allowlists, depth, and workflow inputs remain enforced when a child is launched."
            .to_owned(),
        "Grant reasons are retained only for offered entries; narrowed, unavailable, and skipped states are preserved separately."
            .to_owned(),
    ];
    if mode == PermissionMode::Ask && !approval_channel_available {
        limitations.push(
            "No approval channel is attached. The generic legacy Ask policy degrades to allowing write and external calls; strict session assembly rejects this state."
                .to_owned(),
        );
    }
    let identity = opts.identity();
    RunReadiness {
        schema_version: 1,
        run_id: identity.run_id.clone(),
        agent: identity.agent.clone(),
        provider: identity.provider.clone(),
        model: identity.model.clone(),
        observed_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        applies: "assembled_turn".to_owned(),
        permission_mode: mode.as_str().to_owned(),
        approval_channel_available,
        tools,
        skills,
        connectors,
        limitations,
    }
}

#[cfg(test)]
#[path = "session_readiness_tests.rs"]
mod tests;
