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
    assert_eq!(record.written_version(), 6);
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
            record.builtin_tools_preset.is_some(),
            flags.is_empty(),
            "only inherited default selection carries preset provenance"
        );
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
    assert_eq!(record.written_version(), 6);
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

#[tokio::test]
async fn scoped_read_default_provenance_is_frozen_and_explicit_choices_never_expand() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let (default, _) = prepare_start(&args(&[]), global.clone(), project.clone())
        .await
        .unwrap();
    assert_eq!(
        default.builtin_tools_preset.as_deref(),
        Some(rupu_agent::builtin::SESSION_TOOLS_PRESET)
    );
    assert_eq!(default.written_version(), 6);
    assert_eq!(default.scoped_reads_version, Some(1));
    let names = [
        "session.context",
        "session.workflows",
        "run.inspect",
        "run.netflows",
    ];
    for name in names {
        assert!(default
            .agent_tools
            .as_ref()
            .unwrap()
            .iter()
            .any(|tool| tool == name));
    }
    let wire = serde_json::to_vec(&default).unwrap();
    let read = rupu_runtime::sessions::parse_session_record(&wire).unwrap();
    assert_eq!(read.builtin_tools_preset, default.builtin_tools_preset);
    assert_eq!(read.scoped_reads_version, default.scoped_reads_version);
    assert_eq!(read.agent_tools, default.agent_tools);
    for selection in [
        vec!["--no-tools"],
        vec!["--tool", "read_file"],
        vec!["--tool", "session.context"],
    ] {
        let (custom, _) = prepare_start(&args(&selection), global.clone(), project.clone())
            .await
            .unwrap();
        assert!(custom.builtin_tools_preset.is_none());
        assert_eq!(custom.written_version(), 6);
        assert_eq!(custom.scoped_reads_version, Some(1));
        let frozen = custom.agent_tools.clone();
        assert_eq!(session_agent_spec(&global, &custom).unwrap().tools, frozen);
        assert_eq!(
            frozen.as_ref().unwrap().len(),
            usize::from(selection[0] != "--no-tools")
        );
    }
    let mut legacy = serde_json::to_value(&default).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("builtin_tools_preset");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("scoped_reads_version");
    legacy["version"] = serde_json::json!(5);
    legacy["agent_tools"] = serde_json::json!(["read_file"]);
    let legacy =
        rupu_runtime::sessions::parse_session_record(&serde_json::to_vec(&legacy).unwrap())
            .unwrap();
    assert!(legacy.builtin_tools_preset.is_none());
    assert!(legacy.scoped_reads_version.is_none());
    assert!(legacy.written_version() <= 5);
    assert_eq!(
        session_agent_spec(&global, &legacy).unwrap().tools,
        Some(vec!["read_file".into()])
    );
    assert!(!global.join("sessions").join(&default.session_id).exists());
}

#[tokio::test]
async fn scoped_reader_is_offered_and_called_in_the_actual_worker_only_when_declared() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    for variant in ["new_exact", "new_empty", "legacy_star", "legacy_exact"] {
        let selected = matches!(variant, "new_exact" | "legacy_exact");
        let tmp = tempfile::tempdir().unwrap();
        let (global, mut record) = super::tests::idle_dense_session(&tmp, "ses_scoped_reads");
        record.message_history.clear();
        record.builtin_harness = true;
        record.agent_name = rupu_agent::builtin::SESSION_AGENT_NAME.into();
        record.scoped_reads_version = variant.starts_with("new_").then_some(1);
        record.agent_tools = Some(match variant {
            "new_exact" => vec!["session.context".into()],
            "new_empty" => Vec::new(),
            _ => vec!["*".into()],
        });
        record.permission_mode = "readonly".into();
        record.model_limits = Some(rupu_providers::model_limits::ModelLimits::fixed(
            100_000, 8_192,
        ));
        write_session(&global, SessionScope::Active, &record).unwrap();
        if variant == "legacy_exact" {
            rupu_runtime::session_settings::SessionSettings {
                tools: Some(vec!["*".into(), "session.context".into()]),
                ..Default::default()
            }
            .save(&session_dir(
                &global,
                SessionScope::Active,
                &record.session_id,
            ))
            .unwrap();
        }
        let script = serde_json::json!([
            {"AssistantToolUse":{"text":null,"tool_id":"scoped_call","tool_name":"session.context","tool_input":{},"stop":"tool_use"}},
            {"AssistantText":{"text":"done","stop":"end_turn"}}
        ]).to_string();
        super::tests::with_mock_home(
            &global,
            &script,
            run_turn(RunTurnArgs {
                session_id: record.session_id.clone(),
                run_id: "run_scoped_reads".into(),
                prompt: "observe own context".into(),
                operator_skills: Vec::new(),
            }),
        )
        .await
        .unwrap();
        let events: Vec<_> =
            JsonlReader::iter(record.transcripts_dir.join("run_scoped_reads.jsonl"))
                .unwrap()
                .map(Result::unwrap)
                .collect();
        let offered = events.iter().any(|event| matches!(event, TranscriptEvent::ToolGrant {entries,..} if entries.iter().any(|entry| entry.tool == "session.context" && entry.remote.is_none())));
        assert_eq!(offered, selected, "{variant}");
        assert!(!events.iter().any(|event| matches!(event, TranscriptEvent::ToolGrant {entries,..} if entries.iter().any(|entry| ["session.workflows", "run.inspect", "run.netflows"].contains(&entry.tool.as_str())))), "{variant}");
        let expected = if selected { "allowed" } else { "not_granted" };
        assert!(events.iter().any(|event| matches!(event, TranscriptEvent::ToolAudit {tool,decision:Some(decision),..} if tool == "session.context" && decision == expected)));
        if selected {
            assert!(events.iter().any(|event| matches!(event, TranscriptEvent::ToolResult {structured:Some(value),error:None,..} if value["session_id"] == record.session_id && value["run_id"] == "run_scoped_reads" && value["workspace_id"] == record.workspace_id && value["host_id"] == "local")));
        }
        assert_eq!(
            read_session(&global, &record.session_id)
                .unwrap()
                .0
                .permission_mode,
            "readonly"
        );
    }
}

