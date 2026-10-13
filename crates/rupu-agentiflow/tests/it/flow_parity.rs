//! W7 §6 test 9 (`flow_parity`): the agentiflow lead's launch tools —
//! `dispatch {agent, prompt}`, `run_workflow`, `generate_workflow` and `join
//! {handle}` — are aliases of the catalog's `dispatch` / `join` /
//! `workflows.generate` over the flow's launcher, and keep their old
//! behaviour: the pool is the allowlist, refusals spawn nothing, every unit
//! gets a distinct participant and the flow's engagement, `join` answers the
//! old single-status shape.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rupu_agent::{MockProvider, ScriptedTurn};
use rupu_launch::testing::ScriptedProcesses;
use rupu_launch::{Generation, LaunchEnv, ProcOutcome, ProcState, RunLauncher};
use rupu_runtime::argv::{RunArgv, WorkflowRef};
use rupu_runtime::assembly::{AssemblyContext, RunAssembler};
use rupu_tools::launch::{DispatchTool, JoinTool, WorkflowsGenerateTool};
use rupu_tools::{LaunchPool, LaunchScope, PermissionMode, RunIdentity, Tool, ToolContext};
use serde_json::{json, Value};

struct Flow {
    _tmp: tempfile::TempDir,
    workspace: PathBuf,
    run_dir: PathBuf,
    backend: Arc<ScriptedProcesses>,
    ctx: ToolContext,
    minted: Arc<AtomicUsize>,
}

const LEAD: &str = "cobalt-harbor/heron";

