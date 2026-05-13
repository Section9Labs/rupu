#![deny(clippy::all)]

//! rupu SCM connectors — typed per-platform repo + issue access.
//!
//! Defines [`RepoConnector`] and [`IssueConnector`] trait families
//! plus a [`Registry`] that builds connectors from configured
//! credentials. Per-platform impls live in `connectors/<platform>/`.
//!
//! Spec: `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`.

pub mod clone;
pub mod connectors;
pub mod error;
pub mod event_connector;
pub mod platform;
pub mod registry;
pub mod types;

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
