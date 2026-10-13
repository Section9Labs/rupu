//! `cp serve` adapter for rupu-cp's `SessionStarter` port. Spawns
//! `rupu session start … --detach`, which enqueues the first turn + spawns the
//! session worker and prints `session: <id>`; we parse that and return it.
use rupu_cp::session_starter::{SessionStartError, SessionStartRequest, SessionStarter};
use rupu_runtime::argv::{RunArgv, SessionStart};
use std::path::PathBuf;

/// Spawns `rupu session start …` children. `exe` is the path to the running
/// `rupu` binary (resolved via `std::env::current_exe()` in `cp serve`).
pub struct SubprocessSessionStarter {
    pub exe: PathBuf,
}

/// The `rupu session start --detach` for `req`. `--into` is added only when
/// a repo target AND a clone dir are present. A mode that isn't `ask` /
/// `bypass` / `readonly` is refused here, not by the child.
pub(crate) fn session_start_argv(
    req: &SessionStartRequest,
    clone_dir: Option<&str>,
) -> Result<RunArgv, String> {
    let mode = req
        .mode
        .as_deref()
        .map(rupu_tools::PermissionMode::parse)
        .transpose()
        .map_err(|e| e.to_string())?;
    Ok(RunArgv::SessionStart(SessionStart {
        agent: req.agent.clone(),
        target: req.target.clone(),
        mode,
        prompt: req.prompt.clone(),
        into: req
            .target
            .as_ref()
            .and(clone_dir)
            .map(std::path::PathBuf::from),
        detach: true,
        skills: req.skills.clone(),
        preload_skills: req.preload_skills.clone(),
    }))
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
            std::fs::create_dir_all(&base)
                .map_err(|e| SessionStartError::Spawn(e.to_string()))?;
            Some(base.to_string_lossy().into_owned())
        } else {
            None
        };

        let argv =
            session_start_argv(&req, clone_dir.as_deref()).map_err(SessionStartError::Invalid)?;

        let mut cmd = tokio::process::Command::new(&self.exe);
        cmd.args(argv.to_args());
        if let Some(dir) = req.working_dir.as_deref() {
            cmd.current_dir(dir);
        }

        let out = cmd
            .output()
            .await
            .map_err(|e| SessionStartError::Spawn(e.to_string()))?;

        if !out.status.success() {
            let msg = crate::cp_session_sender::cli_failure(&out.stderr, "session start failed");
            // Exit 1 is the command refusing the request (a skill that
            // doesn't resolve, an unknown agent): the caller's to fix.
            return Err(if out.status.code() == Some(1) {
                SessionStartError::Invalid(msg)
            } else {
                SessionStartError::Spawn(msg)
            });
        }

        parse_session_id(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| SessionStartError::Spawn("could not determine session id from output".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_session_id, session_start_argv, SubprocessSessionStarter};
    use rupu_cp::session_starter::{SessionStartError, SessionStartRequest, SessionStarter};

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
            skills: Vec::new(),
            preload_skills: Vec::new(),
        }
    }

    fn argv(r: &SessionStartRequest, clone_dir: Option<&str>) -> Vec<String> {
        session_start_argv(r, clone_dir)
            .unwrap()
            .to_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn argv_workspace_prompt_mode() {
        assert_eq!(
            argv(&req(None, Some("hi"), Some("ask"), None), None),
            [
                "session",
                "start",
                "triage",
                "--detach",
                "--mode",
                "ask",
                "--prompt=hi"
            ]
        );
    }

    #[test]
    fn argv_repo_adds_into() {
        assert_eq!(
            argv(
                &req(Some("github:o/r"), Some("hi"), None, None),
                Some("/clones/x")
            ),
            [
                "session",
                "start",
                "triage",
                "github:o/r",
                "--detach",
                "--prompt=hi",
                "--into",
                "/clones/x",
            ]
        );
        // No target: no clone, whatever the dir.
        assert_eq!(
            argv(&req(None, None, None, None), Some("/clones/x")),
            ["session", "start", "triage", "--detach"]
        );
    }

    #[test]
    fn argv_minimal() {
        assert_eq!(
            argv(&req(None, None, None, None), None),
            ["session", "start", "triage", "--detach"]
        );
    }

    #[test]
    fn argv_carries_the_skills() {
        let mut r = req(None, Some("hi"), None, None);
        r.skills = vec!["pdf".into()];
        r.preload_skills = vec!["house-style".into()];
        assert_eq!(
            argv(&r, None),
            [
                "session",
                "start",
                "triage",
                "--detach",
                "--prompt=hi",
                "--skill",
                "pdf",
                "--preload-skill",
                "house-style",
            ]
        );
    }

    /// `session start --skill` exits 1 when a skill doesn't resolve: the
    /// request's fault, so it is `Invalid` (a 400) with the CLI's message.
    #[tokio::test]
    async fn a_refused_skill_is_invalid_with_the_clis_message() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let skill = global.join("skills").join("pdf");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: pdf\ndescription: The pdf skill.\n---\nRead PDFs.\n",
        )
        .unwrap();
        let agent = rupu_agent::AgentSpec::parse(
            "---\nname: triage\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nTriage.\n",
        )
        .unwrap();
        let refusal = rupu_runtime::assembly::resolve_agent_skills(
            &rupu_skills::LayerDirs::for_layers(&global, None),
            &agent,
            &rupu_runtime::assembly::Overrides {
                skills: vec!["pdff".into()],
                ..Default::default()
            },
            rupu_skills::SkillReason::Session,
        )
        .await
        .expect_err("a typo is refused");
        let message = format!("[error] {refusal}");
        assert!(
            message.contains("unknown skill \"pdff\" (did you mean pdf?)"),
            "{message}"
        );

        let starter = SubprocessSessionStarter {
            exe: crate::cp_session_sender::tests::failing_rupu(tmp.path(), &format!("{message}\n")),
        };
        let mut r = req(None, None, None, Some(tmp.path().to_str().unwrap()));
        r.skills = vec!["pdff".into()];
        match starter.start(r).await {
            Err(SessionStartError::Invalid(m)) => assert_eq!(m, refusal.to_string()),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_mode_is_refused() {
        assert!(session_start_argv(&req(None, None, Some("yolo"), None), None).is_err());
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
