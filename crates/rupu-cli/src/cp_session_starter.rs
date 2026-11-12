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
    #[cfg(unix)]
    pub approval_broker: Option<std::sync::Arc<rupu_runtime::session_approvals::broker::Broker>>,
}

/// The `rupu session start --detach` for `req`. `--into` is added only when
/// a repo target AND a clone dir are present. A mode that isn't `ask` /
/// `bypass` / `readonly` is refused here, not by the child.
pub(crate) fn session_start_argv(
    req: &SessionStartRequest,
    clone_dir: Option<&str>,
) -> Result<RunArgv, String> {
    rupu_runtime::session_settings::SessionSettings {
        tools: req.tools.clone(),
        ..Default::default()
    }
    .validate()
    .map_err(|e| e.to_string())?;
    if let Some(ceiling) = &req.tool_ceiling {
        rupu_tools::tool_ceiling::validate_names(
            ceiling,
            &rupu_tools::ToolCatalog::builtin(),
            req.mcp_tools.iter().flatten().map(|tool| tool.local_name()),
        )
        .map_err(|e| e.to_string())?;
    }
    if req.idle && (req.prompt.is_some() || req.target.is_some()) {
        return Err("idle session creation cannot include a prompt or target".into());
    }
    if let Some(preset) = req.skill_preset.as_deref() {
        if req.agent != rupu_agent::builtin::SESSION_AGENT_NAME
            || !rupu_runtime::session_skill_preset::valid_preset(preset)
        {
            return Err(
                "skill presets require rupu-session and a supported versioned preset or none"
                    .into(),
            );
        }
    }
    let mode = req
        .mode
        .as_deref()
        .map(rupu_tools::PermissionMode::parse)
        .transpose()
        .map_err(|e| e.to_string())?;
    Ok(RunArgv::SessionStart(SessionStart {
        tool_ceiling: req.tool_ceiling.clone(),
        mcp_tools: req.mcp_tools.clone(),
        mcp_catalog_revision: req.mcp_catalog_revision.clone(),
        effort: req.effort.clone(),
        skill_preset: req.skill_preset.clone(),
        tools: req.tools.clone(),
        agent: req.agent.clone(),
        provider: req.provider.clone(),
        model: req.model.clone(),
        workspace_id: req.workspace_id.clone(),
        idle: req.idle,
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
    #[cfg(unix)]
    fn approval_broker(
        &self,
    ) -> Option<std::sync::Arc<rupu_runtime::session_approvals::broker::Broker>> {
        self.approval_broker.clone()
    }
    fn supports_session_approvals(&self) -> bool {
        #[cfg(unix)]
        {
            self.approval_broker.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    fn supports_session_mcp(&self) -> bool {
        true
    }
    fn supports_session_scoped_reads(&self) -> bool {
        true
    }
    fn supports_session_tool_ceiling(&self) -> bool {
        true
    }
    fn supports_session_tool_selection(&self) -> bool {
        true
    }
    /// Exact pinned full-suite skill preset, independently gated from legacy v1.
    fn supports_full_skill_preset(&self) -> bool {
        true
    }
    fn supports_session_capabilities(&self) -> bool {
        true
    }
    fn supports_reasoning_effort(&self) -> bool {
        true
    }
    fn supports_builtin_harness(&self) -> bool {
        true
    }

    fn supports_idle_harness(&self) -> bool {
        true
    }

    fn supports_exact_global_conversation(&self) -> bool {
        cfg!(unix)
    }
    #[cfg(unix)]
    async fn start_exact_global(
        &self,
        req: SessionStartRequest,
        binding: rupu_runtime::exact_agent::Binding,
    ) -> Result<String, SessionStartError> {
        let proof = exact_session_proof(&req, &binding)?;
        let global =
            crate::paths::global_dir().map_err(|e| SessionStartError::Invalid(e.to_string()))?;
        rupu_runtime::exact_agent::load(
            &global,
            &binding,
            std::path::Path::new(&binding.project_path),
        )
        .map_err(|e| SessionStartError::Conflict(e.to_string()))?;
        self.start_inner(req, Some(proof)).await
    }
    async fn start(&self, req: SessionStartRequest) -> Result<String, SessionStartError> {
        self.start_inner(req, None).await
    }
}

#[cfg(unix)]
fn exact_session_proof(
    req: &SessionStartRequest,
    binding: &rupu_runtime::exact_agent::Binding,
) -> Result<String, SessionStartError> {
    binding
        .validate()
        .map_err(|e| SessionStartError::Invalid(e.to_string()))?;
    if req.agent != binding.effective_name
        || req.workspace_id.as_deref() != Some(&binding.workspace_id)
        || req.working_dir.as_deref() != Some(&binding.project_path)
        || req.target.is_some()
        || req.idle
        || !matches!(req.mode.as_deref(), Some("readonly" | "bypass"))
        || !req
            .prompt
            .as_ref()
            .is_some_and(|p| !p.trim().is_empty() && p.len() <= 32768)
    {
        return Err(SessionStartError::Invalid(
            "exact conversation differs from reviewed definition/project/first turn".into(),
        ));
    }
    serde_json::to_string(binding).map_err(|e| SessionStartError::Invalid(e.to_string()))
}

impl SubprocessSessionStarter {
    async fn start_inner(
        &self,
        req: SessionStartRequest,
        exact: Option<String>,
    ) -> Result<String, SessionStartError> {
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

        let argv =
            session_start_argv(&req, clone_dir.as_deref()).map_err(SessionStartError::Invalid)?;

        let exact_start = exact.is_some();
        let mut cmd = tokio::process::Command::new(&self.exe);
        cmd.args(argv.to_args());
        if let Some(proof) = exact {
            cmd.arg("--exact-agent-definition").arg(proof);
        }
        if let Some(dir) = req.working_dir.as_deref() {
            cmd.current_dir(dir);
        }
        if req.mode.as_deref() == Some("ask") {
            #[cfg(not(unix))]
            return Err(SessionStartError::Invalid(
                "native Ask approvals require a local authenticated broker".into(),
            ));
            #[cfg(unix)]
            {
                if req.agent != rupu_agent::builtin::SESSION_AGENT_NAME {
                    return Err(SessionStartError::Invalid(
                        "native Ask requires the builtin session harness".into(),
                    ));
                }
                let broker = self.approval_broker.as_ref().ok_or_else(|| {
                    SessionStartError::Invalid("Ask approval broker unavailable".into())
                })?;
                let workspace = req.workspace_id.clone().ok_or_else(|| {
                    SessionStartError::Invalid("Ask requires exact workspace identity".into())
                })?;
                let path = req.working_dir.as_ref().map(PathBuf::from).ok_or_else(|| {
                    SessionStartError::Invalid("Ask requires exact workspace path".into())
                })?;
                let channel = broker
                    .spawn_channel(None, workspace, path)
                    .map_err(|_| SessionStartError::Spawn("approval channel unavailable".into()))?;
                cmd.arg("--approval-channel")
                    .stdin(crate::session_approval_channel::stdin_for_child(channel));
            }
        }

        let out = cmd
            .output()
            .await
            .map_err(|e| SessionStartError::Spawn(e.to_string()))?;

        if !out.status.success() {
            let msg = crate::cp_session_sender::cli_failure(&out.stderr, "session start failed");
            // Exit 1 is the command refusing the request (a skill that
            // doesn't resolve, an unknown agent): the caller's to fix.
            return Err(if exact_start {
                // The child may have persisted a session before failing. No safe
                // rejection/automatic retry claim follows a started process.
                SessionStartError::Spawn(msg)
            } else if out.status.code() == Some(75) {
                SessionStartError::Conflict(msg)
            } else if out.status.code() == Some(1) {
                SessionStartError::Invalid(msg)
            } else {
                SessionStartError::Spawn(msg)
            });
        }

        parse_session_id(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
            SessionStartError::Spawn("could not determine session id from output".into())
        })
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
            tool_ceiling: None,
            mcp_tools: None,
            mcp_catalog_revision: None,
            effort: None,
            skill_preset: None,
            tools: None,
            agent: "triage".into(),
            provider: None,
            model: None,
            workspace_id: None,
            idle: false,
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

    #[tokio::test]
    async fn stale_mcp_child_preflight_is_a_conflict_without_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = crate::test_support::exec_ready_script(
            &tmp.path().join("rupu"),
            "echo '[error] MCP catalog changed' >&2\nexit 75\n",
        );
        let starter = SubprocessSessionStarter {
            exe,
            #[cfg(unix)]
            approval_broker: None,
        };
        let request = req(
            None,
            Some("fixture"),
            None,
            Some(tmp.path().to_str().unwrap()),
        );
        assert!(
            matches!(starter.start(request).await, Err(SessionStartError::Conflict(message)) if message.contains("MCP catalog changed"))
        );
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
    fn idle_creation_is_explicit_and_rejects_implicit_work() {
        let mut r = req(None, None, Some("readonly"), None);
        r.idle = true;
        assert!(argv(&r, None).iter().any(|flag| flag == "--idle"));
        r.prompt = Some("go".into());
        assert!(session_start_argv(&r, None).is_err());
        r.prompt = None;
        r.target = Some("github:o/r".into());
        assert!(session_start_argv(&r, None).is_err());
    }

    #[test]
    fn argv_carries_first_turn_tools_without_collapsing_an_empty_selection() {
        let mut r = req(None, Some("inspect"), Some("readonly"), None);
        assert!(!argv(&r, None)
            .iter()
            .any(|v| v == "--tool" || v == "--no-tools"));
        r.tools = Some(vec![]);
        assert!(argv(&r, None).iter().any(|v| v == "--no-tools"));
        r.tools = Some(vec!["read_file".into(), "scm.*".into()]);
        let a = argv(&r, None);
        assert!(a
            .windows(4)
            .any(|v| v == ["--tool", "read_file", "--tool", "scm.*"]));
        assert!(!a.iter().any(|v| v == "--no-tools"));
        r.tools = Some(vec!["--mode".into()]);
        assert!(session_start_argv(&r, None).is_err());
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
            #[cfg(unix)]
            approval_broker: None,
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
    fn argv_preserves_first_turn_model_and_project_identity() {
        let mut request = req(
            None,
            Some("inspect"),
            Some("readonly"),
            Some("/selected/project"),
        );
        request.agent = "rupu-session".into();
        request.provider = Some("named-account".into());
        request.model = Some("deliberate-model".into());
        request.workspace_id = Some("ws_selected".into());
        assert_eq!(
            argv(&request, None),
            [
                "session",
                "start",
                "rupu-session",
                "--provider",
                "named-account",
                "--model",
                "deliberate-model",
                "--workspace-id",
                "ws_selected",
                "--detach",
                "--mode",
                "readonly",
                "--prompt=inspect",
            ]
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
    #[test]
    fn preset_selection_is_preserved_and_never_applied_to_another_agent() {
        let mut request = req(None, Some("inspect"), Some("readonly"), None);
        request.skill_preset = Some("none".into());
        assert!(session_start_argv(&request, None).is_err());
        request.agent = rupu_agent::builtin::SESSION_AGENT_NAME.into();
        assert!(argv(&request, None)
            .windows(2)
            .any(|pair| pair == ["--skill-preset", "none"]));
        request.skill_preset = Some("rupu-session-full-v1".into());
        assert!(session_start_argv(&request, None).is_ok());
        request.skill_preset = Some("future-preset".into());
        assert!(session_start_argv(&request, None).is_err());
    }
}

#[cfg(all(test, unix))]
mod exact_conversation_tests {
    use super::*;
    fn fixture() -> (SessionStartRequest, rupu_runtime::exact_agent::Binding) {
        let binding = rupu_runtime::exact_agent::Binding {
            file_key: "chosen.md".into(),
            definition_revision: "a".repeat(64),
            effective_name: "analyst".into(),
            workspace_id: "ws_selected".into(),
            project_path: "/chosen/project".into(),
            project_revision: "b".repeat(64),
        };
        let req = SessionStartRequest {
            effort: None,
            agent: "analyst".into(),
            provider: None,
            model: None,
            tools: None,
            tool_ceiling: None,
            mcp_tools: Some(vec![]),
            mcp_catalog_revision: None,
            workspace_id: Some(binding.workspace_id.clone()),
            idle: false,
            prompt: Some("Inspect".into()),
            mode: Some("readonly".into()),
            target: None,
            working_dir: Some(binding.project_path.clone()),
            skills: vec![],
            preload_skills: vec![],
            skill_preset: None,
        };
        (req, binding)
    }
    #[test]
    fn exact_conversation_adapter_proof_retains_filename_and_project() {
        let (req, binding) = fixture();
        let proof = exact_session_proof(&req, &binding).unwrap();
        assert_eq!(
            rupu_runtime::exact_agent::Binding::decode(&proof).unwrap(),
            binding
        );
        let args = session_start_argv(&req, None).unwrap().to_args();
        assert!(args.iter().any(|v| v == "--workspace-id"));
        assert!(args.iter().any(|v| v == "--detach"));
    }
    #[test]
    fn exact_conversation_adapter_rejects_request_rebinding() {
        let (req, binding) = fixture();
        let mut v = req.clone();
        v.agent = "shadow".into();
        assert!(exact_session_proof(&v, &binding).is_err());
        let mut v = req.clone();
        v.workspace_id = None;
        assert!(exact_session_proof(&v, &binding).is_err());
        let mut v = req.clone();
        v.working_dir = Some("/other".into());
        assert!(exact_session_proof(&v, &binding).is_err());
        let mut v = req.clone();
        v.target = Some("github:o/r".into());
        assert!(exact_session_proof(&v, &binding).is_err());
        let mut v = req;
        v.idle = true;
        assert!(exact_session_proof(&v, &binding).is_err());
    }
}
