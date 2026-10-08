//! The run assembler (spec 2026-10-07 W3): the per-origin defaults matrix,
//! and one regression test per bug it fixes for the launch sites W3a
//! migrated (`rupu run`, sub-agents, session turns).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rupu_runtime::assembly::{
    defaults_for, AssembledRun, LaunchServices, LaunchSpec, Origin, Overrides, ParentRun,
    RunAssembler, UnitKey,
};
use rupu_runtime::usage_ledger::{LedgerRow, UsageLedger};
use rupu_tools::{PermissionMode, RunIdentity, Surface};
use serde::Serialize;
use serial_test::serial;

use crate::support::{config, Home, MockScript, DONE};

fn agent(home: &Home, frontmatter: &str) -> rupu_agent::AgentSpec {
    home.agent("probe", frontmatter);
    rupu_agent::load_agent(&home.global(), None, "probe").unwrap()
}

fn launch(home: &Home, agent: rupu_agent::AgentSpec, origin: Origin, run_id: &str) -> LaunchSpec {
    LaunchSpec {
        agent,
        origin,
        run_id: run_id.into(),
        transcript_path: home.root().join(format!("{run_id}.jsonl")),
        codename: Some("jade-reef/heron".into()),
        prompt: rupu_agent::UserTurn::new("go"),
        workspace: home.binding(),
        mode: PermissionMode::Bypass,
        ceiling: None,
        prompter: None,
        overrides: Overrides::default(),
        stream: Default::default(),
        hooks: Default::default(),
        pause: None,
        services: LaunchServices::default(),
        collectors: Vec::new(),
    }
}

/// The parent a `SubAgent` origin hangs off: a workflow step.
fn step_parent(home: &Home) -> ParentRun {
    ParentRun {
        identity: Arc::new(RunIdentity {
            run_id: "run_step".into(),
            codename: Some("jade-reef/heron".into()),
            agent: "reviewer".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            depth: 0,
            surface: Surface::Workflow,
            scope_name: Some("security-review".into()),
            step_id: Some("assess".into()),
            ..Default::default()
        }),
        usage: Some(UsageLedger::open(
            home.runs().join("run_wf").join("usage.jsonl"),
        )),
        coverage_stream: Some(home.runs().join("run_root").join("coverage.jsonl")),
    }
}

fn origins(home: &Home) -> Vec<(&'static str, Origin)> {
    vec![
        ("standalone", Origin::Standalone),
        (
            "workflow_step",
            Origin::WorkflowStep {
                workflow_run_id: "run_wf".into(),
                workflow_name: "security-review".into(),
                step_id: "assess".into(),
                unit: Some(UnitKey {
                    index: 2,
                    key: Some("gw".into()),
                }),
                step_actions: vec!["issues.*".into()],
                scope_override: None,
            },
        ),
        (
            "sub_agent",
            Origin::SubAgent {
                parent: step_parent(home),
            },
        ),
        (
            "session_turn",
            Origin::SessionTurn {
                session_id: "ses_1".into(),
                session_dir: home.global().join("sessions").join("ses_1"),
            },
        ),
        (
            "flow_lead",
            Origin::FlowLead {
                flow_id: "af_1".into(),
                flow_dir: home.global().join("agentiflows").join("af_1"),
            },
        ),
        (
            "flow_unit",
            Origin::FlowUnit {
                flow_id: "af_1".into(),
                flow_dir: home.global().join("agentiflows").join("af_1"),
                participant: "probe#1".into(),
            },
        ),
    ]
}

/// What the old launch sites disagreed on, as one origin's run derives it.
#[derive(Serialize)]
struct Derived {
    surface: &'static str,
    scope: String,
    depth: u32,
    parent: Option<String>,
    root: Option<String>,
    step_id: Option<String>,
    codename: Option<String>,
    provider: String,
    model: String,
    alias_scope: String,
    permission: &'static str,
    bash_timeout_secs: u64,
    bash_env_allowlist: Vec<String>,
    findings_profile: &'static str,
    findings_artifact_root: Option<String>,
    usage_ledger: Option<String>,
    coverage_stream: Option<String>,
    netflow: bool,
    net_capture: bool,
    scm: bool,
    dispatcher: bool,
    customer: Option<String>,
    output_tokens: Option<u32>,
    context_window: Option<String>,
    recovery_chain: usize,
    hop_builder: bool,
    max_turns: u32,
    admission: bool,
    grant_offers_dispatch_agent: bool,
    grant_offers_issues_create: bool,
    narrowed: Vec<String>,
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|r| format!("<home>/{}", r.display()))
        .unwrap_or_else(|_| p.display().to_string())
}

