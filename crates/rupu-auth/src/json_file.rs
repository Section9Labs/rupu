//! Plaintext JSON file backend with chmod-600 enforcement.
//!
//! Used when the OS keychain is unreachable. Each call to [`store`]
//! resets the file's permissions to mode 0600. [`retrieve`] warns
//! (via `tracing::warn!`) if the file's permissions are wider than
//! 0600, but does not refuse to read — refusing would prevent
//! recovery on a misconfigured machine.
//!
//! [`store`]: <crate::AuthBackend::store>
//! [`retrieve`]: <crate::AuthBackend::retrieve>

use crate::backend::{AuthBackend, AuthError, ProviderId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing::warn;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// On-disk shape for the auth.json fallback file. Map from provider
/// id (snake_case) to the secret string. `BTreeMap` so the file's
/// key ordering is stable across writes (deterministic output is
/// nice for diff'ability if a developer ever inspects it).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default, flatten)]
    secrets: BTreeMap<String, String>,
}

/// Backend writing to `~/.rupu/auth.json` (or any configured path)
/// with mode 0600 enforced on every write. Used as a fallback when
/// the OS keychain is unreachable.
#[derive(Debug, Clone)]
pub struct JsonFileBackend {
    pub path: PathBuf,
}

impl JsonFileBackend {
    fn read(&self) -> Result<Stored, AuthError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Stored::default()),
            Err(e) => return Err(e.into()),
        };
        self.warn_on_wrong_mode();
        let s: Stored = serde_json::from_str(&text)?;
        Ok(s)
    }

    fn write(&self, s: &Stored) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(s)?;
        std::fs::write(&self.path, body)?;
        self.set_mode_0600();
        Ok(())
    }

    #[cfg(unix)]
    fn set_mode_0600(&self) {
        if let Ok(meta) = std::fs::metadata(&self.path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&self.path, perms) {
                warn!(
                    path = %self.path.display(),
                    error = %e,
                    "could not enforce mode 0600 on auth.json — file may be readable by other users"
                );
            }
        }
    }

    #[cfg(not(unix))]
    fn set_mode_0600(&self) {}

    #[cfg(unix)]
    fn warn_on_wrong_mode(&self) {
        if let Ok(meta) = std::fs::metadata(&self.path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o600 {
                warn!(
                    path = %self.path.display(),
                    mode = format!("{mode:o}"),
                    "auth.json should be mode 0600 — fix with: chmod 600 {}",
                    self.path.display()
                );
            }
        }
    }

    #[cfg(not(unix))]
    fn warn_on_wrong_mode(&self) {}
}

impl AuthBackend for JsonFileBackend {
    fn store(&self, p: ProviderId, secret: &str) -> Result<(), AuthError> {
        let mut s = self.read()?;
        s.secrets.insert(p.as_str().to_string(), secret.to_string());
        self.write(&s)
    }

    fn retrieve(&self, p: ProviderId) -> Result<String, AuthError> {
        let s = self.read()?;
        s.secrets
            .get(p.as_str())
            .cloned()
            .ok_or(AuthError::NotConfigured(p))
    }

    fn forget(&self, p: ProviderId) -> Result<(), AuthError> {
        let mut s = self.read()?;
        if s.secrets.remove(p.as_str()).is_some() {
            self.write(&s)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "json-file"
    }
}
