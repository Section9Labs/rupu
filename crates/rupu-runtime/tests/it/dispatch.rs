//! `InProcessDispatcher` end to end (moved here with the dispatcher from
//! `rupu-cli`, W3): each test dispatches a child through the real run
//! assembler and agent loop, with the provider factory's mock seam.

use std::sync::Arc;

use rupu_runtime::dispatch::{child_codename, DispatchRoot, InProcessDispatcher};
use rupu_runtime::usage_ledger::{LedgerRow, UsageLedger};
use rupu_tools::{AgentDispatcher, PermissionMode, SpawnPermission};
use rupu_transcript::{Event, RunMode};
use serial_test::serial;

use crate::support::{
    bypass, config, events, parent, run_start, DirSubRuns, Home, MockScript, Recorder, Seen, DONE,
};

fn dispatcher(
    home: &Home,
    cfg: rupu_config::Config,
    root: DispatchRoot,
    events: Option<Arc<Recorder>>,
) -> Arc<InProcessDispatcher> {
    InProcessDispatcher::new(
        home.assembler(cfg),
        Arc::new(DirSubRuns(home.runs())),
        root,
        events.map(|e| e as _),
    )
}

fn root(home: &Home) -> DispatchRoot {
    DispatchRoot {
        workspace: home.binding(),
        ..Default::default()
    }
}

#[tokio::test]
#[serial]
async fn readonly_subagent_is_capped() {
    let home = Home::new();
    home.agent(
        "writer",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\npermissionMode: bypass",
    );
    let d = dispatcher(&home, config(), root(&home), None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "w1", "tool_name": "write_file", "tool_input": {"path": "escaped.txt", "content": "x"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "done", "stop": "end_turn" } }
        ]"#,
    );
    let readonly_parent = SpawnPermission {
        ceiling: PermissionMode::Readonly,
        prompter: None,
    };
    let outcome = d
        .dispatch("writer", "write it".into(), &parent(None), readonly_parent)
        .await
        .expect("the denied call does not fail the dispatch");
    assert!(
        !home.workspace().join("escaped.txt").exists(),
        "a readonly parent's child must not write"
    );
    let evs = events(&outcome.transcript_path);
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

/// `DispatchStarted` before `DispatchCompleted`, both with the returned
/// sub-run id; the child's calls land in the root's ledger, attributed to
/// the dispatching run.
#[tokio::test]
#[serial]
async fn dispatch_emits_started_then_completed_and_charges_the_roots_ledger() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let ledger_path = home.root().join("root-usage.jsonl");
    let recorder = Arc::new(Recorder::default());
    let d = dispatcher(
        &home,
        config(),
        DispatchRoot {
            workspace: home.binding(),
            usage: Some(UsageLedger::open(ledger_path.clone())),
            coverage_stream: None,
        },
        Some(recorder.clone()),
    );
    let _m = MockScript::set(DONE);
    let outcome = d
        .dispatch("child", "do the thing".into(), &parent(None), bypass())
        .await
        .expect("dispatch succeeds");

    let seen = recorder.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    match &seen[0] {
        Seen::Started(run_id, c) => {
            assert_eq!(run_id, "parent_run_1");
            assert_eq!(c.sub_run_id, outcome.sub_run_id);
            assert_eq!(c.agent, "child");
            assert_eq!(c.transcript_path, outcome.transcript_path);
            assert_eq!(
                (c.provider.as_str(), c.model.as_str()),
                ("anthropic", "claude-sonnet-4-6")
            );
        }
        other => panic!("Started first, got {other:?}"),
    }
    match &seen[1] {
        Seen::Completed(run_id, done) => {
            assert_eq!(run_id, "parent_run_1");
            assert_eq!(done.sub_run_id, outcome.sub_run_id);
            assert!(done.success);
            assert_eq!((done.tokens_in, done.tokens_out), (1, 1));
        }
        other => panic!("Completed second, got {other:?}"),
    }

    let rows: Vec<LedgerRow> = std::fs::read_to_string(&ledger_path)
        .expect("the child appended to the root's ledger")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(!rows.is_empty());
    for row in &rows {
        assert_eq!(row.step_id, None);
        assert_eq!(row.agent_run_id, outcome.sub_run_id);
        assert_eq!(row.parent_agent_run_id.as_deref(), Some("parent_run_1"));
        assert_eq!(row.agent, "child");
        assert_eq!(row.transcript, outcome.transcript_path);
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
    let d = dispatcher(&home, config(), root(&home), None);
    let _m = MockScript::set(DONE);
    let outcome = d
        .dispatch("pinnedchild", "go".into(), &parent(None), bypass())
        .await
        .unwrap();
    let t = std::fs::read_to_string(&outcome.transcript_path).unwrap();
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
    let d = InProcessDispatcher::new(
        Arc::new(rupu_runtime::assembly::RunAssembler::new(ctx)),
        Arc::new(DirSubRuns(home.runs())),
        root(&home),
        None,
    );
    let _m = MockScript::set(DONE);
    let outcome = d
        .dispatch("child", "go".into(), &parent(None), bypass())
        .await
        .unwrap();
    let head = rupu_transcript::JsonlReader::head(&outcome.transcript_path).unwrap();
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
    let d = dispatcher(
        &home,
        config(),
        DispatchRoot {
            workspace: home.binding(),
            usage: None,
            coverage_stream: Some(stream.clone()),
        },
        None,
    );
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "report_finding", "tool_input": {"scope": "repo", "summary": "child found it", "severity": "low", "evidence": {"rationale": "because"}}, "stop": "tool_use" } },
          { "AssistantText": { "text": "child done", "stop": "end_turn" } }
        ]"#,
    );
    d.dispatch("child", "go".into(), &parent(None), bypass())
        .await
        .unwrap();

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
    // …and the ledger it lands in is the parent's target.
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
    let d = dispatcher(&home, cfg, root(&home), None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": null, "tool_id": "b1", "tool_name": "bash", "tool_input": {"command": "echo probe=$RUPU_W3_BASH_PROBE"}, "stop": "tool_use" } },
          { "AssistantText": { "text": "done", "stop": "end_turn" } }
        ]"#,
    );
    let outcome = d
        .dispatch("child", "go".into(), &parent(None), bypass())
        .await;
    std::env::remove_var("RUPU_W3_BASH_PROBE");
    let evs = events(&outcome.unwrap().transcript_path);
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::ToolResult { output, .. } if output.contains("probe=from-config")
        )),
        "the allowlisted variable reaches the child's bash: {evs:?}"
    );
}

