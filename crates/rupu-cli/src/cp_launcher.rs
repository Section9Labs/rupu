//! `SubprocessLauncher` — the `cp serve` adapter for rupu-cp's [`RunLauncher`]
//! port. It spawns a detached `rupu workflow run …` child process per launch
//! request (`rupu_runtime::spawn`); the child owns its own run.json /
//! events.jsonl lifecycle. The launcher returns the `run_<ULID>` id it minted
//! (and passed via `--run-id`) so the web UI can navigate to the run
//! immediately.

use rupu_cp::launcher::{LaunchError, LaunchRequest, LaunchWatch, RetryRequest, RunLauncher};
use rupu_runtime::argv::RunArgv;
use rupu_runtime::spawn::{spawn_detached, SpawnSpec, SpawnStdio};
use std::path::PathBuf;

/// Spawns `rupu workflow run …` children. `exe` is the path to the running
/// `rupu` binary (resolved via `std::env::current_exe()` in `cp serve`).
pub struct SubprocessLauncher {
    pub exe: PathBuf,
}

/// The detached `rupu workflow run --plain` for `req` under `run_id`: its own
/// process group + null stdio, so a Ctrl-C / SIGINT to `cp serve` (or the CP
/// exiting) does not take the run down.
fn spawn_spec(
    exe: &std::path::Path,
    req: &LaunchRequest,
    run_id: &str,
) -> Result<SpawnSpec, LaunchError> {
    let run = req.workflow_run(run_id).map_err(LaunchError::Invalid)?;
    let mut spec = SpawnSpec::new(exe, RunArgv::Workflow(run));
    spec.cwd = req.working_dir.as_deref().map(PathBuf::from);
    Ok(spec)
}

#[async_trait::async_trait]
impl RunLauncher for SubprocessLauncher {
    async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError> {
        let run_id = format!("run_{}", ulid::Ulid::new());
        spawn_detached(spawn_spec(&self.exe, &req, &run_id)?)
            .map_err(|e| LaunchError::Spawn(e.to_string()))?;
        Ok(run_id)
    }

    fn supports_session_workflows(&self) -> bool {
        true
    }