fn derived(home: &Home, origin: &Origin, run: &AssembledRun) -> Derived {
    let o = &run.opts;
    let id = o.identity();
    let s = &o.tool_context.services;
    let f = s.findings.as_ref().unwrap();
    Derived {
        surface: id.surface.as_str(),
        scope: id.scope().into(),
        depth: id.depth,
        parent: id.parent.as_ref().map(|p| p.run_id.clone()),
        root: id.parent.as_ref().map(|p| p.root_run_id.clone()),
        step_id: id.step_id.clone(),
        codename: id.codename.clone(),
        provider: id.provider.clone(),
        model: id.model.clone(),
        alias_scope: format!("{:?}", o.alias_scope),
        permission: o.permission.mode().as_str(),
        bash_timeout_secs: o.tool_context.workspace.bash.timeout_secs,
        bash_env_allowlist: o.tool_context.workspace.bash.env_allowlist.clone(),
        findings_profile: f.profile.as_str(),
        findings_artifact_root: f.artifact_root.as_deref().map(|p| rel(home.root(), p)),
        usage_ledger: run.usage_ledger.as_deref().map(|p| rel(home.root(), p)),
        coverage_stream: s.coverage_stream.as_deref().map(|p| rel(home.root(), p)),
        netflow: s.netflow_sink.is_some(),
        net_capture: s.net_capture.is_some(),
        scm: s.scm.is_some(),
        dispatcher: s.dispatcher.is_some(),
        customer: s.customer.clone(),
        output_tokens: o.limits.output.tokens,
        context_window: o.pins.context_window.map(|c| format!("{c:?}")),
        recovery_chain: o.recovery.chain.len(),
        hop_builder: o.recovery.hop_builder.is_some(),
        max_turns: o.max_turns,
        admission: defaults_for(origin).admission,
        grant_offers_dispatch_agent: o.grant.offers("dispatch_agent"),
        grant_offers_issues_create: o.grant.offers("issues.create"),
        narrowed: o.grant.narrowed.iter().map(|n| n.to_string()).collect(),
    }
}

/// A dispatcher that never runs: the matrix only checks it is offered.
#[derive(Debug)]
struct NoDispatch;

#[async_trait::async_trait]
impl rupu_tools::AgentDispatcher for NoDispatch {
    async fn dispatch(
        &self,
        _agent: &str,
        _prompt: String,
        _parent: &RunIdentity,
        _permission: rupu_tools::SpawnPermission,
    ) -> Result<rupu_tools::DispatchOutcome, rupu_tools::DispatchError> {
        unreachable!("the matrix never dispatches")
    }
}

/// Spec §6 test 1: every origin assembled against one fixture config; the
/// derived fields of §3.3 are snapshotted, so a change to any per-origin
/// default is a snapshot diff.
#[tokio::test]
#[serial]
async fn origin_matrix() {
    let home = Home::new();
    let mut cfg = config();
    cfg.bash = rupu_config::BashConfig {
        timeout_secs: Some(45),
        env_allowlist: Some(vec!["AWS_PROFILE".into()]),
    };
    cfg.recovery.fallbacks = vec![rupu_config::FallbackEntry {
        provider: None,
        model: "claude-haiku-4-5".into(),
    }];
    let mut ctx = home.context(cfg);
    ctx.customer = Some("acme".into());
    ctx.findings = rupu_coverage::FindingWriteOptions {
        artifact_root: Some(home.global().join("findings").join("artifacts")),
        ..Default::default()
    };
    let assembler = RunAssembler::new(ctx);
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 9\nmaxTokens: 1234\n\
         contextWindow: 1m\ntools: [\"*\"]\ndispatchableAgents: [helper]",
    );
    let mut rows = std::collections::BTreeMap::new();
    for (name, origin) in origins(&home) {
        let mut l = launch(&home, spec.clone(), origin.clone(), &format!("run_{name}"));
        l.services.dispatcher = Some(Arc::new(NoDispatch));
        if matches!(origin, Origin::SubAgent { .. }) {
            l.ceiling = Some(PermissionMode::Ask);
        }
        let _m = MockScript::set(DONE);
        let run = assembler.assemble(l).await.expect("assembles");
        rows.insert(name, derived(&home, &origin, &run));
    }
    insta::assert_yaml_snapshot!(rows);
}

