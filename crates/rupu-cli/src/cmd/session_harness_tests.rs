//! First-turn creation is checked without starting provider work or a worker.
use super::*;
use clap::Parser;

fn args(extra: &[&str]) -> StartArgs {
    let mut argv = vec![
        "rupu",
        "session",
        "start",
        "rupu-session",
        "--prompt",
        "inspect this project",
    ];
    argv.extend_from_slice(extra);
    match crate::Cli::try_parse_from(argv).unwrap().command {
        crate::Cmd::Session {
            action: Action::Start(args),
        } => args,
        _ => panic!("expected session start"),
    }
}

#[tokio::test]
async fn builtin_first_turn_is_selected_before_any_worker_or_settings_patch() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(tmp.path().join(".rupu")).unwrap();
    fs::write(
        tmp.path().join(".rupu/config.toml"),
        "default_provider = 'wrong-ancestor'\ndefault_model = 'wrong-model'\n",
    )
    .unwrap();
    fs::write(
        global.join("config.toml"),
        "default_provider = 'anthropic'\ndefault_model = 'old-model'\n",
    )
    .unwrap();
    let ws = rupu_workspace::register(
        &rupu_workspace::WorkspaceStore {
            root: global.join("workspaces"),
        },
        &project,
    )
    .unwrap();
    let (record, turn) = prepare_start(
        &args(&[
            "--provider",
            "openai",
            "--model",
            "deliberate-model",
            "--workspace-id",
            &ws.id,
        ]),
        global.clone(),
        project.clone(),
    )
    .await
    .unwrap();
    assert_eq!(record.provider_name, "openai");
    assert_eq!(record.model, "deliberate-model");
    assert_eq!(record.agent_name, "rupu-session");
    assert!(record.builtin_harness);
    assert_eq!(record.written_version(), 4);
    assert_eq!(record.workspace_id, ws.id);
    assert_eq!(record.workspace_path, project.canonicalize().unwrap());
    assert_eq!(record.project_root, Some(project.canonicalize().unwrap()));
    assert_eq!(record.permission_mode, "readonly");
    assert_eq!(turn.prompt, "inspect this project");
    assert!(record.message_history.is_empty());
    assert!(record.runs.is_empty());
    assert!(record.worker_pid.is_none());
    assert!(!project.join(".rupu/agents").exists());
    assert!(!global.join("sessions").join(&record.session_id).exists());
    assert!(record.auth_mode.is_none());
}

#[tokio::test]
async fn builtin_creation_refuses_ask_unknown_modes_and_wrong_project_before_a_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    for extra in [
        vec!["--mode", "ask"],
        vec!["--mode", "yolo"],
        vec!["--workspace-id", "ws_missing"],
        vec!["--provider", "missing-account", "--model", "model"],
    ] {
        let error = prepare_start(&args(&extra), global.clone(), project.clone())
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
    assert!(
        rupu_runtime::sessions::load_sessions_in_scope(&global, SessionScope::Active)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn legacy_name_collision_keeps_frozen_agent_and_disk_fallbacks() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("agents")).unwrap();
    fs::write(
        tmp.path().join("agents/rupu-session.md"),
        "---\nname: rupu-session\nfallbacks:\n  - model: old-fallback\n---\nOld project agent.",
    )
    .unwrap();
    let mut record = super::tests::test_session_record();
    record.agent_name = "rupu-session".into();
    record.agent_system_prompt = "frozen legacy prompt".into();
    record.project_root = None;
    let legacy = session_agent_spec(tmp.path(), &record).unwrap();
    assert_eq!(legacy.system_prompt, "frozen legacy prompt");
    assert_eq!(legacy.fallbacks.as_ref().unwrap().len(), 1);
    let mut wire = serde_json::to_value(&record).unwrap();
    wire.as_object_mut().unwrap().remove("builtin_harness");
    assert!(
        !serde_json::from_value::<SessionRecord>(wire)
            .unwrap()
            .builtin_harness
    );
    record.builtin_harness = true;
    let builtin = session_agent_spec(tmp.path(), &record).unwrap();
    assert_eq!(builtin.fallbacks, Some(Vec::new()));
}

#[tokio::test]
async fn duplicate_global_and_project_skills_do_not_block_builtin_creation() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    for root in [
        global.join("skills/review"),
        project.join(".rupu/skills/review"),
    ] {
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: review\ndescription: Review the selected changes.\n---\nReview source.",
        )
        .unwrap();
    }
    let (session, _) = prepare_start(&args(&[]), global.clone(), project.clone())
        .await
        .unwrap();
    assert!(session.builtin_harness);
    assert!(session.skill_index.is_empty());
    let (qualified, _) = prepare_start(&args(&["--skill", "project:review"]), global, project)
        .await
        .unwrap();
    assert_eq!(qualified.skill_index.len(), 1);
    assert_eq!(qualified.skill_index[0].layer, rupu_skills::Layer::Project);
}

fn idle_args(extra: &[&str]) -> Result<StartArgs, clap::Error> {
    let mut argv = vec!["rupu", "session", "start", "rupu-session", "--idle"];
    argv.extend_from_slice(extra);
    match crate::Cli::try_parse_from(argv)?.command {
        crate::Cmd::Session {
            action: Action::Start(args),
        } => Ok(args),
        _ => panic!("expected session start"),
    }
}

#[test]
fn idle_parser_refuses_prompts_targets_and_clone_destinations() {
    for extra in [
        vec!["--prompt", "go"],
        vec!["github:o/r"],
        vec!["--into", "/tmp/clone"],
    ] {
        assert!(idle_args(&extra).is_err());
    }
}

#[tokio::test]
async fn idle_creation_persists_zero_turns_without_spawning_a_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        global.join("config.toml"),
        "default_provider = 'openai'\ndefault_model = 'configured-model'\n",
    )
    .unwrap();
    start_in(
        idle_args(&["--provider", "openai", "--model", "selected-model"]).unwrap(),
        global.clone(),
        project,
    )
    .await
    .unwrap();
    let sessions =
        rupu_runtime::sessions::load_sessions_in_scope(&global, SessionScope::Active).unwrap();
    assert_eq!(sessions.len(), 1);
    let record = sessions[0].record();
    assert!(record.builtin_harness);
    assert_eq!(record.total_turns, 0);
    assert!(record.runs.is_empty());
    assert!(record.message_history.is_empty());
    assert!(record.worker_pid.is_none());
    assert_eq!(record.provider_name, "openai");
    assert_eq!(record.model, "selected-model");
    let dir = session_dir(&global, SessionScope::Active, &record.session_id);
    assert!(dir.join("settings.json").exists());
    assert!(!global.join("runs").exists());
}
