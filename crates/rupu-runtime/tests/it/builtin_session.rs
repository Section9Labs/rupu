//! The bundled session is a real runtime grant with project catalog dispatch,
//! coverage and findings services. No configured provider or live job is used.
use std::sync::Arc;

use rupu_agent::builtin::{session_agent, SESSION_AGENT_NAME};
use rupu_runtime::assembly::{AssembleError, LaunchSpec, Origin, ParentRun, RunAssembler};
use rupu_tools::ports::{AgentEntry, WorkflowEntry};
use rupu_tools::{CatalogPort, PermissionMode, PortError};
use serial_test::serial;

use crate::assembly::{agent, launch};
use crate::support::{config, events, Home, MockScript, NoLaunch, DONE};

struct ProjectCatalog {
    broken_agents: bool,
}
impl CatalogPort for ProjectCatalog {
    fn agents(&self) -> Result<Vec<AgentEntry>, PortError> {
        if self.broken_agents {
            return Err(PortError::new("broken project agent"));
        }
        Ok(["reviewer", SESSION_AGENT_NAME]
            .into_iter()
            .map(|name| AgentEntry {
                name: name.into(),
                ..Default::default()
            })
            .collect())
    }
    fn workflows(&self) -> Vec<WorkflowEntry> {
        vec![
            WorkflowEntry {
                id: "check-project".into(),
                ..Default::default()
            },
            WorkflowEntry {
                id: "broken".into(),
                parse_error: Some("invalid YAML".into()),
                ..Default::default()
            },
        ]
    }
}
fn session_origin(home: &Home) -> Origin {
    Origin::SessionTurn {
        session_id: "ses_harness".into(),
        session_dir: home.global().join("sessions/ses_harness"),
    }
}