/// R9: every origin goes through admission (the lend that keeps nested
/// in-process children deadlock-free is unit-tested in `admission`).
#[test]
fn admission_for_every_origin() {
    let home = Home::new();
    for (name, origin) in origins(&home) {
        assert!(defaults_for(&origin).admission, "{name} skips admission");
    }
}

/// R7: a session turn reaches a config-declared openai-compatible provider,
/// is offered sub-agent dispatch when it declares `dispatchableAgents`, and
/// runs under the session's engagement.
#[tokio::test]
#[serial]
async fn session_openai_compat_and_dispatch() {
    let home = Home::new();
    let mut cfg = config();
    cfg.providers.insert(
        "oracle".into(),
        rupu_config::ProviderConfig {
            kind: Some("openai-compatible".into()),
            base_url: Some("https://example.invalid/v1".into()),
            default_model: Some("oracle-default".into()),
            ..Default::default()
        },
    );
    let mut ctx = home.context(cfg);
    ctx.findings.engagement = Some(Arc::new(
        rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&["network".to_string()])
            .unwrap(),
    ));
    let assembler = RunAssembler::new(ctx);
    let spec = agent(&home, "provider: oracle\ndispatchableAgents: [helper]");
    let session_dir = home.global().join("sessions").join("ses_1");
    let mut l = launch(
        &home,
        spec,
        Origin::SessionTurn {
            session_id: "ses_1".into(),
            session_dir,
        },
        "run_turn",
    );
    l.services.dispatcher = Some(Arc::new(NoDispatch));
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    let id = run.opts.identity();
    assert_eq!(
        (id.provider.as_str(), id.model.as_str()),
        ("oracle", "oracle-default")
    );
    assert!(run.opts.grant.offers("dispatch_agent"));
    assert!(
        run.opts.grant.offers("assets.mark"),
        "the engagement's ambient grant"
    );
    assert_eq!(id.surface, Surface::Session);
    assert_eq!(id.scope(), "ses_1");
}

/// R13: a session turn records its usage in the session's ledger.
#[tokio::test]
#[serial]
async fn session_usage_recorded() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let spec = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6");
    let session_dir = home.global().join("sessions").join("ses_1");
    let l = launch(
        &home,
        spec,
        Origin::SessionTurn {
            session_id: "ses_1".into(),
            session_dir: session_dir.clone(),
        },
        "run_turn1",
    );
    let _m = MockScript::set(DONE);
    let exit = rupu_runtime::run_agent(&assembler, l).await.unwrap();
    exit.result.expect("the turn runs");
    let rows: Vec<LedgerRow> = std::fs::read_to_string(session_dir.join("usage.jsonl"))
        .expect("the session's ledger")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].agent_run_id, "run_turn1");
    assert_eq!(rows[0].agent, "probe");
}

/// R13: a standalone run records its usage in its own run directory, where
/// the CP reads it once the run's record exists.
#[tokio::test]
#[serial]
async fn standalone_usage_recorded() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let spec = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6");
    let l = launch(&home, spec, Origin::Standalone, "run_solo");
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, l)
        .await
        .unwrap()
        .result
        .unwrap();
    let ledger: PathBuf = home.runs().join("run_solo").join("usage.jsonl");
    let body = std::fs::read_to_string(&ledger).expect("the run's own ledger");
    assert_eq!(body.lines().count(), 1);
}

/// A run whose grant fails is refused before its loop starts — but its
/// (empty) transcript exists, as when the loop resolved the grant, so a
/// parent, a session or the CP that opens it finds the run. Sites record it
/// as the run's failure (`AssembleError::into_run_error`).
#[tokio::test]
#[serial]
async fn a_failed_grant_leaves_the_transcript_and_is_a_run_error() {
    let home = Home::new();
    // Built outside the loader, which would refuse the typo at load.
    let spec = rupu_agent::AgentSpec::parse(
        "---\nname: typo\nprovider: anthropic\nmodel: claude-sonnet-4-6\ntools: [repot_finding]\n---\nhi\n",
    )
    .unwrap();
    let l = launch(&home, spec, Origin::Standalone, "run_typo");
    let transcript = l.transcript_path.clone();
    let _m = MockScript::set(DONE);
    let err = match home.assembler(config()).assemble(l).await {
        Ok(_) => panic!("a typo in tools: must not assemble"),
        Err(e) => e,
    };
    assert!(transcript.exists(), "the failed run's transcript exists");
    assert!(matches!(
        err.into_run_error(),
        rupu_agent::RunError::ToolGrant(m) if m.contains("repot_finding")
    ));
}
