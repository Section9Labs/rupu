//! OS keychain backend (macOS Keychain / Linux Secret Service via D-Bus
//! / Windows Credential Manager via the [`keyring`] crate).
//!
//! Probe failure is what triggers fallback to [`crate::JsonFileBackend`]
//! at backend-selection time; see [`crate::probe::select_backend`].
//!
//! [`keyring`]: ::keyring

use crate::backend::{AuthBackend, AuthError, ProviderId};
use tracing::warn;

/// Service name used as the keychain entry namespace. All rupu
/// secrets share this service; the keychain entry username is the
/// provider id (`anthropic`, `openai`, etc.).
///
/// Hardcoded for v0; multi-environment support (e.g., separate
/// `rupu-dev` and `rupu-prod` namespaces) is deferred to Slice C
/// where it ties into workspace/profile config. Until then, all rupu
/// installs on the same machine share keychain entries.
const SERVICE: &str = "rupu";

/// Backend wrapping the OS keychain. Construct via [`Self::new`] or
/// `Default::default()`.
#[derive(Debug, Default, Clone)]
pub struct KeyringBackend;

impl KeyringBackend {
    pub fn new() -> Self {
        Self
    }

    /// Probe for keychain availability by attempting a no-op
    /// set-then-delete on a sentinel entry. Returns `Ok(())` if the
    /// keychain is reachable. Used by [`crate::select_backend`] to
    /// decide whether to use this backend or fall back to a
    /// chmod-600 JSON file.
    pub fn probe() -> Result<(), AuthError> {
        let entry = ::keyring::Entry::new(SERVICE, "__probe__")?;
        // Try set then delete; either failing means we should fall back.
        entry.set_password("probe")?;
        // Best-effort cleanup. If the delete fails, the next probe will
        // re-set and re-delete; the entry is harmless either way. Warn
        // so a stale `rupu/__probe__` entry is diagnosable in the user's
        // keychain UI.
        if let Err(e) = entry.delete_credential() {
            warn!(error = %e, "keyring probe: delete_credential failed; stale rupu/__probe__ entry may persist");
        }
        Ok(())
    }

    fn entry(&self, p: ProviderId) -> Result<::keyring::Entry, AuthError> {
        Ok(::keyring::Entry::new(SERVICE, p.as_str())?)
    }
}

impl AuthBackend for KeyringBackend {
    fn store(&self, p: ProviderId, secret: &str) -> Result<(), AuthError> {
        self.entry(p)?.set_password(secret)?;
        Ok(())
    }

    fn retrieve(&self, p: ProviderId) -> Result<String, AuthError> {
        match self.entry(p)?.get_password() {
            Ok(s) => Ok(s),
            Err(::keyring::Error::NoEntry) => Err(AuthError::NotConfigured(p)),
            Err(e) => Err(AuthError::Keyring(e)),
        }
    }

    fn forget(&self, p: ProviderId) -> Result<(), AuthError> {
        match self.entry(p)?.delete_credential() {
            Ok(()) | Err(::keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::Keyring(e)),
        }
    }

    fn name(&self) -> &'static str {
        "os-keychain"
    }
}
