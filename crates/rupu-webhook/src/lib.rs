//! Webhook receiver for `trigger.on: event` workflows.
//!
//! Long-running HTTP server (axum) that listens for SCM-vendor
//! webhooks (GitHub, GitLab, Linear, Jira), validates the signature, maps the
//! incoming payload to a stable rupu event identifier
//! (`github.pr.opened`, `github.issue.created`, etc.), evaluates
//! each candidate workflow's optional `filter:` expression against
//! the event payload, and dispatches the matching workflows via a
//! caller-supplied [`WorkflowDispatcher`].
//!
//! Signature validation:
//! - **GitHub**: HMAC-SHA256 of the raw body, compared in constant
//!   time against the hex-encoded value in the
//!   `x-hub-signature-256` header (prefixed `sha256=`).
//! - **GitLab**: simple shared-secret comparison against the
//!   `x-gitlab-token` header. (GitLab's webhook UI does not offer
//!   HMAC; the shared-secret model is what their docs prescribe.)
//! - **Jira Cloud**: HMAC-SHA256 in the WebSub-style
//!   `x-hub-signature` header (`sha256=<hex>`).
//!
//! Filter evaluation: `filter:` is a minijinja expression rendered
//! against `{ "event": <payload> }`. Falsy result skips the
//! workflow; truthy fires it. Same falsy literals as the
//! orchestrator's `when:` (false / 0 / empty / no / off).
//!
//! Out of scope (see TODO.md):
//! - Replay protection (X-GitHub-Delivery dedup)
//! - Concurrent workflow dispatch
//! - Threading event JSON into agent prompts (`{{event.*}}`); this
//!   PR runs filter against event JSON but the workflow's prompts
//!   only see `inputs.*` and `steps.*.*`. Follow-up PR will plumb
//!   event-context into the orchestrator's `StepContext`.

pub mod dispatch;
pub mod event_vocab;
pub mod server;
pub mod signature;

pub use dispatch::{dispatch_event, DispatchOutcome, DispatchedWorkflow, WorkflowDispatcher};
pub use event_vocab::{
    map_github_event, map_gitlab_event, map_jira_event, map_linear_event,
    normalize_jira_event_payload, normalize_linear_event_payload,
};
pub use server::{
    serve, WebhookConfig, WebhookError, WebhookEvent, WebhookObserver, WebhookSource,
};
pub use signature::{
    verify_github_signature, verify_gitlab_token, verify_jira_signature, verify_linear_signature,
    SignatureError,
};