#[test]
fn child_codename_numbers_per_parent_and_role() {
    let namer = rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new("jade-reef"));
    let a = child_codename(&namer, "jade-reef/hedgehog", "security-reviewer").unwrap();
    let b = child_codename(&namer, "jade-reef/hedgehog", "security-reviewer").unwrap();
    let c = child_codename(&namer, "jade-reef/heron", "security-reviewer").unwrap();
    assert_eq!(a, "jade-reef/hedgehog>ferret#1");
    assert_eq!(b, "jade-reef/hedgehog>ferret#2");
    assert_eq!(c, "jade-reef/heron>ferret#1");
    assert!(child_codename(&namer, "garbage", "x").is_none());
}

/// The child's `<parent>><role>#n` comes from the INSTALLED namer, is
/// stamped on `DispatchStarted` and the child's `RunStart`; a parent with no
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
    let d = dispatcher(&home, config(), root(&home), Some(recorder.clone()));
    let namer = rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new("jade-reef"));
    d.set_namer(namer.clone());

    let mut outcomes = Vec::new();
    for codename in [Some("jade-reef/heron"), Some("jade-reef/heron"), None] {
        let _m = MockScript::set(DONE);
        outcomes.push(
            d.dispatch(
                "security-reviewer",
                "go".into(),
                &parent(codename),
                bypass(),
            )
            .await
            .unwrap(),
        );
    }
    assert_eq!(
        outcomes[0].codename.as_deref(),
        Some("jade-reef/heron>ferret#1")
    );
    assert_eq!(
        outcomes[1].codename.as_deref(),
        Some("jade-reef/heron>ferret#2")
    );
    assert_eq!(outcomes[2].codename, None);
    let p: rupu_codename::Codename = "jade-reef/heron".parse().unwrap();
    assert_eq!(namer.with(|n| n.next_instance(&p, "ferret")), 3);

    let started: Vec<_> = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|s| match s {
            Seen::Started(_, c) => Some(c.codename.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec![
            Some("jade-reef/heron>ferret#1".to_string()),
            Some("jade-reef/heron>ferret#2".to_string()),
            None
        ]
    );
    let codename = events(&outcomes[0].transcript_path)
        .into_iter()
        .find_map(|e| match e {
            Event::RunStart { codename, .. } => Some(codename),
            _ => None,
        })
        .unwrap();
    assert_eq!(codename.as_deref(), Some("jade-reef/heron>ferret#1"));
}

