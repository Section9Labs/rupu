/// A request to start a fresh agent run.
#[derive(Debug, Clone)]
pub struct AgentLaunchRequest {
    pub agent: String,
    pub prompt: Option<String>,
    pub mode: Option<String>,
    pub target: Option<String>,
    /// Working directory for the run (project/dir target). When `None` the
    /// run executes in the cp-serve process's cwd.
    pub working_dir: Option<String>,
    /// Run id to execute under, when the caller already minted one (a
    /// placed unit's coordinator, see `UnitDispatch::run_id`). SSH, tunnel,
    /// bucket and the local launcher always honour it; an HTTP host does when
    /// it advertises `run.supplied_run_id`, and otherwise mints its own and
    /// returns that — which is what `HostConnector::honours_supplied_run_id`
    /// reports. `None` → the connector mints. See [`Self::run_id_or_mint`].
    pub run_id: Option<String>,
    /// Findings contract override for this run — `rupu run --findings-profile`,
    /// the highest-precedence input to the run's profile resolution. A placed
    /// unit's coordinator sets it from the step's `findings_profile` /
    /// the workflow's `defaults.findings_profile` (the part of the precedence
    /// chain only the coordinator knows); `None` ⇒ the executing host resolves
    /// from the agent file's `findingsProfile`, else `full`.
    ///
    /// Unlike `run_id`, EVERY connector honours this — local, HTTP, tunnel,
    /// bucket and SSH all put it on the `rupu run` argv (directly or via the
    /// peer that builds it). A connector that cannot deliver it to the host
    /// (e.g. a tunnel node too old to advertise support) must refuse the
    /// launch rather than run the agent under a different profile.
    pub findings_profile: Option<rupu_coverage::FindingProfile>,
    /// Engagement profile ids for this run — `rupu run --engagement-profile`,
    /// one flag per id; empty = the native `code` path. A placed unit's
    /// coordinator sets it from the workflow run's selection
    /// (`rupu workflow run --engagement-profile`); the executing host
    /// resolves the ids against its own profile registry (built-ins + its
    /// `<global>/profiles` + the run workspace's `.rupu/profiles`).
    ///
    /// Same contract as `findings_profile`: EVERY connector honours it, and
    /// one that cannot deliver it to the host (a peer that has not
    /// advertised `agent.engagement_profile`) refuses the launch rather than
    /// run the agent on the `code` path.
    pub engagement_profiles: Vec<String>,
    /// The placed unit's workflow-step skills — `rupu run --skill`, one flag
    /// per entry, on top of the agent's own. The executing host resolves
    /// them against its own skill layers. Same contract as
    /// `findings_profile`: every connector delivers them, or refuses the
    /// launch when its peer hasn't advertised `agent.skills`.
    pub skills: Vec<String>,
    /// The step's `preload_skills:` — `rupu run --preload-skill`. Same
    /// contract as `skills`.
    pub preload_skills: Vec<String>,
    /// The skills the coordinator shipped for this unit: `rupu run
    /// --skill-bundle`, resolved on the host against its skill cache. SSH
    /// sends it in the argv behind an in-command `agent.skill_bundle` gate,
    /// HTTP in the body to a peer advertising that feature, and the local
    /// connector forwards it; tunnel and bucket refuse a launch carrying one.
    pub skill_bundle: Option<rupu_skills::ship::SkillBundleManifest>,
    /// The run, and everything it launches, gets no messaging tools: `rupu
    /// run --no-messaging`. A placed unit's coordinator sets it for a
    /// workflow whose messaging is off, or for a run launched under that
    /// cap. Same contract as `skills`: every connector delivers it, or
    /// refuses the launch when its peer hasn't advertised
    /// `agent.no_messaging` (SSH gates it in the launch command).
    pub messaging_off: bool,
    /// Codename minted by a placed unit's coordinator — `rupu run
    /// --codename`. Best-effort: a codename is only a display name, so a peer
    /// that can't take it never blocks the launch. An SSH remote predating
    /// the flag gets it as `RUPU_CODENAME`; a node or HTTP host that hasn't
    /// advertised `run.codename` doesn't get it, the connector logs that
    /// (`RunArgv::for_peer`'s `Downgrade`), and the coordinator's records
    /// keep the name either way.
    pub codename: Option<String>,
}

impl AgentLaunchRequest {
    /// The id this launch runs under: the caller's, when it is a valid run
    /// id (it names a run-store directory and is interpolated into SSH
    /// commands), else a fresh `run_<ULID>`. A malformed supplied id is an
    /// error, never silently replaced.
    pub fn run_id_or_mint(&self) -> Result<String, String> {
        match self.run_id.as_deref() {
            Some(id) if crate::host::connector::valid_run_id(id) => Ok(id.to_string()),
            Some(id) => Err(format!("supplied run id {id:?} is not a valid run id")),
            None => Ok(format!("run_{}", ulid::Ulid::new())),
        }
    }

