use super::*;
use rupu_runtime::exact_agent::{self, Binding, Loaded};
pub(super) fn load_start(
    args: &StartArgs,
    global: &Path,
    pwd: &Path,
) -> anyhow::Result<Option<Loaded>> {
    args.exact_agent_definition.as_deref().map(|raw| {
        let binding = Binding::decode(raw)?;
        anyhow::ensure!(args.agent == binding.effective_name && args.workspace_id.as_deref() == Some(&binding.workspace_id), "exact conversation identity differs from reviewed definition/project");
        anyhow::ensure!(args.target.is_none() && args.into.is_none() && !args.idle && !args.approval_channel && matches!(args.mode.as_deref(), Some("readonly" | "bypass")), "exact conversation requires an explicit noninteractive first turn in the reviewed project");
        anyhow::ensure!(args.prompt_flag.as_ref().or(args.prompt.as_ref()).is_some_and(|p| !p.trim().is_empty() && p.len() <= 32768), "bounded first prompt required");
        exact_agent::load(global, &binding, pwd)
    }).transpose()
}
pub(super) fn validate_record(
    session: &SessionRecord,
    exact: &rupu_runtime::sessions::ExactSessionAgent,
) -> anyhow::Result<()> {
    let binding = Binding::decode(&exact.binding)?;
    anyhow::ensure!(
        session.version >= 8
            && !session.builtin_harness
            && session.agent_name == binding.effective_name
            && session.workspace_id == binding.workspace_id
            && session.workspace_path == Path::new(&binding.project_path)
            && session.project_root.as_deref() == Some(Path::new(&binding.project_path)),
        "exact conversation persisted owner mismatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, StartArgs) {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let global = base.join("home");
        let project = base.join("project");
        fs::create_dir_all(global.join("agents")).unwrap();
        fs::create_dir_all(global.join("workspaces")).unwrap();
        fs::create_dir_all(project.join(".rupu/agents")).unwrap();
        fs::write(global.join("agents/chosen.md"), "---\nname: analyst\nprovider: openai\nmodel: frozen-model\ntools: []\nfallbacks:\n  - model: frozen-fallback\n---\nExact global instructions\n").unwrap();
        fs::write(project.join(".rupu/agents/analyst.md"), "---\nname: analyst\nprovider: anthropic\nmodel: shadow-model\nfallbacks:\n  - model: shadow-fallback\n---\nShadow instructions\n").unwrap();
        fs::write(
            global.join("workspaces/ws_chosen.toml"),
            format!(
                "id='ws_chosen'\npath='{}'\ncreated_at='2026-11-09T00:00:00Z'\n",
                project.display()
            ),
        )
        .unwrap();
        let binding = exact_agent::review(&global, "chosen.md", "ws_chosen")
            .unwrap()
            .review
            .binding;
        let proof = serde_json::to_string(&binding).unwrap();
        let args = match crate::Cli::try_parse_from([
            "rupu",
            "session",
            "start",
            "analyst",
            "--workspace-id",
            "ws_chosen",
            "--exact-agent-definition",
            &proof,
            "--mode",
            "readonly",
            "--prompt",
            "Inspect",
            "--mcp-tools",
            "[]",
        ])
        .unwrap()
        .command
        {
            crate::Cmd::Session {
                action: Action::Start(args),
            } => *args,
            _ => panic!("session start"),
        };
        (tmp, global, project, args)
    }
    #[tokio::test]
    async fn exact_conversation_freezes_definition_and_fallbacks_across_shadow_edits() {
        let (_tmp, global, project, args) = fixture();
        let registry = fs::read(global.join("workspaces/ws_chosen.toml")).unwrap();
        let (record, turn) = prepare_start(&args, global.clone(), project.clone())
            .await
            .unwrap();
        assert_eq!(record.agent_system_prompt, "Exact global instructions\n");
        assert_eq!(record.model, "frozen-model");
        assert_eq!(record.workspace_id, "ws_chosen");
        assert_eq!(turn.prompt, "Inspect");
        assert_eq!(record.written_version(), 8);
        assert_eq!(record.workspace_path, project);
        assert!(!global.join("sessions").join(&record.session_id).exists());
        assert_eq!(
            registry,
            fs::read(global.join("workspaces/ws_chosen.toml")).unwrap()
        );
        fs::remove_file(global.join("agents/chosen.md")).unwrap();
        fs::write(project.join(".rupu/agents/analyst.md"), "malformed shadow").unwrap();
        let spec = session_agent_spec(&global, &record).unwrap();
        assert_eq!(spec.fallbacks.unwrap()[0].model, "frozen-fallback");
        assert_eq!(spec.system_prompt, record.agent_system_prompt);
        let bytes = serde_json::to_vec(&record).unwrap();
        let restored = rupu_runtime::sessions::parse_session_record(&bytes).unwrap();
        assert_eq!(
            session_agent_spec(&global, &restored)
                .unwrap()
                .model
                .as_deref(),
            Some("frozen-model")
        );
    }
    #[test]
    fn exact_conversation_rejects_stale_selection_or_wrong_execution_before_assembly() {
        let (_tmp, global, project, mut args) = fixture();
        assert!(load_start(&args, &global, &project).unwrap().is_some());
        args.workspace_id = Some("ws_other".into());
        assert!(load_start(&args, &global, &project).is_err());
        args.workspace_id = Some("ws_chosen".into());
        assert!(load_start(&args, &global, &global).is_err());
        fs::write(
            global.join("agents/chosen.md"),
            "---\nname: analyst\n---\nChanged\n",
        )
        .unwrap();
        assert!(load_start(&args, &global, &project).is_err());
        assert!(!global.join("sessions").exists());
    }
    #[test]
    fn exact_conversation_rejects_target_idle_ask_and_empty_first_turn() {
        let (_tmp, global, project, args) = fixture();
        let mut invalid = args.clone();
        invalid.target = Some("github:o/r".into());
        assert!(load_start(&invalid, &global, &project).is_err());
        let mut invalid = args.clone();
        invalid.idle = true;
        assert!(load_start(&invalid, &global, &project).is_err());
        let mut invalid = args.clone();
        invalid.mode = Some("ask".into());
        assert!(load_start(&invalid, &global, &project).is_err());
        let mut invalid = args.clone();
        invalid.prompt_flag = Some(" ".into());
        assert!(load_start(&invalid, &global, &project).is_err());
    }
    #[tokio::test]
    async fn exact_record_identity_cannot_be_rebound_and_legacy_record_remains_legacy() {
        let (_tmp, global, project, args) = fixture();
        let (mut record, _) = prepare_start(&args, global.clone(), project).await.unwrap();
        record.workspace_id = "ws_other".into();
        assert!(session_agent_spec(&global, &record).is_err());
        record.workspace_id = "ws_chosen".into();
        record.agent_name = "shadow".into();
        assert!(session_agent_spec(&global, &record).is_err());
        let legacy = super::super::tests::test_session_record();
        assert!(legacy.exact_global_agent.is_none());
        assert!(legacy.written_version() < 7);
        let mut encoded = serde_json::to_value(legacy).unwrap();
        encoded["version"] = serde_json::json!(9);
        assert!(rupu_runtime::sessions::parse_session_record(
            &serde_json::to_vec(&encoded).unwrap()
        )
        .is_err());
    }
    #[tokio::test]
    async fn exact_registered_project_without_layer_never_borrows_parent_definition() {
        let (_tmp, global, project, mut args) = fixture();
        fs::remove_dir_all(project.join(".rupu")).unwrap();
        let parent = project.parent().unwrap();
        fs::create_dir_all(parent.join(".rupu/agents")).unwrap();
        fs::write(
            parent.join(".rupu/agents/analyst.md"),
            "---\nname: analyst\n---\nParent\n",
        )
        .unwrap();
        let binding = exact_agent::review(&global, "chosen.md", "ws_chosen")
            .unwrap()
            .review
            .binding;
        args.exact_agent_definition = Some(serde_json::to_string(&binding).unwrap());
        let (record, _) = prepare_start(&args, global.clone(), project.clone())
            .await
            .unwrap();
        assert_eq!(record.project_root, Some(project));
        assert_eq!(record.agent_system_prompt, "Exact global instructions\n");
        assert_eq!(record.transcripts_dir, global.join("transcripts"));
    }
    #[tokio::test]
    async fn exact_v8_preserves_optional_ceiling_across_save_reload_settings_and_archive_write() {
        for ceiling in [None, Some(vec![]), Some(vec!["skills.load".to_string()])] {
            let (_tmp, global, project, mut args) = fixture();
            args.tool_ceiling = ceiling.as_ref().map(|v| serde_json::to_string(v).unwrap());
            let (record, _) = prepare_start(&args, global.clone(), project).await.unwrap();
            let exact = serde_json::to_value(&record.exact_global_agent).unwrap();
            assert_eq!(record.tool_ceiling, ceiling);
            assert_eq!(record.written_version(), 8);
            write_session(&global, SessionScope::Active, &record).unwrap();
            let (mut loaded, _) = read_session(&global, &record.session_id).unwrap();
            let settings = rupu_runtime::session_settings::SessionSettings {
                tools: Some(vec!["*".into()]),
                ..Default::default()
            };
            let mut spec = session_agent_spec(&global, &loaded).unwrap();
            settings.apply(&mut spec);
            loaded.agent_tools = spec.tools;
            loaded.status = SessionStatus::Idle;
            loaded.worker_pid = None;
            // The writer changes only the version, retaining the whole record clone.
            write_session(&global, SessionScope::Archived, &loaded).unwrap();
            let path = session_record_path(&global, SessionScope::Archived, &record.session_id);
            let restored =
                rupu_runtime::sessions::parse_session_record(&fs::read(path).unwrap()).unwrap();
            assert_eq!(restored.version, 8);
            assert_eq!(restored.tool_ceiling, ceiling);
            assert_eq!(
                serde_json::to_value(&restored.exact_global_agent).unwrap(),
                exact
            );
            assert_eq!(
                session_agent_spec(&global, &restored)
                    .unwrap()
                    .fallbacks
                    .unwrap()[0]
                    .model,
                "frozen-fallback"
            );
            let mut wire = serde_json::to_value(restored).unwrap();
            wire["version"] = serde_json::json!(7);
            assert!(rupu_runtime::sessions::parse_session_record(
                &serde_json::to_vec(&wire).unwrap()
            )
            .is_err());
        }
    }
}