#[test]
fn scoped_reader_service_does_not_expand_legacy_or_custom_declarations() {
    use rupu_runtime::session_settings::SessionSettings;
    let settings = SessionSettings::default();
    for entries in [
        vec![],
        vec!["read_file".into()],
        vec!["core.*".into()],
        vec!["mcp.towned".into()],
    ] {
        assert!(selected_session_reads(Some(1), Some(&entries), &settings).is_empty());
    }
    for entry in [
        "session.context",
        "session.workflows",
        "run.inspect",
        "run.netflows",
        "session.*",
        "run.*",
        "*",
    ] {
        assert!(!selected_session_reads(Some(1), Some(&[entry.into()]), &settings).is_empty());
        assert!(selected_session_reads(None, Some(&[entry.into()]), &settings).is_empty());
    }
    let mut reviewed = SessionSettings {
        revision: 1,
        tools: Some(vec!["*".into(), "session.context".into()]),
        ..Default::default()
    };
    assert_eq!(
        selected_session_reads(None, reviewed.tools.as_deref(), &reviewed),
        vec!["session.context"]
    );
    reviewed.tools = Some(vec!["*".into(), "session.*".into(), "run.*".into()]);
    assert!(selected_session_reads(None, reviewed.tools.as_deref(), &reviewed).is_empty());
    reviewed.tools = Some(vec!["session.context".into()]);
    reviewed.revision = 0;
    assert!(selected_session_reads(None, reviewed.tools.as_deref(), &reviewed).is_empty());
    assert!(selected_session_reads(Some(99), reviewed.tools.as_deref(), &reviewed).is_empty());
}

#[tokio::test]
async fn private_approval_handoff_allows_exact_ask_preparation_without_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let ws = rupu_workspace::register(
        &rupu_workspace::WorkspaceStore {
            root: global.join("workspaces"),
        },
        &project,
    )
    .unwrap();
    let request = args(&[
        "--mode",
        "ask",
        "--provider",
        "openai",
        "--model",
        "fixture-model",
        "--workspace-id",
        &ws.id,
        "--mcp-tools",
        "[]",
    ]);
    let denied =
        prepare_start_with_approval(&request, global.clone(), project.clone(), false).await;
    assert!(denied.is_err());
    let (record, _) = prepare_start_with_approval(&request, global.clone(), project.clone(), true)
        .await
        .unwrap();
    assert_eq!(record.permission_mode, "ask");
    assert!(record.builtin_harness);
    // The verified Ask handoff does not replace scoped-reader provenance or
    // widen an explicit empty tool selection in this combined runtime.
    assert_eq!(record.scoped_reads_version, Some(1));
    assert_eq!(record.written_version(), 6);
    assert_eq!(
        record.builtin_tools_preset.as_deref(),
        Some(rupu_agent::builtin::SESSION_TOOLS_PRESET)
    );
    let empty_request = args(&[
        "--mode", "ask", "--provider", "openai", "--model", "fixture-model",
        "--workspace-id", &ws.id, "--mcp-tools", "[]", "--no-tools",
    ]);
    let (empty, _) = prepare_start_with_approval(
        &empty_request, global.clone(), project, true,
    ).await.unwrap();
    assert_eq!(empty.agent_tools, Some(Vec::new()));
    assert_eq!(empty.scoped_reads_version, Some(1));
    assert_eq!(empty.written_version(), 6);
    assert!(empty.builtin_tools_preset.is_none());
    assert_eq!(record.workspace_id, ws.id);
    assert!(record.worker_pid.is_none());
    assert!(record.runs.is_empty());
    assert!(!global.join("sessions").join(&record.session_id).exists());
}


