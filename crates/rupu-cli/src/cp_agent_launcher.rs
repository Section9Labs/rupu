//! `cp serve` adapter for rupu-cp's `AgentLauncher`. Spawns a detached
//! `rupu run <agent> …` child per request (`rupu_runtime::spawn`).
use rupu_cp::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
use rupu_runtime::argv::RunArgv;
use rupu_runtime::spawn::{spawn_detached, SpawnSpec};
use std::path::PathBuf;

pub struct SubprocessAgentLauncher {
    pub exe: PathBuf,
}

/// The detached `rupu run` for `req`: its own process group + null stdio,
/// so a Ctrl-C / SIGINT to `cp serve` (or the CP exiting) does not take the
/// run down. The child writes its own transcript and run.json lifecycle.
/// It runs under the caller's run id and codename when given (a placed
/// unit's coordinator), else a minted id and its own crew.
fn spawn_spec(
    exe: &std::path::Path,
    req: &AgentLaunchRequest,
) -> Result<(SpawnSpec, String), AgentLaunchError> {
    let run_id = req.run_id_or_mint().map_err(AgentLaunchError::Invalid)?;
    let run = req.agent_run(&run_id).map_err(AgentLaunchError::Invalid)?;
    let mut spec = SpawnSpec::new(exe, RunArgv::Agent(run));
    spec.cwd = req.working_dir.as_deref().map(PathBuf::from);
    Ok((spec, run_id))
}