    /// The `rupu run` this request asks for, under `run_id`. `--tmp` whenever
    /// there is a target, so a repo/PR clone lands in an auto-deleted tmpdir
    /// instead of polluting (or being refused in) the cwd. A mode that isn't
    /// `ask` / `bypass` / `readonly` is refused here, not by the child. A
    /// codename that isn't one is dropped (logged) rather than handed to a
    /// child whose `--codename` would refuse it: a codename never blocks a
    /// launch.
    pub fn agent_run(&self, run_id: &str) -> Result<rupu_runtime::argv::AgentRun, String> {
        let mut run = rupu_runtime::argv::AgentRun::new(&self.agent, run_id);
        run.target = self.target.clone();
        run.codename = self.codename.clone().filter(|c| {
            let ok = c
                .parse::<rupu_codename::Codename>()
                .is_ok_and(|c| !c.segments.is_empty());
            if !ok {
                tracing::warn!(run_id, codename = %c, "not a run codename; launching without it");
            }
            ok
        });
        run.mode = parse_mode(self.mode.as_deref())?;
        run.prompt = self.prompt.clone();
        run.findings_profile = self.findings_profile;
        run.engagement_profiles = self.engagement_profiles.clone();
        run.skills = self.skills.clone();
        run.preload_skills = self.preload_skills.clone();
        run.skill_bundle = self
            .skill_bundle
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| format!("skill bundle: {e}"))?;
        run.tmp_clone = self.target.is_some();
        run.messaging_off = self.messaging_off;
        Ok(run)
    }
}

/// A launch request's `mode` word, through the one mode parser.
pub(crate) fn parse_mode(mode: Option<&str>) -> Result<Option<rupu_tools::PermissionMode>, String> {
    mode.map(rupu_tools::PermissionMode::parse)
        .transpose()
        .map_err(|e| e.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum AgentLaunchError {
    #[error("invalid launch request: {0}")]
    Invalid(String),
    #[error("failed to start run: {0}")]
    Spawn(String),
}

/// Port: starts agent runs. rupu-cp defines it; rupu-cli's `cp serve` provides
/// the subprocess-spawning adapter. Returns the new run id.
#[async_trait::async_trait]
pub trait AgentLauncher: Send + Sync {
    async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError>;

    /// Whether [`Self::launch`] runs under a supplied
    /// [`AgentLaunchRequest::run_id`] rather than minting its own — the
    /// local connector's answer to `HostConnector::honours_supplied_run_id`.
    fn honours_supplied_run_id(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> AgentLaunchRequest {
        AgentLaunchRequest {
            agent: "recon".into(),
            prompt: None,
            mode: Some("bypass".into()),
            target: Some("github:o/r".into()),
            working_dir: None,
            run_id: Some("run_01COORD".into()),
            findings_profile: None,
            engagement_profiles: vec!["network".into()],
            skills: vec!["pdf".into(), "sql".into()],
            preload_skills: vec!["house-style".into()],
            skill_bundle: None,
            messaging_off: true,
            codename: Some("cobalt-harbor/heron#412".into()),
        }
    }

    #[test]
    fn a_request_is_its_rupu_run() {
        let r = req();
        let id = r.run_id_or_mint().unwrap();
        assert_eq!(id, "run_01COORD");
        let run = r.agent_run(&id).unwrap();
        assert_eq!(run.codename.as_deref(), Some("cobalt-harbor/heron#412"));
        assert_eq!(run.mode, Some(rupu_tools::PermissionMode::Bypass));
        assert!(run.tmp_clone, "a target clones into a tmpdir");
        assert_eq!(run.engagement_profiles, ["network"]);
        assert_eq!(run.skills, ["pdf", "sql"]);
        assert_eq!(run.preload_skills, ["house-style"]);
        assert!(run.messaging_off);
        let args: Vec<String> = rupu_runtime::argv::RunArgv::Agent(run)
            .to_args()
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        let skill_args: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "--skill" || w[0] == "--preload-skill")
            .flat_map(|w| [w[0].as_str(), w[1].as_str()])
            .collect();
        assert_eq!(
            skill_args,
            [
                "--skill",
                "pdf",
                "--skill",
                "sql",
                "--preload-skill",
                "house-style"
            ]
        );
        assert_eq!(args.iter().filter(|a| *a == "--no-messaging").count(), 1);
        let mut on = req();
        on.messaging_off = false;
        let args = rupu_runtime::argv::RunArgv::Agent(on.agent_run(&id).unwrap()).to_args();
        assert!(!args.iter().any(|a| a == "--no-messaging"));
    }

    #[test]
    fn a_bad_id_or_mode_is_refused_but_a_bad_codename_is_only_dropped() {
        let mut r = req();
        r.run_id = Some("run_../x".into());
        assert!(r.run_id_or_mint().is_err());
        let mut r = req();
        r.run_id = None;
        assert!(r.run_id_or_mint().unwrap().starts_with("run_"));
        r.mode = Some("yolo".into());
        assert!(r.agent_run("run_1").is_err());
        for junk in ["not a codename", "cobalt-harbor"] {
            let mut r = req();
            r.codename = Some(junk.into());
            assert_eq!(r.agent_run("run_1").unwrap().codename, None, "{junk}");
        }
    }
}
