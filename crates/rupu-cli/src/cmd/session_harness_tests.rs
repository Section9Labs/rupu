//! First-turn creation is checked without starting provider work or a worker.
use super::*;
use clap::Parser;
use serde_json::json;

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
        } => *args,
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
    assert_eq!(record.written_version(), 5);
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
        } => Ok(*args),
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

#[tokio::test]
async fn first_turn_tool_selection_is_frozen_before_the_worker_and_keeps_readonly_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let defaults = rupu_agent::builtin::session_agent().tools;
    for (flags, expected) in [
        (vec![], defaults),
        (vec!["--no-tools"], Some(vec![])),
        (
            vec!["--tool", "read_file", "--tool", "write_file"],
            Some(vec!["read_file".into(), "write_file".into()]),
        ),
        (vec!["--tool", "*"], Some(vec!["*".into()])),
    ] {
        let (record, turn) = prepare_start(&args(&flags), global.clone(), project.clone())
            .await
            .unwrap();
        assert_eq!(record.agent_tools, expected);
        assert_eq!(
            session_agent_spec(&global, &record).unwrap().tools,
            expected
        );
        assert_eq!(record.permission_mode, "readonly");
        assert_eq!(turn.prompt, "inspect this project");
        assert!(record.runs.is_empty() && record.worker_pid.is_none());
        assert!(!global.join("sessions").join(&record.session_id).exists());

        // Feed the frozen selection to the same resolver assembly uses. An
        // empty declaration leaves the session's concerns ambient, and '*'
        // never silently grants the explicit coordination authority tools.
        let services = rupu_tools::ToolCatalog::all()
            .iter()
            .flat_map(|d| d.needs.iter().copied())
            .collect();
        let grant = rupu_tools::ToolCatalog::builtin()
            .resolve_grant(rupu_tools::GrantInputs {
                declared: record.agent_tools.as_deref(),
                step_actions: &[],
                connector_cap: None,
                ambient: &[rupu_tools::AmbientGrant::concerns()],
                available: &services,
                alias_scope: rupu_tools::AliasScope::Everywhere,
            })
            .unwrap();
        assert!(grant.entries.contains_key("coverage.mark"));
        assert!(!grant.entries.contains_key("board.directive"));
        assert!(!grant.entries.contains_key("board.retract"));
        if expected == Some(vec![]) {
            assert!(!grant.entries.contains_key("read_file"));
        }
        let policy = rupu_tools::PermissionPolicy::unattended(rupu_tools::PermissionMode::Readonly);
        if let Some(entry) = grant.entries.get("write_file") {
            assert!(matches!(
                policy.decide_call(
                    &entry.descriptor.name,
                    entry.descriptor.effect,
                    &serde_json::json!({}),
                    &mut rupu_tools::AllowAlways::default()
                ),
                rupu_tools::Decision::Deny { .. }
            ));
        }
        if let Some(entry) = grant.entries.get("dispatch") {
            assert!(matches!(
                policy.decide_call(
                    &entry.descriptor.name,
                    entry.descriptor.effect,
                    &serde_json::json!({}),
                    &mut rupu_tools::AllowAlways::default()
                ),
                rupu_tools::Decision::Spawn {
                    ceiling: rupu_tools::PermissionMode::Readonly
                }
            ));
        }
    }
}

#[tokio::test]
async fn invalid_first_turn_tools_never_create_a_session_or_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    for name in ["unknown_tool", "runs.list", "--mode"] {
        let error = prepare_start(
            &args(&[&format!("--tool={name}")]),
            global.clone(),
            project.clone(),
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
    assert!(
        rupu_runtime::sessions::load_sessions_in_scope(&global, SessionScope::Active)
            .unwrap()
            .is_empty()
    );
    assert!(crate::Cli::try_parse_from([
        "rupu",
        "session",
        "start",
        "rupu-session",
        "--no-tools",
        "--tool",
        "read_file"
    ])
    .is_err());
}

#[tokio::test]
async fn mcp_empty_skips_bad_host_registry_and_legacy_records_keep_empty_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(global.join("mcp.toml"), "invalid host registry").unwrap();
    let (record, _) = prepare_start(
        &args(&["--mcp-tools", "[]"]),
        global.clone(),
        project.clone(),
    )
    .await
    .unwrap();
    assert_eq!(record.mcp_tools.as_ref().unwrap().tools.len(), 0);
    assert_eq!(record.written_version(), 5);
    assert!(!global.join("sessions").join(&record.session_id).exists());
    let legacy = super::tests::test_session_record();
    assert!(legacy.mcp_tools.is_none());
    assert!(legacy.written_version() < 5);
    assert!(prepare_start(&args(&[]), global, project).await.is_err());
}

#[tokio::test]
async fn configured_mcp_defaults_freeze_before_creation_and_stale_choice_never_writes_session() {
    use httpmock::{Method::POST, MockServer};
    let server = MockServer::start_async().await;
    for (method, id, result) in [
        (
            "initialize",
            1,
            json!({"protocolVersion":rupu_mcp_client::PROTOCOL_VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}),
        ),
        (
            "tools/list",
            2,
            json!({"tools":[{"name":"query","description":"Fixture","inputSchema":{"type":"object"}}]}),
        ),
    ] {
        server.mock(|when, then| {
            when.method(POST)
                .json_body_partial(json!({"method":method}).to_string());
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"jsonrpc":"2.0","id":id,"result":result}));
        });
    }
    server.mock(|when, then| {
        when.method(POST)
            .json_body_partial(r#"{"method":"notifications/initialized"}"#);
        then.status(202);
    });
    let calls = server.mock(|when, then| {
        when.method(POST)
            .json_body_partial(r#"{"method":"tools/call"}"#);
        then.status(500);
    });
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        global.join("mcp.toml"),
        format!(
            "version=1\n[servers.fixture]\nlabel='Fixture'\nurl='{}'\nenabled=true",
            server.url("/mcp")
        ),
    )
    .unwrap();
    let (record, _) = prepare_start(&args(&[]), global.clone(), project.clone())
        .await
        .unwrap();
    let frozen = record.mcp_tools.as_ref().unwrap();
    assert_eq!(frozen.tools.len(), 1);
    assert_eq!(frozen.tools[0].identity.tool, "query");
    let stale = args(&[
        "--mcp-tools",
        r#"[{"server":"fixture","tool":"query"}]"#,
        "--mcp-catalog-revision",
        "old",
    ]);
    let error = prepare_start(&stale, global.clone(), project)
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<rupu_runtime::session_mcp::Error>(),
        Some(rupu_runtime::session_mcp::Error::Stale)
    ));
    assert_eq!(fs::read_dir(global.join("sessions")).unwrap().count(), 0);
    calls.assert_hits(0);
}
