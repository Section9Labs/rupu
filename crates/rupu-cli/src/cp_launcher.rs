//! `SubprocessLauncher` — the `cp serve` adapter for rupu-cp's [`RunLauncher`]
//! port. It spawns a detached `rupu workflow run …` child process per launch
//! request; the child owns its own run.json / events.jsonl lifecycle. The
//! launcher returns the `run_<ULID>` id it minted (and passed via `--run-id`)
//! so the web UI can navigate to the run immediately.

use rupu_cp::launcher::{LaunchError, LaunchRequest, RunLauncher};
use std::path::PathBuf;
use std::process::Stdio;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Spawns `rupu workflow run …` children. `exe` is the path to the running
/// `rupu` binary (resolved via `std::env::current_exe()` in `cp serve`).
pub struct SubprocessLauncher {
    pub exe: PathBuf,
}

/// Build the argv (after the executable) for a `rupu workflow run` invocation.
///
/// Order: `workflow run <name> [<target>] --run-id <id> --plain
/// [--input k=v]… [--mode m]`. `inputs` iterate in the `BTreeMap`'s sorted
/// key order.
fn build_run_argv(req: &LaunchRequest, run_id: &str) -> Vec<String> {
    let mut argv = vec![
        "workflow".to_string(),
        "run".to_string(),
        req.workflow.clone(),
    ];
    if let Some(target) = &req.target {
        argv.push(target.clone());
    }
    argv.push("--run-id".to_string());
    argv.push(run_id.to_string());
    argv.push("--plain".to_string());
    for (k, v) in &req.inputs {
        argv.push("--input".to_string());
        argv.push(format!("{k}={v}"));
    }
    if let Some(mode) = &req.mode {
        argv.push("--mode".to_string());
        argv.push(mode.clone());
    }
    argv
}

#[async_trait::async_trait]
impl RunLauncher for SubprocessLauncher {
    async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError> {
        let run_id = format!("run_{}", ulid::Ulid::new());
        let argv = build_run_argv(&req, &run_id);
        // Detached: its own process group + null stdio, so a Ctrl-C / SIGINT to
        // `cp serve` (or the CP exiting) does not take the run down. The child
        // writes its own run.json / events.jsonl / transcripts.
        let mut cmd = std::process::Command::new(&self.exe);
        cmd.args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(dir) = req.working_dir.as_deref() {
            cmd.current_dir(dir);
        }
        #[cfg(unix)]
        cmd.process_group(0); // own process group (not a full session/setsid); detaches from cp-serve's
        cmd.spawn().map_err(|e| LaunchError::Spawn(e.to_string()))?;
        Ok(run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn argv_full_request_sorted_inputs() {
        let mut inputs = BTreeMap::new();
        inputs.insert("k".to_string(), "v".to_string());
        inputs.insert("a".to_string(), "b".to_string());
        let req = LaunchRequest {
            workflow: "audit".to_string(),
            inputs,
            mode: Some("bypass".to_string()),
            target: Some("github:o/r".to_string()),
            working_dir: None,
        };
        let argv = build_run_argv(&req, "run_X");
        assert_eq!(
            argv,
            vec![
                "workflow",
                "run",
                "audit",
                "github:o/r",
                "--run-id",
                "run_X",
                "--plain",
                // BTreeMap sorts keys: a before k.
                "--input",
                "a=b",
                "--input",
                "k=v",
                "--mode",
                "bypass",
            ]
        );
    }

    #[test]
    fn argv_minimal_request() {
        let req = LaunchRequest {
            workflow: "audit".to_string(),
            inputs: BTreeMap::new(),
            mode: None,
            target: None,
            working_dir: None,
        };
        let argv = build_run_argv(&req, "run_X");
        assert_eq!(
            argv,
            vec!["workflow", "run", "audit", "--run-id", "run_X", "--plain"]
        );
    }
}
