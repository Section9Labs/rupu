//! The accounts a run will authenticate as — the launch preview's
//! "this run will use ..." list. Pure: the caller resolves the config (with
//! its provenance) and the SCM account; this module only decides which
//! provider and fallback accounts each agent resolves to, and says where
//! each choice came from. It uses the same rules a launch does
//! ([`crate::provider_factory::resolve_provider_name`] and
//! [`rupu_config::RecoveryConfig::chain_for`] with the `recovery_opts` rule
//! that an unnamed fallback entry stays on the run's own provider), so the
//! preview cannot disagree with the run.

use std::collections::BTreeMap;

use rupu_config::{Config, FallbackEntry, KeyProvenance, KeySource};
use serde::Serialize;

use crate::provider_factory::resolve_provider_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRole {
    Provider,
    Fallback,
    Scm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestEntry {
    pub role: AccountRole,
    pub account: String,
    /// The vendor the account authenticates against (`anthropic`, `openai`,
    /// `github`, ...), when known.
    pub kind: Option<String>,
    /// Which agent(s) use it (provider/fallback); empty for scm.
    pub agents: Vec<String>,
    /// Human-readable origin: "agent frontmatter", "customer default",
    /// "global default", "customer [recovery].fallbacks", "rule owner =
    /// acme-corp", ...
    pub source: String,
}

/// What a run needs, per agent: its own `provider:` and `fallbacks:`.
pub struct AgentFacts<'a> {
    pub name: &'a str,
    pub provider: Option<&'a str>,
    pub fallbacks: Option<&'a [FallbackEntry]>,
}

fn layer_label(source: KeySource) -> &'static str {
    match source {
        KeySource::Global => "global",
        KeySource::Customer => "customer",
        KeySource::Project => "project",
        KeySource::Default => "built-in",
    }
}

/// `"<layer> <what>"`, suffixed " · locked" when a policy lock pins the key.
fn origin(
    provenance: &BTreeMap<String, KeyProvenance>,
    key: &str,
    what: &str,
    fallback_layer: &str,
) -> String {
    match provenance.get(key) {
        Some(p) => {
            let lock = if p.locked_by.is_some() {
                " · locked"
            } else {
                ""
            };
            format!("{} {what}{lock}", layer_label(p.source))
        }
        None => format!("{fallback_layer} {what}"),
    }
}

/// The vendor an account authenticates against: a declared
/// `[providers.<account>].kind`, else the account name when it is itself a
/// vendor name. `None` when neither says.
fn kind_of(cfg: &Config, account: &str) -> Option<String> {
    if let Some(kind) = cfg
        .providers
        .get(account)
        .and_then(|p| p.kind.as_deref())
        .filter(|k| !k.is_empty())
    {
        return Some(kind.to_string());
    }
    rupu_auth::ProviderId::from_vendor_str(account).map(|p| p.as_str().to_string())
}

fn push(
    out: &mut Vec<ManifestEntry>,
    role: AccountRole,
    account: String,
    kind: Option<String>,
    agent: &str,
    source: String,
) {
    if let Some(existing) = out
        .iter_mut()
        .find(|e| e.role == role && e.account == account)
    {
        if !existing.agents.iter().any(|a| a == agent) {
            existing.agents.push(agent.to_string());
        }
        // Agents can reach one account from different places (one names it,
        // another inherits it); keep every distinct origin.
        if !existing.source.split("; ").any(|s| s == source) {
            existing.source = format!("{}; {source}", existing.source);
        }
        return;
    }
    out.push(ManifestEntry {
        role,
        account,
        kind,
        agents: vec![agent.to_string()],
        source,
    });
}

