//! Account-aware credential resolver construction.
//!
//! `rupu-auth` cannot read config (hexagonal rule 1) and `rupu-config`
//! knows nothing about credentials, so `rupu-cli` — which depends on
//! both — is the correct seam for turning `[providers.*]` into the
//! `AccountSpec` list the resolver needs.

use rupu_auth::{AccountSpec, KeychainResolver};

/// Every declared `[providers.<name>]` entry as an `AccountSpec`.
pub fn account_specs(cfg: &rupu_config::Config) -> Vec<AccountSpec> {
    cfg.providers
        .keys()
        .filter_map(|name| {
            rupu_runtime::provider_factory::resolve_kind(name, &cfg.providers)
                .map(|kind| AccountSpec::new(name.clone(), kind))
        })
        .collect()
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
        // `get` routes them to `get_named`, which is correct.
        assert!(specs.iter().any(|s| s.name == "oracle"));
    }

    /// An empty config must yield an empty list, so `resolver_for` is
    /// behaviorally identical to `KeychainResolver::new()` for users who
    /// have declared nothing.
    #[test]
    fn account_specs_is_empty_for_default_config() {
        assert!(account_specs(&rupu_config::Config::default()).is_empty());
    }
}
