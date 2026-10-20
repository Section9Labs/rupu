//! Port: the two operations that can't run in a library call. rupu-cp
//! defines it; rupu-cli's `cp serve` implements it (`cp_admin_ops.rs`).
//! `None` → those endpoints answer 501.
//!
//! - `cron_tick` runs due workflows through the CLI's workflow runtime,
//!   single-flight with `cp serve`'s own cron loop.
//! - `autoflow_run` spawns a detached `rupu autoflow run` child.
//!
//! Everything else the autoflow, cron and maintenance endpoints do is
//! served from its library directly: the autoflow reads, repair and the run
//! precheck from `rupu_autoflow`, the cron listings from
//! `rupu_orchestrator::cron_triggers`, the prunes from
//! `rupu_orchestrator::{archived_transcripts, cleanup}`,
//! `rupu_runtime::sessions` and [`crate::netflow_prune`].
//!
//! This module also holds what those endpoints share: [`local_only`],
//! [`full_issue_ref`], [`HostQuery`], and [`blocking`], which runs a library
//! call off the executor and maps its refusals onto statuses.

use async_trait::async_trait;
use serde::Deserialize;

/// Why a port operation was refused, by the status the endpoint answers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdminOpError {
    /// 400: an argument the operation can't take (a mode autoflows don't
    /// run in).
    #[error("{0}")]
    Invalid(String),
    /// 409: the state forbids it now (a tick already running).
    #[error("{0}")]
    Conflict(String),
    /// 500: the operation failed.
    #[error("{0}")]
    Failed(String),
}

/// `POST /api/autoflows/:name/run`: one autonomous cycle of `name` for
/// `issue`, run detached (`rupu autoflow run <name> <issue>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoflowRunRequest {
    pub name: String,
    /// The issue in full run-target form (`github:owner/repo/issues/42`).
    pub issue: String,
    /// `--repo`: limit resolution to one bound repo.
    pub repo: Option<String>,
    /// `--mode`: `bypass` | `readonly`.
    pub mode: Option<String>,
}

/// `POST /api/cron/tick`: `rupu cron tick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CronTickRequest {
    pub dry_run: bool,
    /// Only the cron-scheduled tier (`--skip-events`).
    pub skip_events: bool,
    /// Only the event-polling tier (`--only-events`).
    pub only_events: bool,
}

/// What `POST /api/cron/tick` did.
#[derive(Debug, Clone, PartialEq)]
pub enum CronTickOutcome {
    /// A real tick started in the background; it fires what is due.
    Started,
    /// A dry run finished: its report of what would fire.
    DryRun(serde_json::Value),
}

#[async_trait]
pub trait AdminOps: Send + Sync {
    /// Start `rupu autoflow run` detached. The endpoint has already run
    /// `rupu_autoflow::run_precheck` (the autoflow resolves for the issue,
    /// no other cycle holds its claim). Returns `{name, issue}` once the
    /// child is spawned.
    async fn autoflow_run(
        &self,
        req: AutoflowRunRequest,
    ) -> Result<serde_json::Value, AdminOpError>;
    /// The tick `cp serve`'s cron loop runs. Single-flight with that loop:
    /// [`AdminOpError::Conflict`] while a tick is in progress.
    async fn cron_tick(&self, req: CronTickRequest) -> Result<CronTickOutcome, AdminOpError>;
}

impl From<AdminOpError> for crate::error::ApiError {
    fn from(e: AdminOpError) -> Self {
        match e {
            AdminOpError::Invalid(m) => Self::bad_request(m),
            AdminOpError::Conflict(m) => Self::conflict(m),
            AdminOpError::Failed(m) => Self::internal(m),
        }
    }
}

/// The port, or a 501 naming what needs `rupu cp serve`.
pub(crate) fn ops<'a>(
    s: &'a crate::state::AppState,
    what: &str,
) -> crate::error::ApiResult<&'a dyn AdminOps> {
    s.admin_ops.as_deref().ok_or_else(|| {
        crate::error::ApiError::not_available(format!("{what} requires `rupu cp serve`"))
    })
}

/// These operations run on this control plane's own `RUPU_HOME` only: a
/// `host` other than `local` is a 501 that says so.
pub(crate) fn local_only(host: Option<&str>, what: &str) -> crate::error::ApiResult<()> {
    match host {
        None | Some("") | Some("local") => Ok(()),
        Some(h) => Err(crate::error::ApiError::not_available(format!(
            "{what} is local-only: host `{h}` can't serve it"
        ))),
    }
}

/// An issue ref in its full form, `<tracker>:<project>/issues/<number>`,
/// or a 400. The control plane has no checkout of its own, so a bare `42`
/// or `#42` — which the CLI resolves against its working directory — could
/// only land on whatever repo `cp serve` happened to start in.
pub(crate) fn full_issue_ref(value: &str) -> crate::error::ApiResult<String> {
    rupu_scm::IssueRef::parse_full(value)
        .map(|r| r.to_full())
        .map_err(|e| {
            crate::error::ApiError::bad_request(format!(
                "{e}: give the full `<tracker>:<project>/issues/<number>` form"
            ))
        })
}

/// An autoflow library failure as its status: a [`rupu_autoflow::Refusal`]
/// anywhere in the chain is its 400 / 404 / 409 (its own message), anything
/// else a 500 with the whole chain.
pub(crate) fn autoflow_error(e: anyhow::Error) -> crate::error::ApiError {
    use rupu_autoflow::Refusal;
    match rupu_autoflow::refusal_of(&e) {
        Some(Refusal::Invalid(m)) => crate::error::ApiError::bad_request(m.clone()),
        Some(Refusal::NotFound(m)) => crate::error::ApiError::not_found(m.clone()),
        Some(Refusal::Conflict(m)) => crate::error::ApiError::conflict(m.clone()),
        None => crate::error::ApiError::internal(format!("{e:#}")),
    }
}

/// Run a synchronous autoflow library call on the blocking pool, its
/// failure mapped by [`autoflow_error`].
pub(crate) async fn blocking<T, F>(f: F) -> crate::error::ApiResult<T>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| crate::error::ApiError::internal(format!("task panicked: {e}")))?
        .map_err(autoflow_error)
}

/// `?host=` on a local-only operation.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct HostQuery {
    /// A host id; only `local` (or none) is served, any other is a 501.
    pub(crate) host: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use rupu_autoflow::Refusal;

    #[test]
    fn a_library_refusal_keeps_its_status_and_message() {
        for (refusal, status) in [
            (Refusal::Invalid("bad ref".into()), StatusCode::BAD_REQUEST),
            (Refusal::NotFound("no claim".into()), StatusCode::NOT_FOUND),
            (Refusal::Conflict("held".into()), StatusCode::CONFLICT),
        ] {
            let message = refusal.to_string();
            let e = anyhow::Error::from(refusal).context("repair");
            let got = autoflow_error(e);
            assert_eq!((got.0, got.1), (status, message));
        }
        let got = autoflow_error(anyhow::anyhow!("disk on fire").context("doctor"));
        assert_eq!(
            (got.0, got.1),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "doctor: disk on fire".into()
            )
        );
    }
}
