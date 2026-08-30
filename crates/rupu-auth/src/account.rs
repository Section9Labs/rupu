//! Declared account identities.
//!
//! An **account** is an identity (whose token this is). A **kind** is a
//! vendor (which client to build). Before multi-account support these
//! were the same string, which is why only one credential per vendor
//! could exist.
//!
//! This module owns `AccountSpec` rather than `rupu-config` because
//! `rupu-auth` must not depend on `rupu-config` (hexagonal rule 1). The
//! CLI reads config and hands the list down.

use crate::backend::ProviderId;

/// One declared account: its name, and the vendor it authenticates against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSpec {
    pub name: String,
    pub kind: String,
}

impl AccountSpec {
    pub fn new(name: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
        }
    }

    /// The vendor this account authenticates against, if `kind` names a
    /// built-in. `None` for `openai-compatible` and for unknown kinds.
    pub fn provider_id(&self) -> Option<ProviderId> {
        ProviderId::from_vendor_str(&self.kind)
    }
}

/// Resolve a provider string to its vendor.
///
/// A declared account wins. Otherwise the name is tried as a vendor name
/// directly — the back-compat rule from the design spec §3.1, which is
/// what keeps a pre-existing `anthropic/api-key` working with no config
/// at all.
pub fn resolve_provider_id(name: &str, accounts: &[AccountSpec]) -> Option<ProviderId> {
    if let Some(a) = accounts.iter().find(|a| a.name == name) {
        return a.provider_id();
    }
    ProviderId::from_vendor_str(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_account_resolves_to_its_kind() {
        let accounts = vec![
            AccountSpec::new("anthropic-work", "anthropic"),
            AccountSpec::new("anthropic-personal", "anthropic"),
        ];
        assert_eq!(
            resolve_provider_id("anthropic-work", &accounts),
            Some(ProviderId::Anthropic)
        );
        assert_eq!(
            resolve_provider_id("anthropic-personal", &accounts),
            Some(ProviderId::Anthropic)
        );
    }

    /// Spec §3.1. A user who never declared anything must keep working.
    #[test]
    fn bare_vendor_name_resolves_with_no_declared_accounts() {
        assert_eq!(
            resolve_provider_id("anthropic", &[]),
            Some(ProviderId::Anthropic)
        );
        assert_eq!(resolve_provider_id("github", &[]), Some(ProviderId::Github));
    }

    #[test]
    fn undeclared_non_vendor_name_does_not_resolve() {
        assert_eq!(resolve_provider_id("anthropic-work", &[]), None);
    }

    /// A declared account may shadow a bare vendor name, and its own
    /// `kind` is what counts.
    #[test]
    fn declared_account_named_after_a_vendor_uses_its_kind() {
        let accounts = vec![AccountSpec::new("anthropic", "anthropic")];
        assert_eq!(
            resolve_provider_id("anthropic", &accounts),
            Some(ProviderId::Anthropic)
        );
    }

    #[test]
    fn openai_compatible_kind_has_no_provider_id() {
        let a = AccountSpec::new("oracle", "openai-compatible");
        assert_eq!(a.provider_id(), None);
    }
}
