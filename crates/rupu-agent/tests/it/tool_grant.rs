//! W2 end to end through the runner: the run's `tool_grant` event, ambient
//! grants, `actions:` narrowing, the `tool_unavailable` notice, an audit line
//! for every call, and unknown `tools:` names failing at load.

use rupu_agent::runner::{CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{CatalogMode, ConcernsBlock, ConcernsEntry, IncludeDirective};
use rupu_providers::types::StopReason;
use rupu_tools::{PermissionMode, PermissionPolicy, ToolContext};
use rupu_transcript::{Event, JsonlReader};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Run {
    agent_tools: Option<Vec<String>>,
    step_actions: Vec<String>,
    concerns: Option<ConcernsBlock>,
    scm: bool,
    permission: PermissionPolicy,
    script: Vec<ScriptedTurn>,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            agent_tools: None,
            step_actions: Vec::new(),
            concerns: None,
            scm: false,
            permission: PermissionPolicy::bypass(),
            script: vec![done()],
        }
    }
}

fn done() -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

fn call(id: &str, tool: &str, input: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: tool.into(),
        tool_input: input,
        stop: StopReason::ToolUse,
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Run `r` in `workspace`; returns the tool names the model was offered on
/// its first request and the transcript's events.
async fn run(workspace: &Path, r: Run) -> (Vec<String>, Vec<Event>) {
    let provider = CapturingMockProvider::new(r.script);
    let captured = provider.captured.clone();
    let transcript: PathBuf = workspace.join("run.jsonl");
    let opts = rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "test".into(),
            prompt: rupu_agent::UserTurn::new("go"),
            provider: Box::new(provider),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: r.permission,
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: {
                let mut tc = ToolContext::in_workspace(workspace);
                tc.identity = std::sync::Arc::new(rupu_tools::RunIdentity {
                    agent: "granted".into(),
                    provider: "mock".into(),
                    model: "mock-1".into(),
                    run_id: "run_tool_grant".into(),
                    ..Default::default()
                });
                tc.workspace.id = "ws_grant".into();
                tc.workspace.path = workspace.to_path_buf();
                tc.services.scm = r.scm.then(|| Arc::new(rupu_scm::Registry::empty()));
                tc
            },
            pins: Default::default(),
            concerns: r.concerns,
            max_turns: 10,
            stream: rupu_agent::StreamOpts {
                no_stream: true,
                suppress_stdout: true,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
            transcript_path: transcript.clone(),
        },
        r.agent_tools.as_deref(),
        &r.step_actions,
    )
    .expect("grant");
    let _ = run_agent(opts).await;
    let offered = captured
        .lock()
        .unwrap()
        .first()
        .map(|req| req.tools.iter().map(|t| t.name.clone()).collect())
        .unwrap_or_default();
    let events = JsonlReader::iter(&transcript)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    (offered, events)
}

fn grant_of(events: &[Event]) -> &Event {
    events
        .iter()
        .find(|e| matches!(e, Event::ToolGrant { .. }))
        .expect("every run writes a tool_grant")
}

fn reasons_for<'a>(events: &'a [Event], tool: &str) -> &'a [String] {
    match grant_of(events) {
        Event::ToolGrant { entries, .. } => {
            &entries
                .iter()
                .find(|e| e.tool == tool)
                .unwrap_or_else(|| panic!("{tool} not in the grant: {entries:?}"))
                .reasons
        }
        _ => unreachable!(),
    }
}

/// `(tool, decision)` for every `tool_audit` line, in order.
fn audits(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ToolAudit { tool, decision, .. } => {
                Some((tool.clone(), decision.clone().unwrap_or_default()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_grant_event_is_exactly_what_the_model_sees() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (offered, events) = run(
        tmp.path(),
        Run {
            agent_tools: Some(strs(&["read_file", "scm.*"])),
            scm: true,
            ..Default::default()
        },
    )
    .await;
    match grant_of(&events) {
        Event::ToolGrant { entries, .. } => {
            let mut granted: Vec<String> = entries.iter().map(|e| e.tool.clone()).collect();
            let mut offered = offered.clone();
            granted.sort();
            offered.sort();
            assert_eq!(granted, offered);
        }
        _ => unreachable!(),
    }
    assert_eq!(reasons_for(&events, "read_file"), ["declared"]);
    assert_eq!(reasons_for(&events, "scm.prs.get"), ["declared:scm.*"]);
}

#[tokio::test]
async fn ambient_is_visible() {
    let tmp = tempfile::TempDir::new().unwrap();
    let concerns = ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto,
            filter: None,
        })],
    };
    let (offered, events) = run(
        tmp.path(),
        Run {
            agent_tools: Some(strs(&["read_file"])),
            concerns: Some(concerns),
            ..Default::default()
        },
    )
    .await;
    for t in [
        "coverage.mark",
        "coverage.status",
        "coverage.remaining",
        "coverage.concerns.search",
        "coverage.concerns.detail",
        "findings.report",
    ] {
        assert!(offered.iter().any(|o| o == t), "{t} offered: {offered:?}");
        assert_eq!(reasons_for(&events, t), ["ambient:concerns"], "{t}");
    }
    assert_eq!(reasons_for(&events, "read_file"), ["declared"]);
}

