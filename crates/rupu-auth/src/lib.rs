//! rupu-auth — credential storage with OS keychain + chmod-600 fallback.
//!
//! Two backend implementations:
//!
//! - [`KeyringBackend`] uses the OS keychain (macOS Keychain, Linux
//!   Secret Service via D-Bus, Windows Credential Manager). Preferred
//!   when reachable.
//! - [`JsonFileBackend`] stores secrets in `~/.rupu/auth.json` with
//!   permissions enforced to mode 0600. Used as a fallback when the
//!   keychain is unavailable (e.g. headless Linux servers without a
//!   running secret-service daemon).
//!
//! [`select_backend`] probes the keychain once and caches the choice
//! at `~/.rupu/cache/auth-backend.json` so subsequent invocations
//! avoid the probe overhead.

pub mod backend;

// Real implementations land in Tasks 13-15:
// - json_file: Task 13 (JsonFileBackend with chmod-600 enforcement)
// - keyring: Task 14 (KeyringBackend with probe())
// - probe: Task 15 (select_backend with cache file)
pub mod json_file;
pub mod keyring;
pub mod probe;

pub use backend::{AuthBackend, AuthError, ProviderId};
pub use json_file::JsonFileBackend;
pub use keyring::KeyringBackend;
pub use probe::{select_backend, BackendChoice, ProbeCache};