/// No namer installed: the child is still named, from an in-memory namer.
#[tokio::test]
#[serial]
async fn dispatch_without_installed_namer_still_names_child() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let d = dispatcher(&home, config(), root(&home), None);
    let _m = MockScript::set(DONE);
    let r = d
        .dispatch(
            "child",
            "go".into(),
            &parent(Some("amber-lantern/otter")),
            bypass(),
        )
        .await
        .unwrap();
    let role = rupu_codename::role_word("child");
    assert_eq!(r.codename, Some(format!("amber-lantern/otter>{role}#1")));
}

/// Spec 2026-10-01 §5.3: a failed child reports `ok: false`, an EMPTY
/// output and its error, without the rung-3 hint (its transcript keeps it).
#[tokio::test]
#[serial]
async fn a_failed_child_returns_no_output_and_its_error() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let d = dispatcher(&home, config(), root(&home), None);
    let _m = MockScript::set(
        r#"[
          { "AssistantToolUse": { "text": "Let me read the settings first", "tool_id": "call_1", "tool_name": "read_file", "tool_input": { "path": "absent.toml" }, "stop": "tool_use" } },
          { "Reply": {
              "content": [{ "type": "text", "text": "I won't do that." }],
              "stop": { "reason": "refusal", "wire": { "provider": "anthropic", "value": "refusal" } }
          } }
        ]"#,
    );
    let outcome = d
        .dispatch("child", "do the thing".into(), &parent(None), bypass())
        .await
        .expect("a failed child is an outcome, not a dispatch error");
    assert!(!outcome.success);
    assert_eq!(outcome.output, "");
    let error = outcome.error.unwrap();
    assert!(error.contains("refused"), "{error}");
    assert!(
        !error.contains("no recovery left") && !error.contains("--continue"),
        "{error}"
    );
    let complete = events(&outcome.transcript_path)
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
    let d = dispatcher(&home, cfg, root(&home), None);
    let _m = MockScript::set(DONE);
    let outcome = d
        .dispatch("child", "go".into(), &parent(None), bypass())
        .await
        .unwrap();
    assert_eq!(
        run_start(&outcome.transcript_path),
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
    let d = dispatcher(&home, cfg, root(&home), None);
    let _m = MockScript::set(DONE);
    let outcome = d
        .dispatch("child", "go".into(), &parent(None), bypass())
        .await
        .unwrap();
    assert_eq!(
        run_start(&outcome.transcript_path),
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
    let d = dispatcher(&home, cfg, root(&home), None);
    let _m = MockScript::set(DONE);
    let outcome = d
        .dispatch("child", "go".into(), &parent(None), bypass())
        .await
        .unwrap();
    assert_eq!(
        run_start(&outcome.transcript_path),
        ("oracle".to_string(), "oracle-default".to_string())
    );
}

/// An undeclared provider fails the dispatch with a config error, before any
/// credential lookup.
#[tokio::test]
#[serial]
async fn an_undeclared_provider_fails_the_dispatch() {
    let home = Home::new();
    home.agent("child", "provider: no-such-account\nmaxTurns: 3");
    let recorder = Arc::new(Recorder::default());
    let d = dispatcher(&home, config(), root(&home), Some(recorder.clone()));
    let _m = MockScript::set(DONE);
    let err = d
        .dispatch("child", "go".into(), &parent(None), bypass())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no-such-account"), "{err}");
    // The live view still sees the child start and fail.
    let seen = recorder.0.lock().unwrap().clone();
    assert!(
        matches!(seen.last(), Some(Seen::Completed(_, d)) if !d.success),
        "{seen:?}"
    );
}

/// IMPORTANT 4 fallback: the child's own transcript records that the parent
/// step's `actions:` narrowing did not apply to it; the final answer is
/// unaffected.
#[tokio::test]
#[serial]
async fn dispatch_leaves_a_visible_delegation_narrowing_notice_on_the_child_transcript() {
    let home = Home::new();
    home.agent(
        "child",
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3",
    );
    let d = dispatcher(&home, config(), root(&home), None);
    let _m = MockScript::set(DONE);
    let outcome = d
        .dispatch("child", "go".into(), &parent(None), bypass())
        .await
        .unwrap();
    let evs = events(&outcome.transcript_path);
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::ToolCall { tool, .. } if tool == "dispatch_agent_narrowing_notice"
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::ToolResult { output, error: None, .. } if output.contains("KNOWN LIMITATION")
    )));
    assert_eq!(outcome.output, "child done");
}
