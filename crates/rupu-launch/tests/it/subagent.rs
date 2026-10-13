//! Sub-agent children end to end (moved here with the in-process dispatcher,
//! W7): each test launches a `subagent` through the real run assembler and
//! agent loop, with the provider factory's mock seam.

use std::sync::Arc;

use rupu_orchestrator::executor::Event as WfEvent;
use rupu_runtime::usage_ledger::LedgerRow;
use rupu_tools::{ChildKind, ChildState, PermissionMode};
use rupu_transcript::{Event, RunMode};
use serial_test::serial;

use crate::support::{
    config, events, request, run, run_start, subagent, Home, MockScript, Recorder, DONE,
};

#[tokio::test]
#[serial]
async fn readonly_subagent_is_capped() {
    let home = Home::new();
    home.agent(
        "writer",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\npermissionMode: bypass",
    );
    let l = home.launcher(config(), None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "w1", "tool_name": "write_file", "tool_input": {"path": "escaped.txt", "content": "x"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "done", "stop": "end_turn" } }
        ]"#,
    );
    let ctx = home.parent(None, &["writer"], PermissionMode::Readonly);
    let status = run(
        &l,
        &ctx,
        request(ChildKind::Subagent, "writer", Some("write it")),
    )
    .await;
    assert_eq!(status.state, ChildState::Done, "{status:?}");
    assert!(
        !home.workspace().join("escaped.txt").exists(),
        "a readonly parent's child must not write"
    );
    let evs = events(status.transcript.as_deref().unwrap());
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::RunStart {
                mode: RunMode::Readonly,
                ..
            }
        )),
        "the child runs readonly: {evs:?}"
    );
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::ToolResult { error: Some(err), .. } if err == "permission_denied"
    )));
}

/// `DispatchStarted` before `DispatchCompleted`, both with the child's
/// sub-run id and kind; the child's calls land in the root's ledger,
/// attributed to the dispatching run.
#[tokio::test]
#[serial]
async fn dispatch_emits_started_then_completed_and_charges_the_roots_ledger() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let recorder = Arc::new(Recorder::default());
    let l = home.launcher(config(), Some(recorder.clone()));
    let _m = MockScript::set(DONE);
    let status = subagent(&home, &l, "child", None).await;
    let transcript = status.transcript.clone().unwrap();

    let seen = recorder.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    match &seen[0] {
        (
            run_id,
            WfEvent::DispatchStarted {
                sub_run_id,
                kind,
                agent,
                transcript_path,
                provider,
                model,
                ..
            },
        ) => {
            assert_eq!(run_id, "parent_run_1");
            assert_eq!(sub_run_id, &status.run_id);
            assert_eq!(kind, "subagent");
            assert_eq!(agent.as_deref(), Some("child"));
            assert_eq!(transcript_path, &transcript);
            assert_eq!(
                (provider.as_deref(), model.as_deref()),
                (Some("anthropic"), Some("claude-sonnet-4-6"))
            );
        }
        other => panic!("Started first, got {other:?}"),
    }
    match &seen[1] {
        (
            run_id,
            WfEvent::DispatchCompleted {
                sub_run_id,
                kind,
                success,
                tokens_in,
                tokens_out,
                ..
            },
        ) => {
            assert_eq!(run_id, "parent_run_1");
            assert_eq!(sub_run_id, &status.run_id);
            assert_eq!(kind, "subagent");
            assert!(success);
            assert_eq!((*tokens_in, *tokens_out), (1, 1));
        }
        other => panic!("Completed second, got {other:?}"),
    }

    let ledger_path = home.runs().join("parent_run_1").join("usage.jsonl");
    let rows: Vec<LedgerRow> = std::fs::read_to_string(&ledger_path)
        .expect("the child appended to the root's ledger")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(!rows.is_empty());
    for row in &rows {
        assert_eq!(row.step_id, None);
        assert_eq!(row.agent_run_id, status.run_id);
        assert_eq!(row.parent_agent_run_id.as_deref(), Some("parent_run_1"));
        assert_eq!(row.agent, "child");
        assert_eq!(row.transcript, transcript);
    }
}

