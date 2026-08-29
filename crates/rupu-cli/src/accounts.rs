//! Account-aware credential resolver construction.
//!
//! `rupu-auth` cannot read config (hexagonal rule 1) and `rupu-config`
//! knows nothing about credentials, so `rupu-cli` — which depends on
//! both — is the correct seam for turning `[providers.*]` into the
//! `AccountSpec` list the resolver needs.

use rupu_auth::{AccountSpec, KeychainResolver};

/// Every declared `[providers.<name>]` entry, plus every declared
/// `[scm.<name>]` entry whose `kind` resolves to a repo platform
/// (github/gitlab), as `AccountSpec`s.
///
/// The `[scm.*]` half closes an arc-level gap: `rupu_scm::Registry::discover`
/// calls `resolver.get(<account name>, ..)` for every declared SCM
/// account (see `rupu-scm`'s `registry.rs`), which is the first code in
/// the tree to do that for a non-vendor account name. Without a matching
/// `AccountSpec`, `resolve_provider_id` returns `None`, `get` falls
/// through to the api-key-only `get_named` path, and a named SCM
/// account's SSO credential is read once but never refreshed — the same
/// "`with_accounts` had zero callers" defect `accounts_sso_e2e.rs`
/// exists to prevent for LLM providers, reproduced for SCM accounts.
/// Nothing breaks silently today (a fresh `gh-work/sso` or a stored
/// `gh-work/api-key` still reads fine via `get_named`); it degrades only
/// once that SSO token nears expiry.
///
/// Mirrors `Registry::discover`'s own kind resolution exactly (`kind`
/// override, or the account name itself parsed as the vendor) so the two
/// never drift. Linear/Jira are deliberately excluded: `Registry::discover`
/// never treats them as multi-account (see its `tracker_accounts` field
/// doc) — they are always looked up under the bare vendor name
/// ("linear"/"jira"), which `resolve_provider_id`'s vendor-name fallback
/// already resolves with no `AccountSpec` needed.
pub fn account_specs(cfg: &rupu_config::Config) -> Vec<AccountSpec> {
    let providers = cfg.providers.keys().filter_map(|name| {
        rupu_runtime::provider_factory::resolve_kind(name, &cfg.providers)
            .map(|kind| AccountSpec::new(name.clone(), kind))
    });
    let scm = cfg.scm.platforms.iter().filter_map(|(name, platform_cfg)| {
        let kind = platform_cfg
            .kind
            .as_deref()
            .and_then(|k| k.parse::<rupu_scm::Platform>().ok())
            .or_else(|| name.parse::<rupu_scm::Platform>().ok())?;
        Some(AccountSpec::new(name.clone(), kind.as_str()))
    });
    providers.chain(scm).collect()
}

/// A `KeychainResolver` that knows the config's declared accounts, so a
/// named account resolves SSO and refreshes against its vendor rather
/// than falling through to the api-key-only `get_named` path.
pub fn resolver_for(cfg: &rupu_config::Config) -> KeychainResolver {
    KeychainResolver::new().with_accounts(account_specs(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_specs_covers_declared_accounts_with_their_kinds() {
        let mut cfg = rupu_config::Config::default();
        cfg.providers.insert(
            "anthropic-work".into(),
            rupu_config::ProviderConfig {
                kind: Some("anthropic".into()),
                ..Default::default()
            },
        );
        cfg.providers.insert(
            "oracle".into(),
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://h:1".into()),
                default_model: Some("m".into()),
                ..Default::default()
            },
        );
        let specs = account_specs(&cfg);
        let work = specs.iter().find(|s| s.name == "anthropic-work").unwrap();
        assert_eq!(work.kind, "anthropic");
        // openai-compatible accounts are declared too: they have no
        // ProviderId, so `resolve_provider_id` returns None for them and
        // `get` routes them to `get_named`, which is correct. Pin the
        // kind itself, not just presence — `AccountSpec.kind` is exactly
        // what `resolve_provider_id` inspects to decide "no ProviderId,
        // route to get_named", so an unpinned kind here would let that
        // routing decision silently drift.
        let oracle = specs.iter().find(|s| s.name == "oracle").unwrap();
        assert_eq!(oracle.kind, "openai-compatible");
    }

    /// An empty config must yield an empty list, so `resolver_for` is
    /// behaviorally identical to `KeychainResolver::new()` for users who
    /// have declared nothing.
    #[test]
    fn account_specs_is_empty_for_default_config() {
        assert!(account_specs(&rupu_config::Config::default()).is_empty());
    }

    /// The gap this function exists to close (arc progress ledger,
    /// Ruling 7): a declared `[scm.gh-work]` account must become an
    /// `AccountSpec` with `kind = "github"` so `resolve_provider_id`
    /// resolves it — covers both the `kind =` override form and the
    /// name-is-the-vendor fallback form, mirroring
    /// `Registry::discover`'s own two-branch resolution.
    #[test]
    fn account_specs_covers_declared_scm_accounts_with_their_resolved_kind() {
        let mut cfg = rupu_config::Config::default();
        cfg.scm.platforms.insert(
            "gh-work".into(),
            rupu_config::ScmPlatformConfig {
                kind: Some("github".into()),
                ..Default::default()
            },
        );
        cfg.scm.platforms.insert(
            "gitlab".into(),
            rupu_config::ScmPlatformConfig {
                base_url: Some("https://gitlab.example.com".into()),
                ..Default::default()
            },
        );
        let specs = account_specs(&cfg);
        let gh_work = specs.iter().find(|s| s.name == "gh-work").unwrap();
        assert_eq!(gh_work.kind, "github");
        let gitlab = specs.iter().find(|s| s.name == "gitlab").unwrap();
        assert_eq!(gitlab.kind, "gitlab");
    }

    /// A declared `[scm.jira]` (or `[scm.linear]`) table never becomes an
    /// `AccountSpec` — `Registry::discover` never treats those as
    /// multi-account, and `resolve_provider_id`'s vendor-name fallback
    /// already resolves the bare name with no `AccountSpec` needed. An
    /// entry here would be harmless but is not what `Registry::discover`
    /// does, so this pins the intentional exclusion.
    #[test]
    fn account_specs_excludes_tracker_only_scm_accounts() {
        let mut cfg = rupu_config::Config::default();
        cfg.scm.platforms.insert(
            "jira".into(),
            rupu_config::ScmPlatformConfig {
                base_url: Some("https://acme.atlassian.net".into()),
                ..Default::default()
            },
        );
        let specs = account_specs(&cfg);
        assert!(specs.iter().all(|s| s.name != "jira"));
    }

    /// A declared `[scm.typo-account]` with an unresolvable `kind` (or no
    /// `kind` and a name that isn't itself a vendor) is silently dropped
    /// here — same "unavailable configured default falls through" shape
    /// `Registry::discover` uses (it warns and skips rather than
    /// panicking or fabricating an entry).
    #[test]
    fn account_specs_drops_scm_accounts_with_unresolvable_kind() {
        let mut cfg = rupu_config::Config::default();
        cfg.scm.platforms.insert(
            "typo-account".into(),
            rupu_config::ScmPlatformConfig {
                kind: Some("gtihub".into()),
                ..Default::default()
            },
        );
        let specs = account_specs(&cfg);
        assert!(specs.iter().all(|s| s.name != "typo-account"));
    }
}