fn child_launch(home: &Home, process_child: bool, strict: bool) -> LaunchSpec {
    let origin = if process_child {
        Origin::Standalone
    } else {
        Origin::SubAgent {
            parent: ParentRun {
                identity: Arc::new(rupu_tools::RunIdentity {
                    launch: rupu_tools::LaunchScope {
                        strict_approval: strict,
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                usage: None,
                coverage_stream: None,
                netflow_stream: None,
            },
        }
    };
    let mut spec = launch(
        home,
        agent(home, "permissionMode: ask"),
        origin,
        "run_child_ask",
    );
    if process_child {
        spec.link = Some(rupu_runtime::argv::LaunchLink {
            depth: 1,
            ceiling: PermissionMode::Bypass,
            parent_run: "run_parent".into(),
            root_run: "run_parent".into(),
            usage_root: None,
            agents: Vec::new(),
            workflows: Vec::new(),
            actions: None,
            no_messaging: false,
            strict_approval: strict,
        });
    } else {
        spec.ceiling = Some(PermissionMode::Bypass);
    }
    spec
}

#[tokio::test]
#[serial]
async fn builtin_session_has_real_services_grants_and_project_dispatch_names() {
    let home = Home::new();
    let mut context = home.context(config());
    context.catalog = Some(Arc::new(ProjectCatalog {
        broken_agents: false,
    }));
    let assembler = RunAssembler::new(context);
    let mut spec = launch(&home, session_agent(), session_origin(&home), "run_harness");
    spec.services.builtin_session_harness = true;
    spec.mode = PermissionMode::Readonly;
    spec.services.launcher = Some(Arc::new(NoLaunch));
    let _mock = MockScript::set(DONE);
    let run = assembler.assemble(spec).await.unwrap();
    assert_eq!(run.identity().launch.agents, ["reviewer"]);
    assert_eq!(run.identity().launch.workflows, ["check-project"]);
    assert_eq!(run.identity().scope(), "ses_harness");
    assert!(run.identity().launch.strict_approval);
    for name in [
        "read_file",
        "catalog.search",
        "agents.list",
        "workflows.get",
        "dispatch",
        "join",
        "coverage.mark",
        "coverage.status",
        "findings.query",
        "findings.report",
        "scm.files.read",
    ] {
        assert!(
            run.opts.grant.offers(name),
            "missing operational tool {name}"
        );
    }
    assert!(run.opts.concerns.is_some());
    assert!(run.opts.tool_context.services.scm.is_some());
    assert_eq!(run.opts.permission.mode(), PermissionMode::Readonly);
    assert_eq!(
        run.opts
            .permission
            .ceiling_for_child(Some(PermissionMode::Bypass)),
        PermissionMode::Readonly
    );
}

#[tokio::test]
#[serial]
async fn broken_catalog_does_not_require_local_agent_repair_before_session_can_start() {
    let home = Home::new();
    let mut context = home.context(config());
    context.catalog = Some(Arc::new(ProjectCatalog {
        broken_agents: true,
    }));
    let assembler = RunAssembler::new(context);
    let mut spec = launch(
        &home,
        session_agent(),
        session_origin(&home),
        "run_broken_catalog",
    );
    spec.services.builtin_session_harness = true;
    let _mock = MockScript::set(DONE);
    let run = assembler.assemble(spec).await.unwrap();
    assert!(run.identity().launch.agents.is_empty());
    assert_eq!(run.identity().launch.workflows, ["check-project"]);
}

#[tokio::test]
#[serial]
async fn builtin_catalog_binding_does_not_expand_an_existing_custom_session() {
    let home = Home::new();
    let mut context = home.context(config());
    context.catalog = Some(Arc::new(ProjectCatalog {
        broken_agents: false,
    }));
    let assembler = RunAssembler::new(context);
    let mut legacy = agent(
        &home,
        "dispatchableAgents: [explicit]\ndispatchableWorkflows: []",
    );
    legacy.name = SESSION_AGENT_NAME.into();
    let mut spec = launch(&home, legacy, session_origin(&home), "run_custom");
    spec.mode = PermissionMode::Ask;
    let _mock = MockScript::set(DONE);
    let run = assembler.assemble(spec).await.unwrap();
    assert_eq!(run.identity().launch.agents, ["explicit"]);
    assert!(run.identity().launch.workflows.is_empty());
    assert_eq!(run.opts.permission.mode(), PermissionMode::Ask);
}

#[tokio::test]
#[serial]
async fn headless_harness_and_delegated_ask_fail_before_provider_or_resources() {
    let home = Home::new();
    // No mock seam: unknown provider proves approval validation happens first.
    let mut cfg = config();
    cfg.default_provider = Some("no-such-provider".into());
    let assembler = home.assembler(cfg);
    let mut spec = launch(&home, session_agent(), session_origin(&home), "run_ask");
    spec.services.builtin_session_harness = true;
    spec.mode = PermissionMode::Ask;
    let transcript = spec.transcript_path.clone();
    assert!(matches!(
        assembler.assemble(spec).await,
        Err(AssembleError::ApprovalUnavailable)
    ));
    assert!(!transcript.exists());

    for process_child in [false, true] {
        let spec = child_launch(&home, process_child, true);
        assert!(matches!(
            assembler.assemble(spec).await,
            Err(AssembleError::ApprovalUnavailable)
        ));
    }
}

#[tokio::test]
#[serial]
async fn legacy_headless_child_keeps_ask_and_strict_readonly_child_stays_capped() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let _mock = MockScript::set(DONE);
    for process in [false, true] {
        for strict in [false, true] {
            let mut spec = child_launch(&home, process, strict);
            if strict {
                if let Some(link) = &mut spec.link {
                    link.ceiling = PermissionMode::Readonly;
                } else {
                    spec.ceiling = Some(PermissionMode::Readonly);
                }
            }
            let run = assembler.assemble(spec).await.unwrap();
            assert_eq!(
                run.opts.permission.mode(),
                if strict {
                    PermissionMode::Readonly
                } else {
                    PermissionMode::Ask
                }
            );
            assert_eq!(run.identity().launch.strict_approval, strict);
        }
    }
}

#[tokio::test]
#[serial]
async fn operator_workflow_marker_enforces_and_propagates_strict_approval_without_parent_link() {
    let home = Home::new();
    let run_dir = home.runs().join("run_operator_workflow");
    std::fs::create_dir_all(&run_dir).unwrap();
    rupu_runtime::strict_approval::require_workflow(&run_dir).unwrap();
    let origin = || Origin::WorkflowStep {
        workflow_run_id: "run_operator_workflow".into(),
        workflow_name: "check".into(),
        step_id: "inspect".into(),
        unit: None,
        step_actions: Vec::new(),
        scope_override: None,
        messaging: rupu_tools::MessagingSwitch::Default,
    };
    let assembler = home.assembler(config());
    let _mock = MockScript::set(DONE);
    let spec = launch(
        &home,
        agent(&home, "permissionMode: ask"),
        origin(),
        "run_strict_step_ask",
    );
    assert!(matches!(
        assembler.assemble(spec).await,
        Err(AssembleError::ApprovalUnavailable)
    ));
    let spec = launch(
        &home,
        agent(&home, "permissionMode: bypass"),
        origin(),
        "run_strict_step",
    );
    let run = assembler.assemble(spec).await.unwrap();
    assert!(run.identity().launch.strict_approval);
    assert!(run.identity().parent.is_none());
    assert_eq!(run.opts.permission.mode(), PermissionMode::Bypass);
}

#[tokio::test]
#[serial]
async fn readonly_harness_executes_catalog_and_coverage_but_denies_workspace_write() {
    use serde_json::json;
    let home = Home::new();
    std::fs::write(home.workspace().join("sample.rs"), "fn main() {}\n").unwrap();
    let tool = |id: &str, name: &str, input| {
        json!({ "AssistantToolUse": {
            "text": null, "tool_id": id, "tool_name": name, "tool_input": input, "stop": "tool_use"
        }})
    };
    let script = json!([
        tool("catalog", "catalog.search", json!({"query": "check"})),
        tool("read", "read_file", json!({"path": "sample.rs"})),
        tool("coverage", "coverage.mark", json!({"concern_id": "session:correctness", "file_path": "sample.rs", "status": "clean", "evidence": {"summary": "Inspected empty entry point"}})),
        tool("findings", "findings.query", json!({})),
        tool("write", "write_file", json!({"path": "denied.rs", "content": "must not write"})),
        {"AssistantText": {"text": "done", "stop": "end_turn"}}
    ]).to_string();
    let _mock = MockScript::set(&script);
    let assembler = home.assembler(config());
    let mut spec = launch(
        &home,
        session_agent(),
        session_origin(&home),
        "run_operational",
    );
    spec.services.builtin_session_harness = true;
    spec.mode = PermissionMode::Readonly;
    let transcript = spec.transcript_path.clone();
    let run = assembler.assemble(spec).await.unwrap();
    let outcome = run.run().await;
    outcome.result.unwrap();
    assert!(!home.workspace().join("denied.rs").exists());
    let transcript_events = events(&transcript);
    let results: Vec<_> = transcript_events
        .iter()
        .filter_map(|event| match event {
            rupu_transcript::Event::ToolResult { call_id, error, .. } => {
                Some((call_id.as_str(), error.as_deref()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![
            ("catalog", None),
            ("read", None),
            ("coverage", None),
            ("findings", None),
            ("write", Some("permission_denied"))
        ]
    );
    let serialized = serde_json::to_value(transcript_events).unwrap();
    let text = serialized.to_string();
    assert!(text.contains("denied:readonly"), "{text}");
    assert!(!text.contains("permission_mode_degraded"));
    let target = rupu_coverage::target_id(&home.workspace(), "ses_harness");
    let paths = rupu_coverage::CoveragePaths::new(&home.workspace(), &target);
    assert!(paths.catalog.exists());
    let marks = rupu_coverage::coverage_status(&paths, Default::default()).unwrap();
    assert_eq!(
        marks.len(),
        1,
        "the coverage tool must really write its session ledger"
    );
}