#[tokio::test]
async fn star_with_actions_narrows_connectors_only() {
    // T8 regression through the runner: `*` keeps every non-connector tool
    // under a step's `actions:`, and only the named connector survives.
    let tmp = tempfile::TempDir::new().unwrap();
    let (offered, events) = run(
        tmp.path(),
        Run {
            agent_tools: Some(strs(&["*"])),
            step_actions: strs(&["issues.get"]),
            scm: true,
            ..Default::default()
        },
    )
    .await;
    for t in ["bash", "write_file", "issues.get", "findings.report"] {
        assert!(offered.iter().any(|o| o == t), "{t} missing: {offered:?}");
    }
    assert!(
        !offered
            .iter()
            .any(|o| o == "issues.list" || o == "scm.prs.get"),
        "{offered:?}"
    );
    match grant_of(&events) {
        Event::ToolGrant { narrowed, .. } => {
            assert!(narrowed.iter().any(|n| n == "scm.prs.get"), "{narrowed:?}")
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_named_tool_without_its_service_gets_a_notice_and_is_not_offered() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (offered, events) = run(
        tmp.path(),
        Run {
            agent_tools: Some(strs(&["read_file", "board.post", "*"])),
            ..Default::default()
        },
    )
    .await;
    assert!(
        !offered.iter().any(|o| o.starts_with("board.")),
        "{offered:?}"
    );
    let notices: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            Event::Notice { kind, message } if kind == "tool_unavailable" => Some(message),
            _ => None,
        })
        .collect();
    // One notice: board.post was named; the other board.* tools `*` matched
    // are skipped silently.
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("board.post") && notices[0].contains("message_bus"));
    match grant_of(&events) {
        Event::ToolGrant {
            unavailable,
            skipped,
            ..
        } => {
            assert_eq!(unavailable.len(), 1);
            assert!(
                skipped.iter().any(|s| s.tool == "board.read"),
                "{skipped:?}"
            );
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn audit_every_call() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "hello\n").unwrap();
    let (_, events) = run(
        tmp.path(),
        Run {
            agent_tools: Some(strs(&["read_file", "write_file"])),
            permission: PermissionPolicy::unattended(PermissionMode::Readonly),
            script: vec![
                call("c1", "read_file", json!({ "path": "a.txt" })),
                call(
                    "c2",
                    "write_file",
                    json!({ "path": "b.txt", "content": "x" }),
                ),
                call("c3", "bash", json!({ "command": "true" })),
                done(),
            ],
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        audits(&events),
        vec![
            ("read_file".to_string(), "allowed".to_string()),
            ("write_file".to_string(), "denied:readonly".to_string()),
            ("bash".to_string(), "not_granted".to_string()),
        ]
    );
    // The allowed call carries its grant reason; the ungranted one none.
    let reasons: Vec<Option<String>> = events
        .iter()
        .filter_map(|e| match e {
            Event::ToolAudit { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasons,
        vec![Some("declared".into()), Some("declared".into()), None]
    );
    assert!(!tmp.path().join("b.txt").exists());
}

#[test]
fn an_unknown_name_fails_the_grant_before_any_run() {
    // An agent built outside the loader (a test, an embedder) still can't
    // run with a typo in `tools:`: its grant never resolves, so no run (and
    // no request) can be assembled.
    let ctx = ToolContext::in_workspace(".");
    let declared = strs(&["repot_finding"]);
    let err = rupu_agent::grant::resolve_run_grant(rupu_agent::grant::RunGrantInputs {
        declared: Some(&declared),
        step_actions: &[],
        concerns: false,
        injected: &[],
        tool_context: &ctx,
        alias_scope: Default::default(),
    })
    .expect_err("a typo in tools: is refused");
    assert!(err.to_string().contains("repot_finding"), "{err}");
}

#[test]
fn unknown_tool_fails_load() {
    let tmp = tempfile::TempDir::new().unwrap();
    let agents = tmp.path().join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("typo.md"),
        "---\nname: typo\ntools: [read_file, repot_finding]\n---\nhi\n",
    )
    .unwrap();
    std::fs::write(
        agents.join("fine.md"),
        "---\nname: fine\ntools: [\"*\", asset_mark]\n---\nhi\n",
    )
    .unwrap();

    let err = rupu_agent::load_agent(tmp.path(), None, "typo").unwrap_err();
    assert_eq!(
        err.to_string(),
        "agent `typo`: unknown tool \"repot_finding\" in tools: (did you mean \"report_finding\" → findings.report?)"
    );
    // One bad file doesn't stop another agent from loading, or the listing.
    rupu_agent::load_agent(tmp.path(), None, "fine").unwrap();
    assert_eq!(rupu_agent::load_agents(tmp.path(), None).unwrap().len(), 2);
    rupu_agent::find_agent(tmp.path(), None, "typo").unwrap();

    let checks = rupu_agent::check_agent_files(tmp.path(), None);
    let failed: Vec<_> = checks
        .iter()
        .filter(|c| c.error.is_some())
        .map(|c| c.name.clone().unwrap())
        .collect();
    assert_eq!(failed, vec!["typo".to_string()]);
}
