//! `cp serve` adapter for rupu-cp's `AgentLauncher`. Spawns a detached
//! `rupu run <agent> …` child per request (own process group + null stdio).
use rupu_cp::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;

pub struct SubprocessAgentLauncher {
    pub exe: PathBuf,
}

/// Build the argv (after the executable) for a `rupu run` invocation.
///
/// Order: `run <agent> [<target>] --run-id <id> [--mode m] [--prompt <p>] [--tmp]`.
/// The prompt is always passed via `--prompt` (never positionally) so it cannot
/// be mis-parsed as a RunTarget when no target is present.
/// `--tmp` is added when a target is present so a repo/PR clone lands in an
/// auto-deleted tmpdir instead of polluting / refusing in cwd.
pub(crate) fn build_agent_argv(req: &AgentLaunchRequest, run_id: &str) -> Vec<String> {
    let mut argv = vec!["run".to_string(), req.agent.clone()];
    if let Some(t) = &req.target {
        argv.push(t.clone());
    }
    argv.push("--run-id".to_string());
    argv.push(run_id.to_string());
    if let Some(m) = &req.mode {
        argv.push("--mode".to_string());
        argv.push(m.clone());
    }
    if let Some(p) = &req.prompt {
        argv.push("--prompt".to_string());
        argv.push(p.clone());
    }
    if req.target.is_some() {
        argv.push("--tmp".to_string());
    }
    argv
}

#[async_trait::async_trait]
impl AgentLauncher for SubprocessAgentLauncher {
    async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
        let run_id = format!("run_{}", ulid::Ulid::new());
        let argv = build_agent_argv(&req, &run_id);
        // Detached: its own process group + null stdio, so a Ctrl-C / SIGINT to
        // `cp serve` (or the CP exiting) does not take the run down. The child
        // writes its own transcript and run.json lifecycle.
        let mut cmd = std::process::Command::new(&self.exe);
        cmd.args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(dir) = req.working_dir.as_deref() {
            cmd.current_dir(dir);
        }
        #[cfg(unix)]
        cmd.process_group(0); // own process group; detaches from cp-serve's
        cmd.spawn()
            .map_err(|e| AgentLaunchError::Spawn(e.to_string()))?;
        Ok(run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::build_agent_argv;
    use rupu_cp::agent_launcher::AgentLaunchRequest;

    #[test]
    fn argv_with_target_prompt_mode() {
        let req = AgentLaunchRequest {
            agent: "triage".into(),
            prompt: Some("look at PR".into()),
            mode: Some("bypass".into()),
            target: Some("github:o/r".into()),
            working_dir: None,
        };
        let argv = build_agent_argv(&req, "run_X");
        assert_eq!(
            argv,
            vec![
                "run",
                "triage",
                "github:o/r",
                "--run-id",
                "run_X",
                "--mode",
                "bypass",
                "--prompt",
                "look at PR",
                "--tmp",
            ]
        );
    }

    #[test]
    fn argv_minimal() {
        let req = AgentLaunchRequest {
            agent: "triage".into(),
            prompt: None,
            mode: None,
            target: None,
            working_dir: None,
        };
        assert_eq!(
            build_agent_argv(&req, "run_X"),
            vec!["run", "triage", "--run-id", "run_X"]
        );
    }

    #[test]
    fn argv_prompt_no_target() {
        let req = AgentLaunchRequest {
            agent: "triage".into(),
            prompt: Some("do a security audit".into()),
            mode: None,
            target: None,
            working_dir: None,
        };
        assert_eq!(
            build_agent_argv(&req, "run_X"),
            vec![
                "run",
                "triage",
                "--run-id",
                "run_X",
                "--prompt",
                "do a security audit"
            ]
        );
    }
}