/// Spec 2026-09-30 §6.1: a child resolves limits for ITS OWN agent.
#[tokio::test]
#[serial]
async fn dispatched_child_resolves_its_own_agents_limits() {
    let home = Home::new();
    home.agent(
        "pinnedchild",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\ncontextWindowTokens: 7000\nmaxTokens: 900",
    );
    let l = home.launcher(config(), None);
    let _m = MockScript::set(DONE);
    let status = subagent(&home, &l, "pinnedchild", None).await;
    let t = std::fs::read_to_string(status.transcript.unwrap()).unwrap();
    assert!(t.contains("\"kind\":\"model_limits\""), "{t}");
    assert!(t.contains("input 7,000 · output 900"), "{t}");
}

/// A child runs under its parent's customer (the assembler's).
#[tokio::test]
#[serial]
async fn dispatched_child_inherits_the_parents_customer() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let mut ctx = home.context(config());
    ctx.customer = Some("acme".into());
    let l = home.launcher_with(
        Arc::new(rupu_runtime::assembly::RunAssembler::new(ctx)),
        Arc::new(rupu_launch::testing::ScriptedProcesses::done("x")),
        None,
    );
    let _m = MockScript::set(DONE);
    let status = subagent(&home, &l, "child", None).await;
    let head = rupu_transcript::JsonlReader::head(status.transcript.unwrap()).unwrap();
    assert_eq!(head.customer, Some(Some("acme".to_string())));
}

/// R4: a child's findings land under its PARENT's scope (here the parent's
/// own agent name), and stream into the root `rupu run`'s coverage stream
/// after the begin line the root wrote.
#[tokio::test]
#[serial]
async fn subagent_findings_scope_is_parents() {
    use rupu_coverage::StreamLine;

    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 4\nfindingsProfile: summary\nconcerns:\n  - include: stride",
    );
    let stream = rupu_coverage::stream_path(&home.runs(), "parent_run_1");
    rupu_coverage::write_stream_begin(&stream, "parent_run_1").unwrap();
    let l = home.launcher(config(), None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "report_finding", "tool_input": {"scope": "repo", "summary": "child found it", "severity": "low", "evidence": {"rationale": "because"}}, "stop": "tool_use" } },
          { "AssistantText": { "text": "child done", "stop": "end_turn" } }
        ]"#,
    );
    let mut ctx = home.parent(None, &["child"], PermissionMode::Bypass);
    ctx.services.coverage_stream = Some(stream.clone());
    run(&l, &ctx, request(ChildKind::Subagent, "child", Some("go"))).await;

    let lines: Vec<StreamLine> = std::fs::read_to_string(&stream)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(
        matches!(&lines[0], StreamLine::Begin { run_id, .. } if run_id == "parent_run_1"),
        "{lines:?}"
    );
    assert!(lines
        .iter()
        .any(|l| matches!(l, StreamLine::Catalog { scope_name, .. } if scope_name == "parent")));
    let (scope, record) = lines
        .iter()
        .find_map(|l| match l {
            StreamLine::Findings { scope_name, record } => Some((scope_name, record)),
            _ => None,
        })
        .expect("the child's finding streams");
    assert_eq!(
        scope, "parent",
        "the parent's scope, not the child's agent name"
    );
    assert_eq!(record.summary, "child found it");
    let target = rupu_coverage::target_id(&home.workspace(), "parent");
    let findings = rupu_coverage::read_findings(&rupu_coverage::CoveragePaths::new(
        &home.workspace(),
        &target,
    ))
    .unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
}

/// R3: a child gets `[bash]` — its allowlisted env var reaches its `bash`,
/// and the configured timeout applies.
#[tokio::test]
#[serial]
async fn subagent_reads_bash_config() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let mut cfg = config();
    cfg.bash = rupu_config::BashConfig {
        timeout_secs: Some(7),
        env_allowlist: Some(vec!["RUPU_W3_BASH_PROBE".into()]),
    };
    std::env::set_var("RUPU_W3_BASH_PROBE", "from-config");
    let l = home.launcher(cfg, None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "b1", "tool_name": "bash", "tool_input": {"command": "echo probe=$RUPU_W3_BASH_PROBE"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "done", "stop": "end_turn" } }
        ]"#,
    );
    let status = subagent(&home, &l, "child", None).await;
    std::env::remove_var("RUPU_W3_BASH_PROBE");
    let evs = events(status.transcript.as_deref().unwrap());
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::ToolResult { output, .. } if output.contains("probe=from-config")
        )),
        "the allowlisted variable reaches the child's bash: {evs:?}"
    );
}

