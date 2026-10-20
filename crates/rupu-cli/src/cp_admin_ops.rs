//! `cp serve` adapter for rupu-cp's `AdminOps` port: the control plane's
//! autoflow, cron and maintenance endpoints run the same functions as
//! `rupu autoflow show|explain|doctor|wakes|repair|run`, `rupu cron
//! list|events|tick` and the prune / cleanup commands, and answer with the
//! JSON report each command prints under `--format json`.
//!
//! Everything runs in this process except an autoflow cycle, which is
//! started as a detached `rupu autoflow run` child (W6's `RunArgv`): a cycle
//! fetches the issue and drives a workflow run, so it outlives the request.

use std::path::PathBuf;

use rupu_cp::admin_ops::{
    AdminOpError, AdminOps, AutoflowRunRequest, CronTickOutcome, CronTickRequest, PruneKind,
    PruneRequest, RepairRequest,
};
use rupu_runtime::argv::{AutoflowRun, RunArgv};
use rupu_runtime::spawn::{spawn_detached, SpawnSpec, SpawnStdio};
use serde_json::{json, Value};

/// The adapter. `exe` is the running `rupu` binary (`current_exe()`);
/// `global` the `RUPU_HOME` the control plane serves.
pub struct CliAdminOps {
    pub exe: PathBuf,
    pub global: PathBuf,
}

/// The typed refusal a CLI function raised (anywhere in the chain), else a
/// failure carrying the whole message.
fn classify(e: anyhow::Error) -> AdminOpError {
    e.chain()
        .find_map(|cause| cause.downcast_ref::<AdminOpError>().cloned())
        .unwrap_or_else(|| AdminOpError::Failed(format!("{e:#}")))
}

/// Run a synchronous CLI function on the blocking pool.
async fn blocking<T, F>(f: F) -> Result<T, AdminOpError>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AdminOpError::Failed(format!("task panicked: {e}")))?
        .map_err(classify)
}

/// The detached `rupu autoflow run` for `req`, its stderr kept in `log` so
/// a cycle that dies before it records anything leaves its error behind.
pub(crate) fn autoflow_run_spec(
    exe: &std::path::Path,
    req: &AutoflowRunRequest,
    log: PathBuf,
) -> Result<SpawnSpec, AdminOpError> {
    let mode = req
        .mode
        .as_deref()
        .map(rupu_tools::PermissionMode::parse)
        .transpose()
        .map_err(|e| AdminOpError::Invalid(e.to_string()))?;
    let mut spec = SpawnSpec::new(
        exe,
        RunArgv::AutoflowRun(AutoflowRun {
            name: req.name.clone(),
            target: req.issue.clone(),
            repo: req.repo.clone(),
            mode,
        }),
    );
    spec.stdio = SpawnStdio::Log(log);
    Ok(spec)
}

#[async_trait::async_trait]
impl AdminOps for CliAdminOps {
    async fn autoflow_show(&self, name: &str, repo: Option<&str>) -> Result<Value, AdminOpError> {
        let (name, repo) = (name.to_string(), repo.map(str::to_string));
        blocking(move || crate::cmd::autoflow::show_json(&name, repo.as_deref())).await
    }

    async fn autoflow_explain(
        &self,
        issue_ref: &str,
        repo: Option<&str>,
    ) -> Result<Value, AdminOpError> {
        let (issue_ref, repo) = (issue_ref.to_string(), repo.map(str::to_string));
        blocking(move || crate::cmd::autoflow::explain_json(&issue_ref, repo.as_deref())).await
    }

    async fn autoflow_doctor(&self, repo: Option<&str>) -> Result<Value, AdminOpError> {
        let (global, repo) = (self.global.clone(), repo.map(str::to_string));
        blocking(move || crate::cmd::autoflow::doctor_json(&global, repo.as_deref())).await
    }

    async fn autoflow_wakes(&self, repo: Option<&str>) -> Result<Value, AdminOpError> {
        let (global, repo) = (self.global.clone(), repo.map(str::to_string));
        blocking(move || crate::cmd::autoflow::wakes_json(&global, repo.as_deref())).await
    }

    async fn autoflow_repair(&self, req: RepairRequest) -> Result<Value, AdminOpError> {
        let global = self.global.clone();
        blocking(move || {
            let outcome = crate::cmd::autoflow::repair_claim(
                &global,
                &req.issue_ref,
                req.release,
                req.requeue,
                req.dry_run,
            )?;
            Ok(serde_json::to_value(outcome)?)
        })
        .await
    }

    async fn autoflow_run(&self, req: AutoflowRunRequest) -> Result<Value, AdminOpError> {
        let global = self.global.clone();
        let check = req.clone();
        blocking(move || {
            crate::cmd::autoflow::run_precheck(
                &global,
                &check.name,
                &check.issue,
                check.repo.as_deref(),
                check.mode.as_deref(),
            )
        })
        .await?;
        let logs = self.global.join("autoflows").join("launches");
        std::fs::create_dir_all(&logs)
            .map_err(|e| AdminOpError::Failed(format!("creating {}: {e}", logs.display())))?;
        let log = logs.join(format!("{}.log", ulid::Ulid::new()));
        let spawned = spawn_detached(autoflow_run_spec(&self.exe, &req, log.clone())?)
            .map_err(|e| AdminOpError::Failed(format!("starting `rupu autoflow run`: {e}")))?;
        Ok(json!({
            "started": true,
            "name": req.name,
            "issue": req.issue,
            "pid": spawned.pid,
            "log": log.display().to_string(),
        }))
    }

