//! W5 §6 test 1: the lead's and a fleet unit's model-facing tool lists
//! (name, description, input schema — what the provider is sent) are pinned
//! by a snapshot taken before the flow tools moved into the catalog. Moving
//! them must not change a byte of it. (`coverage.status` was already renamed
//! `goal.coverage` in W1, so the lead's list carries no rename either; its
//! old name still resolves for the lead — `lead_resolves_coverage_status`.)

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rupu_agent::runner::CapturingMockProvider;
use rupu_agent::{ScriptedTurn, StopReason};
use rupu_agentiflow::{
    run_agentiflow, AgentiflowDef, GenerationCapability, LeadInputs, LeadProvider,
    MockUnitLauncher, ProviderFactory, RunAgentiflowOpts,
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
    AgentiflowDef::parse_str(
        "name: parity\n\
         lead: lead\n\
         engagement_profiles: [network]\n\
         goals:\n  \
           - id: any-finding\n    \
             objective: \"Record at least one finding.\"\n    \
             target: { findings: {}, count_gte: 1 }\n\
         scope: { authorized: true }\n\
         pool: { agents: [lead, recon] }\n\
         round: { ceiling: { rounds: 1 } }\n",
    )
    .unwrap()
}

fn lead_toolset(lead_tools: &str) -> Value {
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
        def: def(),
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
        unit_launcher: Some(Arc::new(MockUnitLauncher::scripted(vec![]))),
        generation: Some(generation),
        pricing: Default::default(),
    })
    .unwrap();
    toolset(&captured)
}

/// The services `rupu run --fleet-run-dir <dir> --fleet-participant <p>`
/// launches its unit with (`cmd/run.rs`).
fn unit_launch(
    run_dir: &Path,
    participant: &str,
) -> (
    LaunchServices,
    Vec<Arc<dyn rupu_agent::TurnCollector>>,
    String,
) {
    let s = rupu_agentiflow::FlowUnitServices::for_participant(run_dir, participant);
    (
        LaunchServices {
            extra_tools: s.extra_tools,
            ..Default::default()
        },
        s.collectors,
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
    let (mut services, collectors, flow_id) = unit_launch(&run_dir, "recon#1");
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
    };
    rupu_runtime::run_agent(&assembler, spec)
        .await
        .expect("assembles")
        .result
        .expect("runs");
    toolset(&captured)
}

#[test]
fn lead_toolset_is_unchanged() {
    insta::assert_yaml_snapshot!("lead_no_declared_tools", lead_toolset("[]"));
    insta::assert_yaml_snapshot!("lead_declares_read_file", lead_toolset("[read_file]"));
}

#[tokio::test]
async fn unit_toolset_is_unchanged() {
    insta::assert_yaml_snapshot!("unit_default_grant", unit_toolset("").await);
    insta::assert_yaml_snapshot!(
        "unit_declares_read_file",
        unit_toolset("tools: [read_file]\n").await
    );
}