/// The child's `<parent>><role>#n` is numbered per parent and role, stamped
/// on `DispatchStarted` and the child's `RunStart`; a parent with no
/// codename leaves the child unnamed.
#[tokio::test]
#[serial]
async fn dispatch_mints_child_codename_and_stamps_dispatch_started() {
    let home = Home::new();
    home.agent(
        "security-reviewer",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let recorder = Arc::new(Recorder::default());
    let l = home.launcher(config(), Some(recorder.clone()));

    let mut statuses = Vec::new();
    for codename in [Some("jade-reef/heron"), Some("jade-reef/heron"), None] {
        let _m = MockScript::set(DONE);
        statuses.push(subagent(&home, &l, "security-reviewer", codename).await);
    }
    let role = rupu_codename::role_word("security-reviewer");
    assert_eq!(
        statuses[0].codename,
        Some(format!("jade-reef/heron>{role}#1"))
    );
    assert_eq!(
        statuses[1].codename,
        Some(format!("jade-reef/heron>{role}#2"))
    );
    assert_eq!(statuses[2].codename, None);

    let started: Vec<_> = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, e)| match e {
            WfEvent::DispatchStarted { codename, .. } => Some(codename.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec![
            Some(format!("jade-reef/heron>{role}#1")),
            Some(format!("jade-reef/heron>{role}#2")),
            None
        ]
    );
    let codename = events(statuses[0].transcript.as_deref().unwrap())
        .into_iter()
        .find_map(|e| match e {
            Event::RunStart { codename, .. } => Some(codename),
            _ => None,
        })
        .unwrap();
    assert_eq!(codename, Some(format!("jade-reef/heron>{role}#1")));
}

/// The root's installed namer numbers the children (and persists them with
/// the run's static slots).
#[tokio::test]
#[serial]
async fn an_installed_namer_numbers_the_children() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let namer =
        rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new("amber-lantern"));
    let l = rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: home.assembler(config()),
        store: Arc::new(rupu_orchestrator::RunStore::new(home.runs())),
        events: None,
        namer: Some(namer.clone()),
        backend: Arc::new(rupu_launch::testing::ScriptedProcesses::done("x")),
        runtime: tokio::runtime::Handle::current(),
        max_concurrent: 8,
        generation: None,
        project: None,
    });
    let _m = MockScript::set(DONE);
    let status = subagent(&home, &l, "child", Some("amber-lantern/otter")).await;
    let role = rupu_codename::role_word("child");
    assert_eq!(
        status.codename,
        Some(format!("amber-lantern/otter>{role}#1"))
    );
    let p: rupu_codename::Codename = "amber-lantern/otter".parse().unwrap();
    assert_eq!(namer.with(|n| n.next_instance(&p, role)), 2);
}

/// Spec 2026-10-01 §5.3: a failed child reports success false, an EMPTY
/// output and its error, without the rung-3 hint (its transcript keeps it).
#[tokio::test]
#[serial]
async fn a_failed_child_returns_no_output_and_its_error() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let l = home.launcher(config(), None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": "Let me read the settings first", "tool_id": "call_1", "tool_name": "read_file", "tool_input": { "path": "absent.toml" }, "stop": "tool_use" } },
          { "Reply": {
              "content": [{ "type": "text", "text": "I won't do that." }],
              "stop": { "reason": "refusal", "wire": { "provider": "anthropic", "value": "refusal" } }
          } }
        ]"#,
    );
    let status = subagent(&home, &l, "child", None).await;
    assert_eq!(status.state, ChildState::Done);
    assert_eq!(status.success, Some(false));
    assert_eq!(status.output.as_deref(), Some(""));
    let error = status.error.clone().unwrap();
    assert!(error.contains("refused"), "{error}");
    assert!(
        !error.contains("no recovery left") && !error.contains("--continue"),
        "{error}"
    );
    let complete = events(status.transcript.as_deref().unwrap())
        .into_iter()
        .find_map(|e| match e {
            Event::RunComplete { error, .. } => error,
            _ => None,
        })
        .unwrap();
    assert!(complete.contains("no recovery left"), "{complete}");
}

