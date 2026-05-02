//! OS keychain backend. Implemented in Task 14 of Plan 1.

use crate::backend::{AuthBackend, AuthError, ProviderId};

/// Backend wrapping the OS keychain via the `keyring` crate.
pub struct KeyringBackend;

impl KeyringBackend {
    pub fn new() -> Self {
        Self
    }

    /// Returns `Ok(())` if the OS keychain is reachable; `Err`
    /// otherwise. Used by [`crate::select_backend`] to choose a
    /// backend at startup.
    pub fn probe() -> Result<(), AuthError> {
        todo!("KeyringBackend::probe lands in Task 14")
    }
}

impl Default for KeyringBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthBackend for KeyringBackend {
    fn store(&self, _p: ProviderId, _s: &str) -> Result<(), AuthError> {
        todo!("KeyringBackend lands in Task 14")
    }
    fn retrieve(&self, _p: ProviderId) -> Result<String, AuthError> {
        todo!("KeyringBackend lands in Task 14")
    }
    fn forget(&self, _p: ProviderId) -> Result<(), AuthError> {
        todo!("KeyringBackend lands in Task 14")
    }
    fn name(&self) -> &'static str {
        "os-keychain"
    }
}
