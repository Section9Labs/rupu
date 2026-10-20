//! `cp serve` adapter for rupu-cp's `AgentiflowLauncher`: the detached start
//! `rupu agentiflow run --detach` runs (`cmd::agentiflow::start_detached`),
//! with this binary as the child.
use crate::cmd::agentiflow::{start_detached, DetachError, DetachedStart};
use rupu_cp::agentiflow_launcher::{
    AgentiflowLaunchError, AgentiflowLaunchRequest, AgentiflowLauncher, AgentiflowStart,
};
use std::path::PathBuf;

pub struct SubprocessAgentiflowLauncher {
    pub exe: PathBuf,
    /// The `RUPU_HOME` the child records the run under: the one `cp serve`
    /// serves.
    pub global_dir: PathBuf,
}

#[async_trait::async_trait]
impl AgentiflowLauncher for SubprocessAgentiflowLauncher {
    async fn launch(
        &self,
        req: AgentiflowLaunchRequest,
    ) -> Result<AgentiflowStart, AgentiflowLaunchError> {
        let started = start_detached(
            &self.exe,
            &req.definition,
            &req.run_id,
            &self.global_dir,
            Some(&req.working_dir),
        )
        .await
        .map_err(|e| match e {
            DetachError::Spawn(m) => AgentiflowLaunchError::Spawn(m),
            e @ DetachError::Exited { .. } => AgentiflowLaunchError::NotStarted(e.to_string()),
        })?;
        Ok(match started {
            DetachedStart::Started => AgentiflowStart::Started,
            DetachedStart::Pending { log } => AgentiflowStart::Pending { log },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::agentiflow::detached_spawn_spec;

    fn args(spec: &rupu_runtime::spawn::SpawnSpec) -> Vec<String> {
        spec.argv
            .to_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_child_is_the_foreground_run_under_the_minted_id_in_the_working_dir() {
        let spec = detached_spawn_spec(
            "/bin/rupu".as_ref(),
            "acme",
            "af_01CP",
            "/home/u/.rupu".as_ref(),
            Some("/work/repo".as_ref()),
        );
        assert_eq!(
            args(&spec),
            ["agentiflow", "run", "--run-id", "af_01CP", "--", "acme"]
        );
        assert_eq!(spec.cwd.as_deref(), Some("/work/repo".as_ref()));
        assert_eq!(
            spec.env,
            [(
                "RUPU_HOME".into(),
                std::ffi::OsString::from("/home/u/.rupu")
            )]
        );
        assert!(spec.keep_child);
        let rupu_runtime::spawn::SpawnStdio::Log(log) = &spec.stdio else {
            panic!("stderr goes to the run's detach log")
        };
        assert_eq!(
            log,
            &PathBuf::from("/home/u/.rupu/agentiflows/af_01CP/detach.log")
        );
    }

    #[tokio::test]
    async fn a_child_that_exits_before_starting_is_not_started_and_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let launcher = SubprocessAgentiflowLauncher {
            exe: "/usr/bin/false".into(),
            global_dir: tmp.path().to_path_buf(),
        };
        let err = launcher
            .launch(AgentiflowLaunchRequest {
                definition: "acme".into(),
                run_id: "af_01CPFAIL".into(),
                working_dir: tmp.path().to_path_buf(),
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, AgentiflowLaunchError::NotStarted(ref m) if m.contains("before it started")),
            "{err}"
        );
        assert!(!tmp.path().join("agentiflows/af_01CPFAIL").exists());
    }
}