/// ISSUES.md I-8: a child that pins neither provider nor model resolves
/// both through config's defaults.
#[tokio::test]
#[serial]
async fn dispatch_honors_config_default_provider_and_model() {
    let home = Home::new();
    home.agent("child", "maxTurns: 3");
    let mut cfg = config();
    cfg.default_provider = Some("cfg-provider".into());
    cfg.default_model = Some("cfg-model".into());
    cfg.providers.insert(
        "cfg-provider".into(),
        rupu_config::ProviderConfig {
            kind: Some("anthropic".into()),
            ..Default::default()
        },
    );
    let l = home.launcher(cfg, None);
    let _m = MockScript::set(DONE);
    let status = subagent(&home, &l, "child", None).await;
    assert_eq!(
        run_start(status.transcript.as_deref().unwrap()),
        ("cfg-provider".to_string(), "cfg-model".to_string())
    );
}

/// The agent's own pins win over config's defaults.
#[tokio::test]
#[serial]
async fn dispatch_agent_frontmatter_overrides_config_defaults() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: pinned-provider\nmodel: pinned-model\nmaxTurns: 3",
    );
    let mut cfg = config();
    cfg.default_provider = Some("cfg-provider".into());
    cfg.default_model = Some("cfg-model".into());
    for p in ["cfg-provider", "pinned-provider"] {
        cfg.providers.insert(
            p.into(),
            rupu_config::ProviderConfig {
                kind: Some("anthropic".into()),
                ..Default::default()
            },
        );
    }
    let l = home.launcher(cfg, None);
    let _m = MockScript::set(DONE);
    let status = subagent(&home, &l, "child", None).await;
    assert_eq!(
        run_start(status.transcript.as_deref().unwrap()),
        ("pinned-provider".to_string(), "pinned-model".to_string())
    );
}

/// A config-declared openai-compatible provider is reachable from a child,
/// and its `default_model` beats the global default (ISSUES.md I-3).
#[tokio::test]
#[serial]
async fn dispatch_prefers_provider_scoped_default_model_over_global_default() {
    let home = Home::new();
    home.agent("child", "provider: oracle\nmaxTurns: 3");
    let mut cfg = config();
    cfg.default_model = Some("global-default-model".into());
    cfg.providers.insert(
        "oracle".into(),
        rupu_config::ProviderConfig {
            kind: Some("openai-compatible".into()),
            base_url: Some("https://example.invalid/v1".into()),
            default_model: Some("oracle-default".into()),
            ..Default::default()
        },
    );
    let l = home.launcher(cfg, None);
    let _m = MockScript::set(DONE);
    let status = subagent(&home, &l, "child", None).await;
    assert_eq!(
        run_start(status.transcript.as_deref().unwrap()),
        ("oracle".to_string(), "oracle-default".to_string())
    );
}

/// An undeclared provider fails the child (not the parent) with a config
/// error, before any credential lookup; the live view sees it start and fail.
#[tokio::test]
#[serial]
async fn an_undeclared_provider_fails_the_child() {
    let home = Home::new();
    home.agent("child", "provider: no-such-account\nmaxTurns: 3");
    let recorder = Arc::new(Recorder::default());
    let l = home.launcher(config(), Some(recorder.clone()));
    let _m = MockScript::set(DONE);
    let status = subagent(&home, &l, "child", None).await;
    assert_eq!(status.state, ChildState::Failed);
    assert!(
        status.error.as_deref().unwrap().contains("no-such-account"),
        "{status:?}"
    );
    let seen = recorder.0.lock().unwrap().clone();
    assert!(matches!(
        seen.first(),
        Some((_, WfEvent::DispatchStarted { .. }))
    ));
    assert!(
        matches!(
            seen.last(),
            Some((_, WfEvent::DispatchCompleted { success: false, .. }))
        ),
        "{seen:?}"
    );
}

