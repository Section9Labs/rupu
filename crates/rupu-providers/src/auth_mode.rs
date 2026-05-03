//! Neutral auth-mode marker used across the runtime.
//!
//! Decouples the agent runtime and CLI from the on-the-wire shape of
//! `AuthCredentials`. The runtime only needs to know "is this an API
//! key or an SSO bearer?" for routing and rendering; the actual secret
//! lives behind the `CredentialResolver` (Plan 2) or the existing
//! `AuthBackend` (Plan 1).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    ApiKey,
    Sso,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ApiKey => "api-key",
            Self::Sso => "sso",
        }
    }
}

impl fmt::Display for AuthMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AuthMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "api-key" | "api_key" | "apikey" => Ok(Self::ApiKey),
            "sso" | "oauth" => Ok(Self::Sso),
            _ => Err(format!("unknown auth mode: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_as_str() {
        assert_eq!(AuthMode::ApiKey.to_string(), "api-key");
        assert_eq!(AuthMode::Sso.to_string(), "sso");
    }

    #[test]
    fn from_str_accepts_canonical_and_aliases() {
        assert_eq!(AuthMode::from_str("api-key").unwrap(), AuthMode::ApiKey);
        assert_eq!(AuthMode::from_str("api_key").unwrap(), AuthMode::ApiKey);
        assert_eq!(AuthMode::from_str("sso").unwrap(), AuthMode::Sso);
        assert_eq!(AuthMode::from_str("oauth").unwrap(), AuthMode::Sso);
        assert!(AuthMode::from_str("nope").is_err());
    }

    #[test]
    fn serde_roundtrip_kebab_case() {
        let json = serde_json::to_string(&AuthMode::ApiKey).unwrap();
        assert_eq!(json, "\"api-key\"");
        let parsed: AuthMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, AuthMode::ApiKey);
    }
}