/// Accounts a run will use. Deduplicates by (role, account), merging
/// `agents` and joining each distinct `source` with "; ". `scm` is appended as given.
pub fn credential_manifest(
    cfg: &Config,
    provenance: &BTreeMap<String, KeyProvenance>,
    agents: &[AgentFacts<'_>],
    scm: Option<ManifestEntry>,
) -> Vec<ManifestEntry> {
    let mut out: Vec<ManifestEntry> = Vec::new();
    for agent in agents {
        let provider = resolve_provider_name(agent.provider, cfg.default_provider.as_deref());
        let named = agent.provider.is_some_and(|p| !p.trim().is_empty());
        let source = if named {
            "agent frontmatter".to_string()
        } else {
            origin(provenance, "default_provider", "default", "built-in")
        };
        push(
            &mut out,
            AccountRole::Provider,
            provider.clone(),
            kind_of(cfg, &provider),
            agent.name,
            source,
        );

        let chain = cfg.recovery.chain_for(agent.fallbacks);
        let chain_source = if agent.fallbacks.is_some() {
            "agent frontmatter".to_string()
        } else {
            origin(
                provenance,
                "recovery.fallbacks",
                "[recovery].fallbacks",
                "global",
            )
        };
        for entry in chain {
            let account = entry
                .provider
                .as_deref()
                .filter(|p| !p.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| provider.clone());
            push(
                &mut out,
                AccountRole::Fallback,
                account.clone(),
                kind_of(cfg, &account),
                agent.name,
                chain_source.clone(),
            );
        }
    }
    out.extend(scm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_config::LockOwner;

    fn kp(source: KeySource, locked_by: Option<LockOwner>) -> KeyProvenance {
        KeyProvenance {
            source,
            locked: locked_by.is_some(),
            locked_by,
        }
    }

    fn cfg_default(provider: &str) -> Config {
        Config {
            default_provider: Some(provider.to_string()),
            ..Default::default()
        }
    }

    fn agent<'a>(name: &'a str, provider: Option<&'a str>) -> AgentFacts<'a> {
        AgentFacts {
            name,
            provider,
            fallbacks: None,
        }
    }

    #[test]
    fn an_agent_naming_its_provider_is_attributed_to_its_frontmatter() {
        let cfg = cfg_default("anthropic-acme");
        let prov = BTreeMap::from([(
            "default_provider".to_string(),
            kp(KeySource::Customer, None),
        )]);
        let m = credential_manifest(&cfg, &prov, &[agent("a", Some("openai"))], None);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].role, AccountRole::Provider);
        assert_eq!(m[0].account, "openai");
        assert_eq!(m[0].kind.as_deref(), Some("openai"));
        assert_eq!(m[0].source, "agent frontmatter");
        assert_eq!(m[0].agents, vec!["a"]);
    }

    #[test]
    fn an_agent_without_one_takes_the_default_and_says_when_it_is_locked() {
        let cfg = cfg_default("anthropic-acme");
        let prov = BTreeMap::from([(
            "default_provider".to_string(),
            kp(KeySource::Customer, Some(LockOwner::Customer)),
        )]);
        let m = credential_manifest(&cfg, &prov, &[agent("a", None)], None);
        assert_eq!(m[0].account, "anthropic-acme");
        assert_eq!(m[0].source, "customer default · locked");
        // Not a vendor name and not declared: the kind is unknown.
        assert_eq!(m[0].kind, None);

        let unlocked =
            BTreeMap::from([("default_provider".to_string(), kp(KeySource::Global, None))]);
        let m = credential_manifest(&cfg, &unlocked, &[agent("a", None)], None);
        assert_eq!(m[0].source, "global default");
    }

    #[test]
    fn a_declared_kind_names_the_vendor() {
        let mut cfg = cfg_default("anthropic-acme");
        cfg.providers.insert(
            "anthropic-acme".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("anthropic".to_string()),
                ..Default::default()
            },
        );
        let m = credential_manifest(&cfg, &BTreeMap::new(), &[agent("a", None)], None);
        assert_eq!(m[0].kind.as_deref(), Some("anthropic"));
        assert_eq!(m[0].source, "built-in default");
    }

    #[test]
    fn an_unnamed_fallback_stays_on_the_agents_provider() {
        let mut cfg = cfg_default("anthropic-acme");
        cfg.recovery.fallbacks = vec![
            FallbackEntry {
                provider: None,
                model: "m1".into(),
            },
            FallbackEntry {
                provider: Some("openai".into()),
                model: "m2".into(),
            },
        ];
        let prov = BTreeMap::from([(
            "recovery.fallbacks".to_string(),
            kp(KeySource::Customer, None),
        )]);
        let m = credential_manifest(&cfg, &prov, &[agent("a", None)], None);
        let fallbacks: Vec<_> = m
            .iter()
            .filter(|e| e.role == AccountRole::Fallback)
            .collect();
        assert_eq!(fallbacks.len(), 2);
        assert_eq!(fallbacks[0].account, "anthropic-acme");
        assert_eq!(fallbacks[1].account, "openai");
        assert_eq!(fallbacks[1].source, "customer [recovery].fallbacks");

        // An agent's own chain wins and is attributed to it.
        let own = [FallbackEntry {
            provider: None,
            model: "x".into(),
        }];
        let m = credential_manifest(
            &cfg,
            &prov,
            &[AgentFacts {
                name: "b",
                provider: Some("openai"),
                fallbacks: Some(&own),
            }],
            None,
        );
        let fb = m.iter().find(|e| e.role == AccountRole::Fallback).unwrap();
        assert_eq!(fb.account, "openai");
        assert_eq!(fb.source, "agent frontmatter");
    }

    #[test]
    fn two_agents_on_one_account_make_one_entry() {
        let cfg = cfg_default("anthropic");
        let m = credential_manifest(
            &cfg,
            &BTreeMap::new(),
            &[
                agent("a", None),
                agent("b", Some("anthropic")),
                agent("a", None),
                agent("c", Some("anthropic")),
            ],
            None,
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].agents, vec!["a", "b", "c"]);
        // Inherited by `a`, named by `b` and `c`: both origins, once each.
        assert_eq!(m[0].source, "built-in default; agent frontmatter");
    }

    #[test]
    fn the_scm_entry_is_appended() {
        let scm = ManifestEntry {
            role: AccountRole::Scm,
            account: "github-acme".into(),
            kind: Some("github".into()),
            agents: vec![],
            source: "rule owner = acme-corp".into(),
        };
        let m = credential_manifest(
            &cfg_default("anthropic"),
            &BTreeMap::new(),
            &[agent("a", None)],
            Some(scm.clone()),
        );
        assert_eq!(m.last(), Some(&scm));
        assert_eq!(m.len(), 2);
    }
}