/// A sub-agent gets its own agent's skills, never its parent's (skills spec
/// §2.3): the parent's run is indexed, the child's has no index, no loader,
/// and the parent's skill files are outside what its read tools reach.
#[tokio::test]
#[serial]
async fn a_subagent_does_not_inherit_its_parents_skills() {
    let home = Home::new();
    let reference = home
        .skill("pdf", "PDF things.")
        .canonicalize()
        .unwrap()
        .join("references/REF.md");
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-haiku-4-5\ntools: [\"*\"]",
    );
    home.agent(
        "parent",
        "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [dispatch_agent]\n\
         dispatchableAgents: [child]\nskills: [pdf]",
    );
    let assembler = home.assembler(config());
    let l = home.launcher_with(
        assembler.clone(),
        Arc::new(rupu_launch::testing::ScriptedProcesses::done("unused")),
        None,
    );
    let turn = |id: &str, tool: &str, input: serde_json::Value| {
        serde_json::json!([
            { "AssistantToolUse": { "text": null, "tool_id": id, "tool_name": tool, "tool_input": input, "stop": "tool_use" } },
            { "AssistantText": { "text": "done", "stop": "end_turn" } }
        ])
    };
    let _m = MockScript::set(
        &serde_json::json!({
            "turns": turn("p1", "dispatch_agent", serde_json::json!({ "agent": "child", "prompt": "go" })),
            "models": {
                "claude-haiku-4-5": turn("k1", "read_file", serde_json::json!({ "path": reference })),
            }
        })
        .to_string(),
    );
    let parent_path = home.root().join("run_parent.jsonl");
    rupu_runtime::run_agent(
        &assembler,
        rupu_runtime::assembly::LaunchSpec {
            agent: rupu_agent::load_agent(&home.global(), None, "parent").unwrap(),
            origin: rupu_runtime::assembly::Origin::Standalone,
            run_id: "run_parent".into(),
            transcript_path: parent_path.clone(),
            codename: None,
            prompt: rupu_agent::UserTurn::new("go"),
            workspace: home.binding(),
            mode: PermissionMode::Bypass,
            ceiling: None,
            prompter: None,
            overrides: Default::default(),
            stream: Default::default(),
            hooks: Default::default(),
            pause: None,
            services: rupu_runtime::assembly::LaunchServices {
                launcher: Some(l.clone() as Arc<dyn rupu_tools::Launcher>),
                ..Default::default()
            },
            collectors: Vec::new(),
            link: None,
        },
    )
    .await
    .unwrap()
    .result
    .unwrap();

    let indexed = |evs: &[Event]| {
        evs.iter()
            .filter(|e| matches!(e, Event::SkillIndex { .. }))
            .count()
    };
    assert_eq!(indexed(&events(&parent_path)), 1);
    let children = rupu_launch::ledger::read(&home.runs().join("run_parent"));
    let child = children.first().expect("the parent launched its child");
    let evs = events(
        child
            .started
            .transcript
            .as_deref()
            .expect("a sub-agent transcript"),
    );
    let prompt = evs
        .iter()
        .find_map(|e| match e {
            Event::RunStart { system_prompt, .. } => system_prompt.clone(),
            _ => None,
        })
        .expect("the child's run_start");
    assert!(!prompt.contains("## Skill"), "{prompt}");
    assert_eq!(indexed(&evs), 0);
    let offered = evs
        .iter()
        .find_map(|e| match e {
            Event::ToolGrant { entries, .. } => Some(entries.clone()),
            _ => None,
        })
        .expect("the child's tool_grant");
    assert!(offered.iter().all(|e| e.tool != "skills.load"));
    let read = evs
        .iter()
        .find_map(|e| match e {
            Event::ToolResult { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("the child's read_file result");
    assert!(
        read.is_some(),
        "the parent's skill files are not the child's to read"
    );
}