#[async_trait::async_trait]
impl AgentLauncher for SubprocessAgentLauncher {
    async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
        let (spec, run_id) = spawn_spec(&self.exe, &req)?;
        spawn_detached(spec).map_err(|e| AgentLaunchError::Spawn(e.to_string()))?;
        Ok(run_id)
    }

    fn honours_supplied_run_id(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::spawn_spec;
    use rupu_cp::agent_launcher::{AgentLaunchError, AgentLaunchRequest};
    use rupu_runtime::argv::RunArgv;

    fn req() -> AgentLaunchRequest {
        AgentLaunchRequest {
            model: None,
            provider: None,
            continue_from: None,
            codename: None,
            agent: "triage".into(),
            prompt: Some("look at PR".into()),
            mode: Some("bypass".into()),
            target: Some("github:o/r".into()),
            working_dir: Some("/tmp/p".into()),
            run_id: None,
            findings_profile: None,
            engagement_profiles: Vec::new(),
            skills: Vec::new(),
            preload_skills: Vec::new(),
            skill_bundle: None,
            messaging: Default::default(),
        }
    }

    fn args(spec: &rupu_runtime::spawn::SpawnSpec) -> Vec<String> {
        spec.argv
            .to_args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_targeted_launch_clones_into_a_tmpdir_and_runs_in_the_working_dir() {
        let (spec, run_id) = spawn_spec("/bin/rupu".as_ref(), &req()).unwrap();
        assert!(run_id.starts_with("run_"), "{run_id}");
        assert_eq!(
            args(&spec),
            [
                "run",
                "triage",
                "github:o/r",
                "--run-id",
                &run_id,
                "--mode",
                "bypass",
                "--prompt=look at PR",
                "--tmp"
            ]
        );
        assert_eq!(spec.cwd.as_deref(), Some("/tmp/p".as_ref()));
        assert!(!spec.keep_child);
    }

    /// L8: the local CP launcher used to mint its own id and drop the
    /// codename. A coordinator's are the run's now.
    #[test]
    fn a_supplied_run_id_and_codename_are_honoured() {
        let mut r = req();
        r.run_id = Some("run_01COORD".into());
        r.codename = Some("cobalt-harbor/heron#412".into());
        let (spec, run_id) = spawn_spec("/bin/rupu".as_ref(), &r).unwrap();
        assert_eq!(run_id, "run_01COORD");
        let RunArgv::Agent(run) = &spec.argv else {
            panic!()
        };
        assert_eq!(run.run_id, "run_01COORD");
        assert_eq!(run.codename.as_deref(), Some("cobalt-harbor/heron#412"));
        assert!(args(&spec).contains(&"--codename".to_string()));
    }

    /// A placed unit's step skills reach the child's `rupu run`.
    #[test]
    fn the_units_skills_are_on_the_argv() {
        let mut r = req();
        r.target = None;
        r.skills = vec!["pdf".into(), "project:notes".into()];
        r.preload_skills = vec!["house-style".into()];
        let (spec, run_id) = spawn_spec("/bin/rupu".as_ref(), &r).unwrap();
        assert_eq!(
            args(&spec),
            [
                "run",
                "triage",
                "--run-id",
                &run_id,
                "--mode",
                "bypass",
                "--skill",
                "pdf",
                "--skill",
                "project:notes",
                "--preload-skill",
                "house-style",
                "--prompt=look at PR",
            ]
        );
    }

    /// A placed unit's workflow `messaging:` switch is on the argv: `rupu
    /// run --no-messaging` or `--messaging-tool <name>` each.
    #[test]
    fn a_units_messaging_is_on_the_argv() {
        use rupu_runtime::argv::LaunchMessaging;
        for (messaging, flags) in [
            (LaunchMessaging::Off, vec!["--no-messaging"]),
            (
                LaunchMessaging::Tools(vec!["msg.send".into()]),
                vec!["--messaging-tool", "msg.send"],
            ),
        ] {
            let mut r = req();
            r.target = None;
            r.messaging = messaging;
            let (spec, run_id) = spawn_spec("/bin/rupu".as_ref(), &r).unwrap();
            let want: Vec<&str> = [
                "run",
                "triage",
                "--run-id",
                &run_id,
                "--mode",
                "bypass",
                "--prompt=look at PR",
            ]
            .into_iter()
            .chain(flags)
            .collect();
            assert_eq!(args(&spec), want);
        }
    }

    /// A CP continue: `rupu run <agent> --continue <id>` under a new run
    /// id, in the continued run's directory, with only the overrides it
    /// names — the child restores the rest from the run's own command.
    #[test]
    fn a_continue_is_rupu_run_continue_with_its_overrides() {
        let mut r = req();
        r.prompt = None;
        r.mode = None;
        r.target = None;
        r.continue_from = Some("sub_01PREV".into());
        r.model = Some("claude-opus-5-5".into());
        r.provider = Some("anthropic".into());
        let (spec, run_id) = spawn_spec("/bin/rupu".as_ref(), &r).unwrap();
        assert_eq!(
            args(&spec),
            [
                "run",
                "triage",
                "--run-id",
                &run_id,
                "--continue",
                "sub_01PREV",
                "--model=claude-opus-5-5",
                "--provider=anthropic",
            ]
        );
        assert_eq!(spec.cwd.as_deref(), Some("/tmp/p".as_ref()));
    }

    /// A CP continue with a prompt: the prompt rides as ONE `--prompt=`
    /// element next to `--continue`, so the child tells the continuation it.
    #[test]
    fn a_continue_with_a_prompt_carries_it() {
        let mut r = req();
        r.prompt = Some("-x and then?".into());
        r.mode = None;
        r.target = None;
        r.continue_from = Some("run_01PREV".into());
        let (spec, run_id) = spawn_spec("/bin/rupu".as_ref(), &r).unwrap();
        assert_eq!(
            args(&spec),
            [
                "run",
                "triage",
                "--run-id",
                &run_id,
                "--prompt=-x and then?",
                "--continue",
                "run_01PREV",
            ]
        );
    }

    #[test]
    fn a_malformed_run_id_or_mode_is_refused() {
        let mut bad_id = req();
        bad_id.run_id = Some("../x".into());
        let mut bad_mode = req();
        bad_mode.mode = Some("yolo".into());
        for r in [bad_id, bad_mode] {
            assert!(matches!(
                spawn_spec("/bin/rupu".as_ref(), &r),
                Err(AgentLaunchError::Invalid(_))
            ));
        }
    }
}
