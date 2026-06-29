//! `cp serve` adapter for rupu-cp's `SessionStarter` port. Spawns
//! `rupu session start … --detach`, which enqueues the first turn + spawns the
//! session worker and prints `session: <id>`; we parse that and return it.
use rupu_cp::session_starter::{SessionStartError, SessionStartRequest, SessionStarter};
use std::path::PathBuf;

/// Spawns `rupu session start …` children. `exe` is the path to the running
/// `rupu` binary (resolved via `std::env::current_exe()` in `cp serve`).
pub struct SubprocessSessionStarter {
    pub exe: PathBuf,
}

/// Build the argv (after the executable) for a `rupu session start` invocation.
///
/// Order: `session start <agent> [<target>] --detach [--mode m] [--prompt p]
/// [--into <clone_dir>]`. `--into` is added only when a repo target AND a clone
/// dir are present.
pub(crate) fn build_session_start_argv(
    req: &SessionStartRequest,
    clone_dir: Option<&str>,
) -> Vec<String> {
    let mut argv = vec![
        "session".to_string(),
        "start".to_string(),
        req.agent.clone(),
    ];
    if let Some(t) = &req.target {
        argv.push(t.clone());
    }
    argv.push("--detach".to_string());
    if let Some(m) = &req.mode {
        argv.push("--mode".to_string());
        argv.push(m.clone());
    }
    if let Some(p) = &req.prompt {
        argv.push("--prompt".to_string());
        argv.push(p.clone());
    }
    if req.target.is_some() {
        if let Some(dir) = clone_dir {
            argv.push("--into".to_string());
            argv.push(dir.to_string());
        }
    }
    argv
}

/// Scan `session start --detach` stdout for the `session: <id>` line and return
/// the session id. Matches the CLI's printout (`println!("session: {id}")`).
pub(crate) fn parse_session_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(rest) = line.trim().strip_prefix("session:") {
            let id = rest.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

#[async_trait::async_trait]
impl SessionStarter for SubprocessSessionStarter {
    async fn start(&self, req: SessionStartRequest) -> Result<String, SessionStartError> {
        // A repo target with no explicit working_dir needs a persistent clone
        // dir (the session lives on after start). Create it under the global
        // rupu dir so it survives across cp-serve restarts.
        let clone_dir = if req.target.is_some() && req.working_dir.is_none() {
            let base = crate::paths::global_dir()
                .map_err(|e| SessionStartError::Spawn(e.to_string()))?
                .join("clones")
                .join(ulid::Ulid::new().to_string());
            std::fs::create_dir_all(&base).map_err(|e| SessionStartError::Spawn(e.to_string()))?;
            Some(base.to_string_lossy().into_owned())
        } else {
            None
        };

        let argv = build_session_start_argv(&req, clone_dir.as_deref());

        let mut cmd = tokio::process::Command::new(&self.exe);
        cmd.args(&argv);
        if let Some(dir) = req.working_dir.as_deref() {
            cmd.current_dir(dir);
        }

        let out = cmd
            .output()
            .await
            .map_err(|e| SessionStartError::Spawn(e.to_string()))?;

        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(SessionStartError::Spawn(if err.is_empty() {
                "session start failed".into()
            } else {
                err
            }));
        }

        parse_session_id(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
            SessionStartError::Spawn("could not determine session id from output".into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{build_session_start_argv, parse_session_id};
    use rupu_cp::session_starter::SessionStartRequest;

    fn req(
        target: Option<&str>,
        prompt: Option<&str>,
        mode: Option<&str>,
        wd: Option<&str>,
    ) -> SessionStartRequest {
        SessionStartRequest {
            agent: "triage".into(),
            prompt: prompt.map(Into::into),
            mode: mode.map(Into::into),
            target: target.map(Into::into),
            working_dir: wd.map(Into::into),
        }
    }

    #[test]
    fn argv_workspace_prompt_mode() {
        let a = build_session_start_argv(&req(None, Some("hi"), Some("ask"), None), None);
        assert_eq!(
            a,
            vec!["session", "start", "triage", "--detach", "--mode", "ask", "--prompt", "hi"]
        );
    }

    #[test]
    fn argv_repo_adds_into() {
        let a = build_session_start_argv(
            &req(Some("github:o/r"), Some("hi"), None, None),
            Some("/clones/x"),
        );
        assert_eq!(
            a,
            vec![
                "session",
                "start",
                "triage",
                "github:o/r",
                "--detach",
                "--prompt",
                "hi",
                "--into",
                "/clones/x",
            ]
        );
    }

    #[test]
    fn argv_minimal() {
        assert_eq!(
            build_session_start_argv(&req(None, None, None, None), None),
            vec!["session", "start", "triage", "--detach"]
        );
    }

    #[test]
    fn parse_session_id_finds_line() {
        assert_eq!(
            parse_session_id("session: ses_01XYZ\nrun: run_1\n"),
            Some("ses_01XYZ".into())
        );
        assert_eq!(parse_session_id("run: run_1\n"), None);
    }
}
