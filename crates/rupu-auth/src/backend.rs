//! Auth backend trait. Implementations: keyring, JSON file (chmod 600).
//!
//! The `AuthBackend` is the abstraction the agent runtime uses to
//! retrieve provider credentials. The concrete backend is selected at
//! probe time (see [`crate::probe`]) and may be either the OS keychain
//! or a chmod-600 fallback file at `~/.rupu/auth.json`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Identifies a credential namespace within the backend (one secret
/// per provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Anthropic,
    Openai,
    Copilot,
    Local,
}

impl ProviderId {
    /// Stable string form used as the keychain entry username and as
    /// the JSON-file key.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Copilot => "copilot",
            Self::Local => "local",
        }
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
    Keyring(String),
    #[error("not configured for provider {0}")]
    NotConfigured(&'static str),
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
