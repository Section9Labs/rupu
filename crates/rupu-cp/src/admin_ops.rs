//! Port: the autoflow, cron and maintenance operations only the CLI had —
//! `rupu autoflow show|explain|doctor|wakes|repair|run`, `rupu cron
//! list|events|tick` and `rupu transcript|session|netflow prune` /
//! `rupu cleanup`. rupu-cp defines it; rupu-cli's `cp serve` implements it
//! over the same functions those commands run (`cp_admin_ops.rs`), so the
//! API and the CLI share one implementation. `None` → the endpoints
//! answer 501.
//!
//! Every report is the JSON the CLI command prints with `--format json`
//! (its `kind` + `version` + rows/item), carried through as it is.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Why an operation was refused, by the status the endpoint answers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdminOpError {
    /// 400: an argument the operation can't take (a bad issue ref, a bad
    /// duration, conflicting flags).
    #[error("{0}")]
    Invalid(String),
    /// 404: nothing by that name (no visible autoflow, no claim).
    #[error("{0}")]
    NotFound(String),
    /// 409: the state forbids it now (an ambiguous autoflow name, a claim
    /// another cycle holds, a tick already running).
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

/// `POST /api/autoflows/claims/repair`: `rupu autoflow repair <ref>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairRequest {
    pub issue_ref: String,
    /// Release the claim after repairing.
    pub release: bool,
    /// Queue a follow-up wake after repairing.
    pub requeue: bool,
    /// Report what would be repaired; change nothing.
    pub dry_run: bool,
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

/// What `POST /api/maintenance/prune` removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PruneKind {
    /// Archived standalone transcripts (`rupu transcript prune`).
    Transcripts,
    /// Archived sessions (`rupu session prune`).
    Sessions,
    /// Netflow ledgers (`rupu netflow prune`).
    Netflow,
    /// Archived sessions and transcripts together (`rupu cleanup`).
    Cleanup,
}

impl PruneKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transcripts => "transcripts",
            Self::Sessions => "sessions",
            Self::Netflow => "netflow",
            Self::Cleanup => "cleanup",
        }
    }
}

/// `POST /api/maintenance/prune`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneRequest {
    pub kind: PruneKind,
    /// `--older-than` (`30d`, `12h`, `1w`); `None` = the command's default.
    pub older_than: Option<String>,
    pub dry_run: bool,
}

#[async_trait]
pub trait AdminOps: Send + Sync {
    /// `rupu autoflow show <name> [--repo]`: the `autoflow_show` report.
    async fn autoflow_show(
        &self,
        name: &str,
        repo: Option<&str>,
    ) -> Result<serde_json::Value, AdminOpError>;
    /// `rupu autoflow explain <ref> [--repo]`: the `autoflow_explain` report.
    async fn autoflow_explain(
        &self,
        issue_ref: &str,
        repo: Option<&str>,
    ) -> Result<serde_json::Value, AdminOpError>;
    /// `rupu autoflow doctor [--repo]`: the `autoflow_doctor` report.
    async fn autoflow_doctor(&self, repo: Option<&str>) -> Result<serde_json::Value, AdminOpError>;
    /// `rupu autoflow wakes [--repo]`: the `autoflow_wakes` report.
    async fn autoflow_wakes(&self, repo: Option<&str>) -> Result<serde_json::Value, AdminOpError>;
    /// `rupu autoflow repair`: `{issue_ref, dry_run, repairs}`.
    async fn autoflow_repair(&self, req: RepairRequest) -> Result<serde_json::Value, AdminOpError>;
    /// Check that the cycle can start (the autoflow resolves for the issue,
    /// no other cycle holds its claim), then start `rupu autoflow run`
    /// detached. Returns `{name, issue}` once the child is spawned.
    async fn autoflow_run(
        &self,
        req: AutoflowRunRequest,
    ) -> Result<serde_json::Value, AdminOpError>;
    /// `rupu cron list`: the `cron_list` report.
    async fn cron_list(&self) -> Result<serde_json::Value, AdminOpError>;
    /// `rupu cron events`: the `cron_events` report.
    async fn cron_events(&self) -> Result<serde_json::Value, AdminOpError>;
    /// The tick `cp serve`'s cron loop runs. Single-flight with that loop:
    /// [`AdminOpError::Conflict`] while a tick is in progress.
    async fn cron_tick(&self, req: CronTickRequest) -> Result<CronTickOutcome, AdminOpError>;
    /// The CLI's prune / cleanup: the rows it removed or would remove.
    async fn prune(&self, req: PruneRequest) -> Result<serde_json::Value, AdminOpError>;
}

impl From<AdminOpError> for crate::error::ApiError {
    fn from(e: AdminOpError) -> Self {
        match e {
            AdminOpError::Invalid(m) => Self::bad_request(m),
            AdminOpError::NotFound(m) => Self::not_found(m),
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

/// `?host=` on a local-only operation.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct HostQuery {
    /// A host id; only `local` (or none) is served, any other is a 501.
    pub(crate) host: Option<String>,
}