    async fn launch_identified(
        &self,
        req: LaunchRequest,
        run_id: &str,
        watch: LaunchWatch,
    ) -> Result<(), LaunchError> {
        let mut spec = spawn_spec(&self.exe, &req, run_id)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&watch.stderr)
            .map_err(|e| LaunchError::Spawn(e.to_string()))?;
        spec.stdio = SpawnStdio::Log(watch.stderr);
        spec.keep_child = true;
        let mut spawned = spawn_detached(spec).map_err(|e| LaunchError::Spawn(e.to_string()))?;
        let mut child = spawned.child.take().expect("requested retained child");
        let callback = watch.on_exit;
        let failed = callback.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("session-workflow-{}", spawned.pid))
            .spawn(move || {
                callback(child.wait().map_err(|e| e.to_string()));
            })
        {
            failed(Err(format!("could not observe workflow process: {error}")));
        }
        Ok(())
    }

    /// `rupu workflow resume <id> [--restart-interrupted]`, detached like a
    /// launch. It runs where the resume worker's children run: the resume
    /// reads the run's own record and snapshot, not the cwd.
    async fn retry(&self, req: RetryRequest) -> Result<(), LaunchError> {
        let mut spec = SpawnSpec::new(&self.exe, req.argv());
        spec.stdio = SpawnStdio::Log(req.log.clone());
        spawn_detached(spec).map_err(|e| LaunchError::Spawn(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn a_launch_is_a_plain_detached_workflow_run_with_sorted_inputs() {
        let mut inputs = BTreeMap::new();
        inputs.insert("k".to_string(), "v".to_string());
        inputs.insert("a".to_string(), "b".to_string());
        let req = LaunchRequest {
            engagement_profiles: Vec::new(),
            file: None,
            workflow: "audit".to_string(),
            inputs,
            mode: Some("bypass".to_string()),
            target: Some("github:o/r".to_string()),
            working_dir: Some("/tmp/p".into()),
        };
        let spec = spawn_spec("/bin/rupu".as_ref(), &req, "run_X").unwrap();
        let argv: Vec<String> = spec
            .argv
            .to_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            argv,
            [
                "workflow",
                "run",
                "audit",
                "github:o/r",
                "--run-id",
                "run_X",
                "--mode",
                "bypass",
                "--plain",
                "--input",
                "a=b",
                "--input",
                "k=v",
            ]
        );
        assert_eq!(spec.cwd.as_deref(), Some("/tmp/p".as_ref()));
    }

    #[test]
    fn engagement_profiles_and_a_file_are_on_the_argv() {
        let req = LaunchRequest {
            workflow: "ignored".to_string(),
            engagement_profiles: vec!["network".into(), "web".into()],
            file: Some("/g/cp/workflow-launches/w.yaml".into()),
            ..LaunchRequest::default()
        };
        let spec = spawn_spec("/bin/rupu".as_ref(), &req, "run_X").unwrap();
        let argv: Vec<String> = spec
            .argv
            .to_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            argv,
            [
                "workflow",
                "run",
                "--file",
                "/g/cp/workflow-launches/w.yaml",
                "--run-id",
                "run_X",
                "--plain",
                "--engagement-profile",
                "network",
                "--engagement-profile",
                "web",
            ]
        );
    }

    #[test]
    fn a_retry_is_an_operators_workflow_resume() {
        for (restart, want) in [
            (false, vec!["workflow", "resume", "run_X"]),
            (
                true,
                vec!["workflow", "resume", "run_X", "--restart-interrupted"],
            ),
        ] {
            let argv: Vec<String> = RetryRequest {
                run_id: "run_X".into(),
                restart_interrupted: restart,
                mode: None,
                log: "/g/runs/run_X/retry.log".into(),
            }
            .argv()
            .to_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
            assert_eq!(argv, want);
        }
    }

    #[test]
    fn a_retry_in_a_given_mode_passes_it() {
        let argv: Vec<String> = RetryRequest {
            run_id: "run_X".into(),
            restart_interrupted: false,
            mode: Some(rupu_tools::PermissionMode::Readonly),
            log: "/g/runs/run_X/retry.log".into(),
        }
        .argv()
        .to_args()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        assert_eq!(argv, ["workflow", "resume", "run_X", "--mode", "readonly"]);
    }

    #[test]
    fn an_unknown_mode_is_refused_before_spawning() {
        let req = LaunchRequest {
            workflow: "audit".to_string(),
            mode: Some("yolo".into()),
            ..LaunchRequest::default()
        };
        assert!(matches!(
            spawn_spec("/bin/rupu".as_ref(), &req, "run_X"),
            Err(LaunchError::Invalid(_))
        ));
    }
    #[tokio::test]
    async fn session_workflow_child_exit_reports_reserved_identity_and_startup_stderr() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("early-exit");
        std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$@\" >&2\nprintf 'invalid startup fixture\\n' >&2\nexit 7\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let stderr = temp.path().join("startup.log");
        let adapter = SubprocessLauncher { exe: executable };
        adapter
            .launch_identified(
                LaunchRequest {
                    workflow: "demo".into(),
                    ..Default::default()
                },
                "run_reserved",
                LaunchWatch {
                    stderr: stderr.clone(),
                    on_exit: std::sync::Arc::new(move |result| {
                        sender.send(result).unwrap();
                    }),
                },
            )
            .await
            .unwrap();
        let result = tokio::task::spawn_blocking(move || {
            receiver.recv_timeout(std::time::Duration::from_secs(5))
        })
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(result.code(), Some(7));
        let log = std::fs::read_to_string(&stderr).unwrap();
        assert!(log.contains("--run-id\nrun_reserved\n"), "{log}");
        assert!(log.contains("invalid startup fixture"));
        assert_eq!(
            std::fs::metadata(stderr).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
