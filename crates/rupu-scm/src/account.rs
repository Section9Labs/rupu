//! Account identity for SCM connectors.
//!
//! Mirrors the model Arc 1 established for LLM providers: the account
//! name is the identity, `kind` is the vendor. `Platform` stays the
//! vendor — it is correctly typed as such on `RepoRef` — so only the
//! `Registry`'s map key becomes an `AccountId`.

use std::fmt;

/// The name of one configured SCM account, e.g. `gh-work`.
///
/// Ordered so account listings and fan-out results are deterministic
/// rather than hash-ordered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub String);

impl AccountId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for AccountId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_round_trips_and_displays() {
        let a = AccountId::new("gh-work");
        assert_eq!(a.as_str(), "gh-work");
        assert_eq!(a.to_string(), "gh-work");
        assert_eq!(AccountId::from("gh-work"), a);
    }

    #[test]
    fn account_ids_sort_deterministically() {
        let mut v = [
            AccountId::new("gh-work"),
            AccountId::new("acme-ghe"),
            AccountId::new("gh-personal"),
        ];
        v.sort();
        assert_eq!(
            v.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
            ["acme-ghe", "gh-personal", "gh-work"]
        );
    }
}