    async fn cron_list(&self) -> Result<Value, AdminOpError> {
        blocking(crate::cmd::cron::list_json).await
    }

    async fn cron_events(&self) -> Result<Value, AdminOpError> {
        blocking(crate::cmd::cron::events_json).await
    }

    async fn cron_tick(&self, req: CronTickRequest) -> Result<CronTickOutcome, AdminOpError> {
        let Some(guard) = crate::cmd::cron::try_begin_tick() else {
            return Err(AdminOpError::Conflict(
                "a cron tick is already running; try again when it ends".into(),
            ));
        };
        if req.dry_run {
            let report = crate::cmd::cron::tick(true, req.skip_events, req.only_events)
                .await
                .map_err(classify)?;
            drop(guard);
            return serde_json::to_value(report)
                .map(CronTickOutcome::DryRun)
                .map_err(|e| AdminOpError::Failed(e.to_string()));
        }
        tokio::spawn(async move {
            let _guard = guard;
            if let Err(e) = crate::cmd::cron::tick(false, req.skip_events, req.only_events).await {
                tracing::warn!(error = %e, "cp serve: requested cron tick failed");
            }
        });
        Ok(CronTickOutcome::Started)
    }

    async fn prune(&self, req: PruneRequest) -> Result<Value, AdminOpError> {
        let global = self.global.clone();
        blocking(move || {
            let older = req.older_than.as_deref();
            let (report, errors) = match req.kind {
                PruneKind::Transcripts => (
                    crate::cmd::transcript::prune_json(older, req.dry_run)?,
                    Vec::new(),
                ),
                PruneKind::Sessions => (
                    crate::cmd::session::prune_json(older, req.dry_run)?,
                    Vec::new(),
                ),
                PruneKind::Netflow => crate::cmd::netflow::prune_json(&global, older, req.dry_run)?,
                PruneKind::Cleanup => (
                    crate::cmd::cleanup::cleanup_json(older, req.dry_run)?,
                    Vec::new(),
                ),
            };
            Ok(prune_answer(req.kind, req.dry_run, report, errors))
        })
        .await
    }
}

/// `{kind, dry_run, would_remove | removed, errors}`: the report's rows
/// under the key that says whether anything was deleted.
pub(crate) fn prune_answer(
    kind: PruneKind,
    dry_run: bool,
    report: Value,
    errors: Vec<String>,
) -> Value {
    let rows = report.get("rows").cloned().unwrap_or_else(|| json!([]));
    let key = if dry_run { "would_remove" } else { "removed" };
    let mut out = serde_json::Map::new();
    out.insert("kind".into(), json!(kind.as_str()));
    out.insert("dry_run".into(), json!(dry_run));
    out.insert(key.into(), rows);
    out.insert("errors".into(), json!(errors));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(mode: Option<&str>) -> AutoflowRunRequest {
        AutoflowRunRequest {
            name: "fix-issues".into(),
            issue: "github:acme/widgets/issues/42".into(),
            repo: Some("github:acme/widgets".into()),
            mode: mode.map(str::to_string),
        }
    }

    #[test]
    fn an_autoflow_run_is_a_detached_autoflow_run_logging_stderr() {
        let spec = autoflow_run_spec(
            std::path::Path::new("/bin/rupu"),
            &req(Some("readonly")),
            PathBuf::from("/tmp/x.log"),
        )
        .unwrap();
        let args: Vec<String> = spec
            .argv
            .to_args()
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "autoflow",
                "run",
                "--repo",
                "github:acme/widgets",
                "--mode",
                "readonly",
                "--",
                "fix-issues",
                "github:acme/widgets/issues/42",
            ]
        );
        assert_eq!(spec.stdio, SpawnStdio::Log(PathBuf::from("/tmp/x.log")));
    }

    #[test]
    fn an_unknown_mode_is_invalid() {
        let err = autoflow_run_spec(
            std::path::Path::new("/bin/rupu"),
            &req(Some("yolo")),
            PathBuf::from("/tmp/x.log"),
        )
        .unwrap_err();
        assert!(matches!(err, AdminOpError::Invalid(_)), "{err:?}");
    }

    #[test]
    fn a_typed_refusal_survives_context() {
        let e = anyhow::Error::from(AdminOpError::NotFound("no claim".into())).context("repair");
        assert_eq!(classify(e), AdminOpError::NotFound("no claim".into()));
        let e = anyhow::anyhow!("disk on fire");
        assert_eq!(classify(e), AdminOpError::Failed("disk on fire".into()));
    }

    #[test]
    fn a_prune_answer_names_its_rows_by_what_happened() {
        let report = json!({"kind": "session_prune", "version": 1, "rows": [{"session_id": "s"}]});
        assert_eq!(
            prune_answer(PruneKind::Sessions, true, report.clone(), vec![]),
            json!({"kind": "sessions", "dry_run": true, "would_remove": [{"session_id": "s"}], "errors": []})
        );
        assert_eq!(
            prune_answer(PruneKind::Sessions, false, report, vec!["x".into()]),
            json!({"kind": "sessions", "dry_run": false, "removed": [{"session_id": "s"}], "errors": ["x"]})
        );
    }
}