#[test]
fn tool_ceiling_cli_is_exact_optional_and_rejects_invalid_before_preparation() {
    assert_eq!(requested_tool_ceiling(&args(&[])).unwrap(), None);
    assert_eq!(
        requested_tool_ceiling(&args(&["--tool-ceiling", "[]"])).unwrap(),
        Some(vec![])
    );
    assert_eq!(
        requested_tool_ceiling(&args(&[
            "--tool-ceiling",
            "[\"skills.load\"]",
            "--mcp-tools",
            "[]"
        ]))
        .unwrap(),
        Some(vec!["skills.load".into()])
    );
    for raw in [
        "[\"*\"]",
        "[\"skill_load\"]",
        "[\"unknown\"]",
        "null",
        "[\"read_file\",\"read_file\"]",
    ] {
        assert!(
            requested_tool_ceiling(&args(&["--tool-ceiling", raw])).is_err(),
            "{raw}"
        );
    }
}

#[test]
fn persisted_tool_ceiling_survives_reload_settings_and_bookkeeping() {
    let global = tempfile::tempdir().unwrap();
    let mut record = super::tests::test_session_record();
    record.tool_ceiling = Some(vec!["skills.load".into()]);
    write_session(global.path(), SessionScope::Active, &record).unwrap();
    let path = session_record_path(global.path(), SessionScope::Active, &record.session_id);
    let mut reloaded =
        rupu_runtime::sessions::parse_session_record(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(reloaded.version, 7);
    let settings = rupu_runtime::session_settings::SessionSettings {
        tools: Some(vec!["*".into()]),
        ..Default::default()
    };
    let mut agent = rupu_agent::builtin::session_agent();
    settings.apply(&mut agent);
    reloaded.agent_tools = agent.tools;
    reloaded.worker_pid = None;
    reloaded.status = SessionStatus::Idle;
    write_session(global.path(), SessionScope::Active, &reloaded).unwrap();
    let reloaded = rupu_runtime::sessions::parse_session_record(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(reloaded.tool_ceiling, Some(vec!["skills.load".into()]));
    assert!(
        serde_json::from_value::<rupu_runtime::session_settings::SessionSettings>(
            json!({"tool_ceiling":null})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<rupu_runtime::session_settings::SessionSettings>(
            json!({"tool_ceiling":[]})
        )
        .is_err()
    );
    let mut wire = serde_json::to_value(reloaded).unwrap();
    wire["version"] = json!(6);
    assert!(
        rupu_runtime::sessions::parse_session_record(&serde_json::to_vec(&wire).unwrap()).is_err()
    );
    wire["version"] = json!(7);
    wire["tool_ceiling"] = json!(["skill_load"]);
    assert!(
        rupu_runtime::sessions::parse_session_record(&serde_json::to_vec(&wire).unwrap()).is_err()
    );
}

#[tokio::test]
async fn tool_ceiling_freezes_at_creation_without_starting_provider_or_worker() {
    let temporary = tempfile::tempdir().unwrap();
    let global = temporary.path().join("global");
    let project = temporary.path().join("project");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&global).unwrap();
    fs::write(
        global.join("config.toml"),
        "default_provider='anthropic'\ndefault_model='mock'\n",
    )
    .unwrap();
    let (record, _) = prepare_start(
        &args(&[
            "--tool-ceiling",
            "[\"skills.load\"]",
            "--mcp-tools",
            "[]",
            "--skill-preset",
            "none",
        ]),
        global.clone(),
        project,
    )
    .await
    .unwrap();
    assert_eq!(record.tool_ceiling, Some(vec!["skills.load".into()]));
    assert_eq!(record.written_version(), 7);
    assert!(record.worker_pid.is_none() && record.runs.is_empty());
    assert!(!global.join("sessions").join(&record.session_id).exists());
}
