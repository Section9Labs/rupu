//! `cp serve` adapter for rupu-cp's `AdminOps` port: the operations whose
//! logic still lives in this crate. The cron tick runs the CLI's own tick
//! under the `RUPU_HOME` tick lock; an autoflow cycle is started as a
//! detached `rupu autoflow run` child (W6's `RunArgv`). The autoflow reads,
//! repair and the session prunes call the functions `rupu autoflow
//! show|explain|doctor|wakes|repair`, `rupu session prune` and `rupu
//! cleanup` run, and answer with the JSON report each command prints under
//! `--format json`, until their extraction into libraries.

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

    async fn cron_tick(&self, req: CronTickRequest) -> Result<CronTickOutcome, AdminOpError> {
        if req.dry_run {
            let mut found = Vec::new();
            crate::cmd::cron::tick(true, req.skip_events, req.only_events, &mut |fire| {
                found.push(fire)
            })
            .await
            .map_err(classify)?;
            return Ok(CronTickOutcome::DryRun(json!({
                "kind": "cron_tick",
                "version": 1,
                "dry_run": true,
                "would_fire": found,
            })));
        }
        let lock = rupu_orchestrator::cron_triggers::TickLock::try_acquire(&self.global)
            .map_err(|e| AdminOpError::Failed(format!("taking the cron tick lock: {e}")))?
            .ok_or_else(|| {
                AdminOpError::Conflict(
                    "a cron tick is already running; try again when it ends".into(),
                )
            })?;
        tokio::spawn(async move {
            let _lock = lock;
            if let Err(e) =
                crate::cmd::cron::tick(false, req.skip_events, req.only_events, &mut |_| {}).await
            {
                tracing::warn!(error = %e, "cp serve: requested cron tick failed");
            }
        });
        Ok(CronTickOutcome::Started)
    }

    async fn prune_sessions(&self, req: PruneRequest) -> Result<Value, AdminOpError> {
        blocking(move || {
            let older = req.older_than.as_deref();
            match req.kind {
                PruneKind::Sessions => Ok(serde_json::to_value(
                    crate::cmd::session::prune_report(older, req.dry_run)?,
                )?),
                PruneKind::Cleanup => crate::cmd::cleanup::cleanup_json(older, req.dry_run),
                other => Err(AdminOpError::Invalid(format!(
                    "the control plane prunes `{}` itself",
                    other.as_str()
                ))
                .into()),
            }
        })
        .await
    }
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

    /// A real tick while another holds the `RUPU_HOME` tick lock (`cp
    /// serve`'s loop, a crontab `rupu cron tick`) is a conflict, and starts
    /// nothing; a dry run takes no lock and still answers.
    #[tokio::test]
    async fn a_tick_while_the_lock_is_held_is_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let ops = CliAdminOps {
            exe: PathBuf::from("/bin/rupu"),
            global: tmp.path().to_path_buf(),
        };
        let held = rupu_orchestrator::cron_triggers::TickLock::try_acquire(tmp.path())
            .unwrap()
            .expect("the lock is free");
        let err = ops
            .cron_tick(CronTickRequest {
                dry_run: false,
                skip_events: true,
                only_events: false,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AdminOpError::Conflict(_)), "{err:?}");
        drop(held);
    }

    #[test]
    fn a_typed_refusal_survives_context() {
        let e = anyhow::Error::from(AdminOpError::NotFound("no claim".into())).context("repair");
        assert_eq!(classify(e), AdminOpError::NotFound("no claim".into()));
        let e = anyhow::anyhow!("disk on fire");
        assert_eq!(classify(e), AdminOpError::Failed("disk on fire".into()));
    }
}
