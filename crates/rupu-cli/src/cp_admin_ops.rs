//! `cp serve` adapter for rupu-cp's `AdminOps` port: the two operations
//! that can't be a library call. The cron tick runs the CLI's own tick
//! (the workflow runtime) under the `RUPU_HOME` tick lock; an autoflow cycle
//! is started as a detached `rupu autoflow run` child (W6's `RunArgv`). The
//! control plane serves everything else from the libraries.

use std::path::PathBuf;

use rupu_cp::admin_ops::{
    AdminOpError, AdminOps, AutoflowRunRequest, CronTickOutcome, CronTickRequest,
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
    async fn autoflow_run(&self, req: AutoflowRunRequest) -> Result<Value, AdminOpError> {
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
            .map_err(|e| AdminOpError::Failed(format!("{e:#}")))?;
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
}
