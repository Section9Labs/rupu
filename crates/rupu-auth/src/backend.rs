//! Auth backend trait. Implementations: keyring, JSON file (chmod 600).
//!
//! The `AuthBackend` is the abstraction the agent runtime uses to
//! retrieve provider credentials. The concrete backend is selected at
//! probe time (see [`crate::probe`]) and may be either the OS keychain
//! or a chmod-600 fallback file at `~/.rupu/auth.json`.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Identifies a credential namespace within the backend (one secret
/// per provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Anthropic,
    Openai,
    Gemini,
    Copilot,
    Local,
    Github,
    Gitlab,
    Linear,
}

impl ProviderId {
    /// Stable string form used as the keychain entry username and as
    /// the JSON-file key.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
            Self::Local => "local",
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Linear => "linear",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors from credential storage operations.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("not configured for provider {0}")]
    NotConfigured(ProviderId),
}

/// Credential store. Implementations: [`crate::KeyringBackend`] and
/// [`crate::JsonFileBackend`].
pub trait AuthBackend: Send + Sync {
    /// Store `secret` for `provider`, replacing any existing value.
    fn store(&self, provider: ProviderId, secret: &str) -> Result<(), AuthError>;
    /// Retrieve the secret for `provider`. Returns `NotConfigured` if absent.
    fn retrieve(&self, provider: ProviderId) -> Result<String, AuthError>;
    /// Forget the secret for `provider`. No-op if absent.
    fn forget(&self, provider: ProviderId) -> Result<(), AuthError>;
    /// Human-readable backend name for `rupu auth status`.
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod gemini_id_tests {
    use super::*;

    #[test]
    fn provider_id_gemini_string_form() {
        assert_eq!(ProviderId::Gemini.as_str(), "gemini");
    }

    #[test]
    fn provider_id_gemini_serde_roundtrip() {
        let json = serde_json::to_string(&ProviderId::Gemini).unwrap();
        assert_eq!(json, "\"gemini\"");
        let parsed: ProviderId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ProviderId::Gemini);
    }
}

#[cfg(test)]
mod scm_provider_id_tests {
    use super::*;

    #[test]
    fn github_string_form() {
        assert_eq!(ProviderId::Github.as_str(), "github");
    }

    #[test]
    fn gitlab_string_form() {
        assert_eq!(ProviderId::Gitlab.as_str(), "gitlab");
    }

    #[test]
    fn github_serde_roundtrip() {
        let json = serde_json::to_string(&ProviderId::Github).unwrap();
        assert_eq!(json, "\"github\"");
        let p: ProviderId = serde_json::from_str(&json).unwrap();
        assert_eq!(p, ProviderId::Github);
    }

    #[test]
    fn linear_string_form() {
        assert_eq!(ProviderId::Linear.as_str(), "linear");
    }
}
