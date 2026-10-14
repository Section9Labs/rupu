//! W5 §6 test 1: the lead's and a fleet unit's model-facing tool lists
//! (name, description, input schema — what the provider is sent) are pinned
//! by a snapshot. W5 moved the flow tools into the catalog without changing a
//! byte; W7 changed the launch tools on purpose: the lead's `dispatch` /
//! `join` / `workflows.generate` are the unified catalog tools (the old
//! `run_workflow` is an alias of `dispatch`), and a unit, which now has the
//! flow's launcher, gets `dispatch` / `join` from its default grant.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rupu_agent::runner::CapturingMockProvider;
use rupu_agent::{ScriptedTurn, StopReason};
use rupu_agentiflow::{
    run_agentiflow, AgentiflowDef, GenerationCapability, LeadInputs, LeadProvider, ProviderFactory,
    RunAgentiflowOpts,
};
use rupu_providers::{LlmProvider, LlmRequest};
use rupu_runtime::assembly::{
    AssemblyContext, LaunchServices, LaunchSpec, Origin, PreparedProvider, RunAssembler,
    WorkspaceBinding,
};
use serde_json::{json, Value};

type Captured = Arc<Mutex<Vec<LlmRequest>>>;

fn done() -> Vec<ScriptedTurn> {
    vec![ScriptedTurn::AssistantText {
        text: "Done.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]
}

fn capturing() -> (CapturingMockProvider, Captured) {
    let p = CapturingMockProvider::new(done());
    let captured = p.captured.clone();
    (p, captured)
}

/// The first request's tools, as the model sees them, in the order sent.
fn toolset(captured: &Captured) -> Value {
    let reqs = captured.lock().unwrap();
    let req = reqs.first().expect("the run made a model call");
    Value::Array(
        req.tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect(),
    )
}

fn active() -> rupu_coverage::ActiveSet {
    rupu_coverage::builtin_registry()
        .unwrap()
        .active_set(&["network".to_string()])
        .unwrap()
}

fn started() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

/// One round, then the ceiling stops the flow.
fn def() -> AgentiflowDef {
    def_with("")
}

fn def_with(extra: &str) -> AgentiflowDef {
    AgentiflowDef::parse_str(&format!(
        "name: parity\n\
         lead: lead\n\
         engagement_profiles: [network]\n\
         goals:\n  \
           - id: any-finding\n    \
             objective: \"Record at least one finding.\"\n    \
             target: {{ findings: {{}}, count_gte: 1 }}\n\
         scope: {{ authorized: true }}\n\
         pool: {{ agents: [lead, recon] }}\n\
         round: {{ ceiling: {{ rounds: 1 }} }}\n\
         {extra}"
    ))
    .unwrap()
}

fn lead_toolset(lead_tools: &str) -> Value {
    lead_toolset_of(def(), lead_tools)
}

fn lead_toolset_of(def: AgentiflowDef, lead_tools: &str) -> Value {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let captured: Captured = Arc::default();
    let shared = captured.clone();
    let make: ProviderFactory = Box::new(move || -> Box<dyn LlmProvider> {
        let mut p = CapturingMockProvider::new(done());
        p.captured = shared.clone();
        Box::new(p)
    });
    let generation = GenerationCapability {
        provider: "mock".into(),
        model: "mock-1".into(),
        factory: Arc::new(|| -> Box<dyn LlmProvider> {
            Box::new(CapturingMockProvider::new(done()))
        }),
    };
    run_agentiflow(RunAgentiflowOpts {
        def,
        workspace: workspace.clone(),
        global: global.clone(),
        active: active(),
        started: started(),
        now: Box::new(started),
        assembler: RunAssembler::new(AssemblyContext::minimal(global.clone())),
        workspace_id: "ws_parity".into(),
        lead: LeadInputs {
            agent: rupu_agent::AgentSpec::parse(&format!(
                "---\nname: lead\ntools: {lead_tools}\n---\nYou are the lead.\n"
            ))
            .unwrap(),
            provider: LeadProvider::Injected {
                provider: "mock".into(),
                model: "mock-1".into(),
                make,
            },
        },
        run_id: "af_parity".into(),
        process_backend: Some(Arc::new(rupu_launch::testing::ScriptedProcesses::scripted(
            vec![],
        ))),
        generation: Some(generation),
        pricing: Default::default(),
    })
    .unwrap();
    toolset(&captured)
}

/// The services `rupu run --message-space <dir> --participant <p>` launches
/// its unit with (`cmd/run.rs`): its bus (the assembler attaches its message
/// collectors), and a launcher like every `rupu run`'s.
fn unit_launch(
    global: &Path,
    run_dir: &Path,
    participant: &str,
) -> (
    LaunchServices,
    Vec<Arc<dyn rupu_agent::TurnCollector>>,
    String,
) {
    let s = rupu_agentiflow::FlowUnitServices::for_participant(
        run_dir,
        participant,
        Default::default(),
    );
    let launcher = rupu_launch::RunLauncher::new(rupu_launch::LaunchEnv {
        assembler: Arc::new(RunAssembler::new(AssemblyContext::minimal(
            global.to_path_buf(),
        ))),
        store: Arc::new(rupu_orchestrator::RunStore::new(global.join("runs"))),
        events: None,
        namer: None,
        backend: Arc::new(rupu_launch::testing::ScriptedProcesses::scripted(vec![])),
        runtime: tokio::runtime::Handle::current(),
        max_concurrent: 8,
        generation: None,
        project: None,
    });
    (
        LaunchServices {
            message_bus: Some(s.message_bus),
            launcher: Some(launcher),
            ..Default::default()
        },
        Vec::new(),
        s.flow_id,
    )
}

async fn unit_toolset(unit_frontmatter: &str) -> Value {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let workspace = tmp.path().join("ws");
    let run_dir = global.join("agentiflows").join("af_parity");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&run_dir).unwrap();

    let agent = rupu_agent::AgentSpec::parse(&format!(
        "---\nname: recon\n{unit_frontmatter}---\nYou survey.\n"
    ))
    .unwrap();
    let (provider, captured) = capturing();
    let (mut services, collectors, flow_id) = unit_launch(&global, &run_dir, "recon#1");
    services.provider = Some(PreparedProvider::injected(
        "mock",
        "mock-1",
        Box::new(provider),
        &agent,
        None,
    ));
    let assembler = RunAssembler::new(AssemblyContext::minimal(global.clone()));
    let spec = LaunchSpec {
        agent,
        origin: Origin::FlowUnit {
            flow_id,
            flow_dir: run_dir.clone(),
            participant: "recon#1".into(),
        },
        run_id: "run_unit".into(),
        transcript_path: tmp.path().join("unit.jsonl"),
        codename: None,
        prompt: rupu_agent::UserTurn {
            message: "scan".into(),
            ..Default::default()
        },
        workspace: WorkspaceBinding {
            id: "ws_parity".into(),
            path: workspace,
        },
        mode: rupu_tools::PermissionMode::Bypass,
        ceiling: None,
        prompter: None,
        overrides: Default::default(),
        stream: rupu_agent::StreamOpts {
            suppress_stdout: true,
            ..Default::default()
        },
        hooks: Default::default(),
        pause: None,
        services,
        collectors,
        link: None,
    };
    rupu_runtime::run_agent(&assembler, spec)
        .await
        .expect("assembles")
        .result
        .expect("runs");
    toolset(&captured)
}

/// Spec F1d D3: an agentiflow's `messaging: false` leaves its lead no
/// `board.*` / `msg.send`, the rest of its coordination set intact. Absent,
/// the lead's list is today's (the snapshot above).
#[test]
fn agentiflow_messaging_false_strips_the_leads_messaging_tools() {
    let names: Vec<String> = lead_toolset_of(def_with("messaging: false\n"), "[read_file]")
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !names
            .iter()
            .any(|n| n.starts_with("board.") || n.starts_with("msg.")),
        "{names:?}"
    );
    for kept in ["read_file", "goal.status", "budget.status", "dispatch", "agents.list"] {
        assert!(names.iter().any(|n| n == kept), "{kept} in {names:?}");
    }
    let on: Vec<String> = lead_toolset_of(def_with("messaging: true\n"), "[read_file]")
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(on.iter().any(|n| n == "board.directive"), "{on:?}");
}

#[test]
fn lead_toolset_snapshot() {
    insta::assert_yaml_snapshot!("lead_no_declared_tools", lead_toolset("[]"));
    insta::assert_yaml_snapshot!("lead_declares_read_file", lead_toolset("[read_file]"));
}

#[tokio::test]
async fn unit_toolset_snapshot() {
    insta::assert_yaml_snapshot!("unit_default_grant", unit_toolset("").await);
    insta::assert_yaml_snapshot!(
        "unit_declares_read_file",
        unit_toolset("tools: [read_file]\n").await
    );
}
