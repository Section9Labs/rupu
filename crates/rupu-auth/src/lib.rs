//! rupu-auth — credential storage.
//!
//! One backend: [`JsonFileBackend`] stores secrets in `~/.rupu/auth.json`
//! with permissions enforced to mode 0600, honoring `RUPU_HOME` and
//! `RUPU_AUTH_FILE`.
//!
//! The OS keychain backend was retired. A bare CLI binary's keychain
//! requirement is cdhash-bound, so every rebuild invalidated it and the
//! next read failed silently — the "credentials vanished after an
//! update" failure. Peer CLIs (`gh`, `aws`, `gcloud`, `kubectl`,
//! `terraform`) all use files for the same reason.
//!
//! Note this concerns where rupu stores *its own* credentials. Importing
//! credentials another tool left in a keychain is a separate concern and
//! lives in `rupu-providers` (`auth::discovery`).

pub mod backend;
pub mod json_file;

pub mod account_key;
pub use account_key::{account_for, legacy_account_for};

pub mod oauth;

pub mod stored;
pub use stored::StoredCredential;

pub mod in_memory;
pub mod resolver;
pub use resolver::{CredentialResolver, KeychainResolver};

pub use backend::{AuthBackend, AuthError, ProviderId};
pub use json_file::JsonFileBackend;
