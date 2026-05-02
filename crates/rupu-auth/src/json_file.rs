//! Plaintext JSON file backend with chmod-600 enforcement.
//! Implemented in Task 13 of Plan 1.

use crate::backend::{AuthBackend, AuthError, ProviderId};
use std::path::PathBuf;

/// Backend writing to `~/.rupu/auth.json` (or any configured path)
/// with mode 0600 enforced on every write. Used as a fallback when
/// the OS keychain is unreachable.
pub struct JsonFileBackend {
    pub path: PathBuf,
}

impl AuthBackend for JsonFileBackend {
    fn store(&self, _p: ProviderId, _s: &str) -> Result<(), AuthError> {
        todo!("JsonFileBackend lands in Task 13")
    }
    fn retrieve(&self, _p: ProviderId) -> Result<String, AuthError> {
        todo!("JsonFileBackend lands in Task 13")
    }
    fn forget(&self, _p: ProviderId) -> Result<(), AuthError> {
        todo!("JsonFileBackend lands in Task 13")
    }
    fn name(&self) -> &'static str {
        "json-file"
    }
}