impl Flow {
    /// A lead over a pool of `agents` and `workflows`, engagement `network`,
    /// at `depth`, whose units walk `script`, and whose generator answers
    /// `generated`.
    fn new(
        agents: &[&str],
        workflows: &[&str],
        depth: u32,
        script: Vec<ProcState>,
        generated: Option<&str>,
    ) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let workspace = tmp.path().join("ws");
        let run_dir = global.join("agentiflows").join("af_parity");
        for d in [&global, &workspace, &run_dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        let backend = Arc::new(ScriptedProcesses::scripted(script));
        let minted = Arc::new(AtomicUsize::new(0));
        let generation = generated.map(|text| {
            let text = text.to_string();
            let counter = minted.clone();
            Generation {
                provider: "mock".into(),
                model: "mock-1".into(),
                factory: Arc::new(move || {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                        text: text.clone(),
                        stop: rupu_agent::StopReason::EndTurn,
                        input_tokens: 1,
                        output_tokens: 1,
                    }])) as Box<dyn rupu_providers::LlmProvider>
                }),
            }
        });
        let launcher = RunLauncher::new(LaunchEnv {
            assembler: Arc::new(RunAssembler::new(AssemblyContext::minimal(global.clone()))),
            store: Arc::new(rupu_orchestrator::RunStore::new(global.join("runs"))),
            events: None,
            namer: None,
            backend: backend.clone(),
            runtime: tokio::runtime::Handle::current(),
            max_concurrent: 8,
            generation,
            project: Some(workspace.join(".rupu")),
        });
        let mut ctx = ToolContext::in_workspace(&workspace);
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        *ctx.identity_mut() = RunIdentity {
            run_id: "af_parity".into(),
            codename: Some(LEAD.into()),
            agent: "lead".into(),
            depth,
            launch: LaunchScope {
                agents: names(agents),
                workflows: names(workflows),
                pool: Some(LaunchPool {
                    agents: names(agents),
                    workflows: names(workflows),
                }),
                children_dir: Some(run_dir.clone()),
                usage_root: Some(run_dir.join("usage.jsonl")),
                flow: Some(run_dir.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        ctx.services.launcher = Some(launcher.clone());
        ctx.services.workflow_generator = Some(launcher);
        ctx.services.findings = Some(rupu_coverage::FindingWriteOptions {
            engagement: Some(Arc::new(
                rupu_coverage::builtin_registry()
                    .unwrap()
                    .active_set(&["network".into()])
                    .unwrap(),
            )),
            ..Default::default()
        });
        ctx.call.spawn_ceiling = Some(PermissionMode::Bypass);
        Self {
            _tmp: tmp,
            workspace,
            run_dir,
            backend,
            ctx,
            minted,
        }
    }

    fn workflow(&self, id: &str, yaml: &str) {
        let dir = self.workspace.join(".rupu").join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.yaml")), yaml).unwrap();
    }

    async fn call(&self, tool: &dyn Tool, called_as: &str, input: Value) -> rupu_tools::ToolOutput {
        let mut ctx = self.ctx.clone();
        ctx.call.called_as = Some(called_as.into());
        tool.invoke(input, &ctx).await.unwrap()
    }

    fn spawned_args(&self) -> Vec<RunArgv> {
        self.backend.spawned().into_iter().map(|l| l.argv).collect()
    }
}

fn addr(name: &str, n: u32) -> String {
    format!("{}#{n}", rupu_codename::role_word(name))
}

fn body(out: &rupu_tools::ToolOutput) -> Value {
    assert_eq!(out.error, None, "{out:?}");
    serde_json::from_str(&out.stdout).unwrap()
}

fn running() -> Vec<ProcState> {
    vec![ProcState::Running]
}

#[tokio::test]
async fn dispatch_refuses_an_agent_outside_the_pool_and_spawns_nothing() {
    let f = Flow::new(&["recon"], &[], 0, running(), None);
    let out = f
        .call(
            &DispatchTool,
            "dispatch",
            json!({ "agent": "not-in-pool", "prompt": "scan" }),
        )
        .await;
    let err = out.error.expect("refused");
    assert!(err.contains("not in this flow's pool"), "{err}");
    assert!(f.backend.spawned().is_empty());
}

#[tokio::test]
async fn dispatch_refuses_at_max_depth_but_allows_just_below_it() {
    let at_max = Flow::new(
        &["recon"],
        &[],
        rupu_tools::MAX_LAUNCH_DEPTH,
        running(),
        None,
    );
    let out = at_max
        .call(
            &DispatchTool,
            "dispatch",
            json!({ "agent": "recon", "prompt": "scan" }),
        )
        .await;
    assert!(out.error.unwrap().contains("depth"));
    assert!(at_max.backend.spawned().is_empty());

    let below = Flow::new(
        &["recon"],
        &[],
        rupu_tools::MAX_LAUNCH_DEPTH - 1,
        running(),
        None,
    );
    let out = below
        .call(
            &DispatchTool,
            "dispatch",
            json!({ "agent": "recon", "prompt": "scan" }),
        )
        .await;
    assert_eq!(out.error, None);
    assert_eq!(below.backend.spawned().len(), 1);
}

#[tokio::test]
async fn every_dispatch_gets_a_unique_participant_and_the_flow_engagement() {
    let f = Flow::new(&["recon", "exploit"], &[], 0, running(), None);
    let mut participants = Vec::new();
    for agent in ["recon", "recon", "exploit"] {
        let out = f
            .call(
                &DispatchTool,
                "dispatch",
                json!({ "agent": agent, "prompt": "go" }),
            )
            .await;
        participants.push(body(&out)["participant"].as_str().unwrap().to_string());
    }
    assert_eq!(
        participants,
        [addr("recon", 1), addr("recon", 2), addr("exploit", 1)]
    );
    for argv in f.spawned_args() {
        let RunArgv::Agent(run) = argv else { panic!() };
        assert_eq!(run.engagement_profiles, ["network"]);
        assert_eq!(run.mode, Some(PermissionMode::Bypass));
        assert_eq!(run.prompt.as_deref(), Some("go"));
        let flow = run.flow.unwrap();
        assert_eq!(flow.run_dir, f.run_dir);
        assert!(run.codename.unwrap().starts_with(&format!("{LEAD}>")));
    }
}

#[tokio::test]
async fn dispatch_returns_the_handle_and_join_returns_the_outcome() {
    let f = Flow::new(
        &["recon"],
        &[],
        0,
        vec![
            ProcState::Running,
            ProcState::Done(ProcOutcome {
                output: "found 3 hosts".into(),
                success: true,
            }),
        ],
        None,
    );
    let out = f
        .call(
            &DispatchTool,
            "dispatch",
            json!({ "agent": "recon", "prompt": "scan" }),
        )
        .await;
    let handle = body(&out)["handle"].as_str().unwrap().to_string();
    assert!(handle.starts_with("ch_"), "{handle}");
    let out = f
        .call(
            &JoinTool,
            "join",
            json!({ "handle": handle, "timeout_secs": 30 }),
        )
        .await;
    assert_eq!(
        body(&out),
        json!({ "status": "done", "success": true, "output": "found 3 hosts" })
    );
}

#[tokio::test]
async fn join_times_out_on_a_unit_that_never_finishes_and_reports_it_running() {
    let f = Flow::new(&["recon"], &[], 0, running(), None);
    let out = f
        .call(
            &DispatchTool,
            "dispatch",
            json!({ "agent": "recon", "prompt": "scan" }),
        )
        .await;
    let handle = body(&out)["handle"].as_str().unwrap().to_string();
    // Let the watcher see it start.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let out = f
        .call(
            &JoinTool,
            "join",
            json!({ "handle": handle, "timeout_secs": 0 }),
        )
        .await;
    assert_eq!(body(&out), json!({ "status": "running" }));
    let out = f
        .call(
            &JoinTool,
            "join",
            json!({ "handle": "ch_nope", "timeout_secs": 0 }),
        )
        .await;
    assert_eq!(body(&out)["status"], "failed");
}

#[tokio::test]
async fn malformed_arguments_are_invalid_input() {
    let f = Flow::new(&["recon"], &[], 0, running(), None);
    for (tool, called_as, input) in [
        (
            &DispatchTool as &dyn Tool,
            "dispatch",
            json!({ "agent": "recon" }),
        ),
        (&DispatchTool, "run_workflow", json!({ "inputs": {} })),
        (
            &DispatchTool,
            "run_workflow",
            json!({ "workflow": "w", "inputs": [1] }),
        ),
        (
            &JoinTool,
            "join",
            json!({ "handle": "ch_1", "timeout_secs": "soon" }),
        ),
    ] {
        let mut ctx = f.ctx.clone();
        ctx.call.called_as = Some(called_as.into());
        assert!(
            tool.invoke(input.clone(), &ctx).await.is_err(),
            "{called_as} {input}"
        );
    }
    assert!(f.backend.spawned().is_empty());
}

#[tokio::test]
async fn run_workflow_checks_run_in_the_documented_order_and_spawn_nothing() {
    let f = Flow::new(
        &["recon"],
        &["wide", "gated", "needs-input"],
        0,
        running(),
        None,
    );
    f.workflow(
        "unlisted",
        "name: unlisted\nsteps:\n  - id: s\n    agent: recon\n    prompt: hi\n",
    );
    f.workflow(
        "wide",
        "name: wide\nsteps:\n  - id: s\n    agent: rogue\n    prompt: hi\n",
    );
    f.workflow(
        "gated",
        "name: gated\nsteps:\n  - id: g\n    approval:\n      required: true\n",
    );
    f.workflow(
        "needs-input",
        "name: needs-input\ninputs:\n  target: { type: string, required: true }\nsteps:\n  - id: s\n    agent: recon\n    prompt: \"{{ inputs.target }}\"\n",
    );
    for (wf, reason) in [
        ("unlisted", "workflow 'unlisted' is not in this flow's pool"),
        ("missing", "workflow 'missing' is not in this flow's pool"),
        (
            "wide",
            "workflow 'wide' dispatches agents outside the pool: rogue",
        ),
        (
            "gated",
            "v1 cannot run a gated or host/distribute workflow as a child (gated)",
        ),
        ("needs-input", "input `target` is required"),
    ] {
        let out = f
            .call(&DispatchTool, "run_workflow", json!({ "workflow": wf }))
            .await;
        let err = out.error.unwrap_or_default();
        assert!(err.contains(reason), "{wf}: {err}");
    }
    assert!(f.backend.spawned().is_empty());
}

#[tokio::test]
async fn run_workflow_spawns_a_workflow_unit_with_the_inputs_and_engagement() {
    let f = Flow::new(&["recon"], &["sweep"], 0, running(), None);
    f.workflow(
        "sweep",
        "name: sweep\ninputs:\n  target: { type: string, required: true }\nsteps:\n  - id: s\n    agent: recon\n    prompt: \"{{ inputs.target }}\"\n",
    );
    let out = f
        .call(
            &DispatchTool,
            "run_workflow",
            json!({ "workflow": "sweep", "inputs": { "target": "10.0.0.5" } }),
        )
        .await;
    assert_eq!(body(&out)["participant"], addr("sweep", 1));
    let RunArgv::Workflow(run) = &f.spawned_args()[0] else {
        panic!()
    };
    // The vetted file, not the name for the unit to look up again.
    assert_eq!(
        run.workflow,
        WorkflowRef::File(f.workspace.join(".rupu/workflows/sweep.yaml"))
    );
    assert_eq!(
        run.inputs.get("target").map(String::as_str),
        Some("10.0.0.5")
    );
    assert_eq!(run.engagement_profiles, ["network"]);
    assert!(run.plain);
}

#[tokio::test]
async fn an_agent_and_a_workflow_of_the_same_name_get_distinct_participants() {
    let f = Flow::new(&["recon", "probe"], &["probe"], 0, running(), None);
    f.workflow(
        "probe",
        "name: probe\nsteps:\n  - id: s\n    agent: recon\n    prompt: hi\n",
    );
    let a = f
        .call(
            &DispatchTool,
            "dispatch",
            json!({ "agent": "probe", "prompt": "go" }),
        )
        .await;
    let w = f
        .call(
            &DispatchTool,
            "run_workflow",
            json!({ "workflow": "probe" }),
        )
        .await;
    assert_ne!(body(&a)["participant"], body(&w)["participant"]);
}

const GEN_WF: &str = "name: fresh-sweep\nsteps:\n  - id: only\n    agent: writer\n    prompt: hi\n";

#[tokio::test]
async fn generate_workflow_dispatches_a_file_backed_unit_with_engagement() {
    let f = Flow::new(&["writer"], &[], 0, running(), Some(GEN_WF));
    let out = f
        .call(
            &WorkflowsGenerateTool,
            "generate_workflow",
            json!({ "description": "sweep the gateway" }),
        )
        .await;
    let b = body(&out);
    assert_eq!(b["participant"], addr("fresh-sweep", 1));
    let file = PathBuf::from(b["generated_file"].as_str().unwrap());
    assert_eq!(file.parent().unwrap(), f.run_dir.join("generated"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap().trim(),
        GEN_WF.trim()
    );
    let RunArgv::Workflow(run) = &f.spawned_args()[0] else {
        panic!()
    };
    assert_eq!(run.workflow, WorkflowRef::File(file));
    assert_eq!(run.engagement_profiles, ["network"]);
    assert_eq!(f.minted.load(Ordering::Relaxed), 1);
}

async fn assert_gen_refused(f: &Flow, needle: &str) {
    let out = f
        .call(
            &WorkflowsGenerateTool,
            "generate_workflow",
            json!({ "description": "sweep" }),
        )
        .await;
    let err = out.error.expect("refused");
    assert!(err.contains(needle), "{err}");
    assert!(f.backend.spawned().is_empty());
    assert!(!f.run_dir.join("generated").exists(), "no file left behind");
}

#[tokio::test]
async fn generate_workflow_refuses_out_of_pool_agents_and_gates() {
    let rogue = "name: rogue\nsteps:\n  - id: only\n    agent: rogue\n    prompt: hi\n";
    assert_gen_refused(
        &Flow::new(&["writer"], &[], 0, running(), Some(rogue)),
        "not in this flow's pool",
    )
    .await;
    let gated = "name: g\nsteps:\n  - id: g\n    approval:\n      required: true\n";
    assert_gen_refused(
        &Flow::new(&["writer"], &[], 0, running(), Some(gated)),
        "approval gate",
    )
    .await;
}

#[tokio::test]
async fn generate_workflow_refuses_at_max_depth_before_calling_the_provider() {
    let f = Flow::new(
        &["writer"],
        &[],
        rupu_tools::MAX_LAUNCH_DEPTH,
        running(),
        Some(GEN_WF),
    );
    assert_gen_refused(&f, "depth").await;
    assert_eq!(f.minted.load(Ordering::Relaxed), 0);
}
