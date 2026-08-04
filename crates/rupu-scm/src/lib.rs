#![deny(clippy::all)]
// disallowed_methods is grouped under clippy::all in this clippy version;
// the workspace-level `disallowed_methods = "warn"` (Cargo.toml) is meant
// to keep this crate buildable under clippy until Plan 2 migrates it onto
// rupu-netflow, but a crate-level `deny(clippy::all)` would otherwise
// silently escalate it back to a hard error. Downgrade it back to warn
// here explicitly; Plan 2's final task removes this override along with
// the last raw reqwest call in this crate.
#![warn(clippy::disallowed_methods)]

//! rupu SCM connectors — typed per-platform repo + issue access.
//!
//! Defines [`RepoConnector`] and [`IssueConnector`] trait families
//! plus a [`Registry`] that builds connectors from configured
//! credentials. Per-platform impls live in `connectors/<platform>/`.
//!
//! Spec: `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`.

pub mod client_options;
pub mod clone;
pub mod connectors;
pub mod error;
pub mod event_connector;
pub mod platform;
pub mod registry;
pub mod types;
pub mod weburl;

pub use client_options::{clone_url, scm_timeout, CloneProtocol, ScmClientOptions};
pub use clone::{clone_repo_ref, CloneError};
pub use connectors::{IssueConnector, RepoConnector};
pub use error::{classify_scm_error, ScmError};
pub use event_connector::{EventConnector, EventPollResult, PolledEvent};
pub use platform::{IssueTracker, Platform};
pub use registry::Registry;
pub use types::{
    Branch, Comment, CreateIssue, CreatePr, Diff, EventSourceRef, EventSubjectRef, FileContent,
    Issue, IssueFilter, IssueRef, IssueState, PipelineTrigger, Pr, PrFilter, PrRef, PrState, Repo,
    RepoRef, WorkflowDispatch,
};

/// Install a process-level rustls `CryptoProvider`, once.
///
/// The dependency tree enables **both** rustls backends (aws-lc-rs via
/// object_store/reqwest, ring via octocrab/jsonwebtoken), so rustls 0.23
/// refuses to auto-select and panics on first TLS use. `rupu-cli`'s `main`
/// installs one at startup, but anything that links this crate without going
/// through that binary — tests, `rupu-cp` embedders, other consumers — gets
/// the panic instead. Call this first if you are not the `rupu` binary.
///
/// Idempotent and safe to call from many threads: a lost race just means
/// somebody else installed a provider, which is the desired end state.
pub fn install_default_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
