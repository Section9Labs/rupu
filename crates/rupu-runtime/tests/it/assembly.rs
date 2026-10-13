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

pub(crate) fn agent(home: &Home, frontmatter: &str) -> rupu_agent::AgentSpec {
    home.agent("probe", frontmatter);
    rupu_agent::load_agent(&home.global(), None, "probe").unwrap()
}

pub(crate) fn launch(
    home: &Home,
    agent: rupu_agent::AgentSpec,
    origin: Origin,
    run_id: &str,
) -> LaunchSpec {
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
        link: None,
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
                messaging: None,
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
    launcher: bool,
    /// What the run may launch, and where its children are recorded.
    launch_agents: Vec<String>,
    children_dir: Option<String>,
    catalog: bool,
    customer: Option<String>,
    output_tokens: Option<u32>,
    context_window: Option<String>,
    recovery_chain: usize,
    hop_builder: bool,
    max_turns: u32,
    admission: bool,
    grant_offers_dispatch: bool,
    grant_offers_issues_create: bool,
    grant_offers_agents_list: bool,
    /// The origin's own grant (W5 §3.4): its reason and tools.
    origin_grant: Option<String>,
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
        launcher: s.launcher.is_some(),
        launch_agents: id.launch.agents.clone(),
        children_dir: id
            .launch
            .children_dir
            .as_deref()
            .map(|p| rel(home.root(), p)),
        catalog: s.catalog.is_some(),
        customer: s.customer.clone(),
        output_tokens: o.limits.output.tokens,
        context_window: o.pins.context_window.map(|c| format!("{c:?}")),
        recovery_chain: o.recovery.chain.len(),
        hop_builder: o.recovery.hop_builder.is_some(),
        max_turns: o.max_turns,
        admission: defaults_for(origin).admission,
        grant_offers_dispatch: o.grant.offers("dispatch"),
        grant_offers_issues_create: o.grant.offers("issues.create"),
        grant_offers_agents_list: o.grant.offers("agents.list"),
        origin_grant: defaults_for(origin)
            .origin_grant
            .map(|g| format!("{}: {}", g.reason, g.tools.join(" "))),
        narrowed: o.grant.narrowed.iter().map(|n| n.to_string()).collect(),
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
        l.services.launcher = Some(Arc::new(crate::support::NoLaunch));
        if matches!(origin, Origin::SubAgent { .. }) {
            l.ceiling = Some(PermissionMode::Ask);
        }
        let _m = MockScript::set(DONE);
        let run = assembler.assemble(l).await.expect("assembles");
        rows.insert(name, derived(&home, &origin, &run));
    }
    insta::assert_yaml_snapshot!(rows);
}

/// W5 §3.3a / D22: a `tools: ["*"]` agent resolved under every origin, with
/// the services that origin's owner provides (a lead: its bus and status; a
/// fleet unit: its bus; every site: the catalog). `*` grants the roster tools
/// wherever the catalog is provided, and never the authority tools — the
/// lead holds `board.directive` / `board.retract` only through its origin
/// grant.
#[tokio::test]
#[serial]
async fn star_parity_per_origin() {
    let home = Home::new();
    let assembler = RunAssembler::new(home.context(config()));
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [\"*\"]",
    );
    let flow_dir = home.global().join("agentiflows").join("af_1");
    let mut rows = std::collections::BTreeMap::new();
    for (name, origin) in origins(&home) {
        let mut l = launch(&home, spec.clone(), origin.clone(), &format!("run_{name}"));
        l.services.launcher = Some(Arc::new(crate::support::NoLaunch));
        match &origin {
            Origin::FlowLead { .. } => {
                l.services.message_bus = Some(Arc::new(rupu_fleet::Bus::open(&flow_dir, "lead")));
                l.services.run_status = Some(Arc::new(crate::support::NoStatus));
            }
            Origin::FlowUnit { participant, .. } => {
                l.services.message_bus = Some(Arc::new(rupu_fleet::Bus::open(
                    &flow_dir,
                    participant.clone(),
                )));
            }
            _ => {}
        }
        let _m = MockScript::set(DONE);
        let run = assembler.assemble(l).await.expect("assembles");
        let grant = &run.opts.grant;
        for authority in ["board.directive", "board.retract"] {
            if let Some(e) = grant.entry(authority) {
                assert!(
                    matches!(origin, Origin::FlowLead { .. }),
                    "{name}: `*` must never grant {authority}"
                );
                assert_eq!(e.reasons_string(), "origin:flow_lead", "{name}");
            }
        }
        for roster in [
            "agents.list",
            "agents.get",
            "workflows.list",
            "workflows.get",
            "catalog.search",
        ] {
            assert!(
                grant.offers(roster),
                "{name}: `*` with a catalog grants {roster}"
            );
        }
        let entries: std::collections::BTreeMap<String, String> = grant
            .entries
            .values()
            .map(|e| (e.canonical.to_string(), e.reasons_string()))
            .collect();
        rows.insert(name, entries);
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
/// is offered `dispatch` when it has a launcher, and
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
    l.services.launcher = Some(Arc::new(crate::support::NoLaunch));
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    let id = run.opts.identity();
    assert_eq!(
        (id.provider.as_str(), id.model.as_str()),
        ("oracle", "oracle-default")
    );
    assert!(run.opts.grant.offers("dispatch"));
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

/// `[coverage].full_mode_max_concerns` is the threshold a run's concerns
/// render with; unset, the default applies.
#[tokio::test]
#[serial]
async fn coverage_threshold_comes_from_config() {
    let home = Home::new();
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nconcerns:\n  - include: stride",
    );
    for (set, want) in [
        (Some(3), 3),
        (None, rupu_coverage::DEFAULT_FULL_MODE_THRESHOLD),
    ] {
        let mut cfg = config();
        cfg.coverage.full_mode_max_concerns = set;
        let l = launch(&home, spec.clone(), Origin::Standalone, "run_cov");
        let _m = MockScript::set(DONE);
        let run = home.assembler(cfg).assemble(l).await.expect("assembles");
        let concerns = run.opts.concerns.as_ref().expect("the agent's concerns");
        assert_eq!(concerns.full_mode_max_concerns, want, "{set:?}");
    }
}

/// W7 D18: a process child's `--launch-*` link sets what an in-process parent
/// would — its depth, parent and root, the ceiling over its own
/// `permissionMode`, the root's ledger it rolls up to, and the caps on what it may
/// launch and which connector tools it may call.
#[tokio::test]
#[serial]
async fn a_launch_link_carries_depth_ceiling_caps_and_the_usage_root() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\npermissionMode: bypass\n\
         tools: [\"*\"]\ndispatchableAgents: [inner, outsider]",
    );
    let root_ledger = home.runs().join("run_root").join("usage.jsonl");
    let mut l = launch(&home, spec, Origin::Standalone, "run_child");
    l.codename = Some("jade-reef/heron#1>lynx#1".into());
    l.services.launcher = Some(Arc::new(crate::support::NoLaunch));
    l.link = Some(rupu_runtime::argv::LaunchLink {
        depth: 2,
        ceiling: PermissionMode::Readonly,
        parent_run: "run_parent".into(),
        root_run: "run_root".into(),
        usage_root: Some(root_ledger.clone()),
        agents: vec!["inner".into()],
        workflows: vec![],
        actions: Some(vec!["issues.get".into()]),
    });
    let _m = MockScript::set(DONE);
    let run = assembler.assemble(l).await.unwrap();
    let id = run.opts.identity();
    assert_eq!(id.depth, 2);
    let parent = id.parent.as_ref().unwrap();
    assert_eq!(
        (parent.run_id.as_str(), parent.root_run_id.as_str()),
        ("run_parent", "run_root")
    );
    assert_eq!(parent.codename.as_deref(), Some("jade-reef/heron#1"));
    assert_eq!(run.opts.permission.mode(), PermissionMode::Readonly);
    let own = home.runs().join("run_child").join("usage.jsonl");
    assert_eq!(
        run.usage_ledger.as_deref(),
        Some(own.as_path()),
        "its own ledger"
    );
    assert_eq!(id.launch.usage_root.as_deref(), Some(own.as_path()));
    assert_eq!(
        id.launch.rollup_root.as_deref(),
        Some(root_ledger.as_path()),
        "copies roll up to the root"
    );
    assert_eq!(
        id.launch.agents,
        vec!["inner".to_string()],
        "outsider is capped away"
    );
    assert!(run.opts.grant.offers("issues.get"));
    assert!(
        !run.opts.grant.offers("issues.create"),
        "the parent's connector cap"
    );
    assert_eq!(
        id.launch.connector_cap,
        Some(vec!["issues.get".to_string()]),
        "passed on to its own children"
    );
}

/// W7 §3.4: inside an agentiflow the pool is the allowlist (intersected with
/// the agent's own when it declares one), and an in-process child inherits
/// the pool and its parent's set as a cap.
#[tokio::test]
#[serial]
async fn a_flow_pool_is_the_allowlist_and_children_inherit_it() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let flow_dir = home.global().join("agentiflows").join("af_1");
    let lead_spec = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6");
    let mut l = launch(
        &home,
        lead_spec,
        Origin::FlowLead {
            flow_id: "af_1".into(),
            flow_dir: flow_dir.clone(),
        },
        "af_1",
    );
    l.services.launcher = Some(Arc::new(crate::support::NoLaunch));
    l.services.flow_pool = Some(rupu_tools::LaunchPool {
        agents: vec!["recon".into(), "exploit".into()],
        workflows: vec!["web-assess".into()],
    });
    let _m = MockScript::set(DONE);
    let lead = assembler.assemble(l).await.unwrap();
    let lead_id = lead.opts.identity();
    assert_eq!(lead_id.launch.agents, vec!["recon", "exploit"]);
    assert_eq!(lead_id.launch.workflows, vec!["web-assess"]);
    assert_eq!(
        lead_id.launch.children_dir.as_deref(),
        Some(flow_dir.as_path())
    );
    assert_eq!(lead_id.launch.flow.as_deref(), Some(flow_dir.as_path()));

    // A sub-agent of the lead that declares its own list: the pool ∩ it.
    let child_spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\ndispatchableAgents: [exploit, outsider]",
    );
    let mut c = launch(
        &home,
        child_spec,
        Origin::SubAgent {
            parent: ParentRun {
                identity: Arc::new(lead_id.clone()),
                usage: None,
                coverage_stream: None,
            },
        },
        "sub_1",
    );
    c.ceiling = Some(PermissionMode::Bypass);
    let child = assembler.assemble(c).await.unwrap();
    let child_id = child.opts.identity();
    assert_eq!(child_id.depth, 1);
    assert_eq!(child_id.launch.agents, vec!["exploit"]);
    assert_eq!(child_id.launch.flow.as_deref(), Some(flow_dir.as_path()));
}

/// Every run with a space joins it as it starts, so the operator can reach
/// it (F1b); it is offered the messaging tools only when it takes part
/// (spec F1a FD1): its agent names one. The log waits for a first message.
#[tokio::test]
#[serial]
async fn every_run_joins_its_space_and_takes_part_only_when_asked() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let tools = |run: &rupu_runtime::assembly::AssembledRun| {
        let s = &run.opts.tool_context.services;
        (
            s.message_space.is_some(),
            s.message_bus.is_some(),
            run.opts.grant.offers("msg.send"),
        )
    };
    let joined = |run: &rupu_runtime::assembly::AssembledRun, run_id: &str| {
        let bus = run
            .opts
            .tool_context
            .services
            .message_space
            .clone()
            .unwrap();
        assert!(bus.is_joined(), "{run_id}");
        let roster = bus.roster().unwrap();
        let me = roster
            .participants()
            .find(|p| p.member.run_id == run_id)
            .unwrap_or_else(|| panic!("{run_id} joined"));
        assert!(me.live);
        assert_eq!(me.member.pid, Some(std::process::id()));
        assert!(!bus.log_path().exists(), "{run_id}: no message yet, no log");
    };

    let quiet = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: ['*']");
    let _m = MockScript::set(DONE);
    let run = assembler
        .assemble(launch(&home, quiet, Origin::Standalone, "run_quiet"))
        .await
        .unwrap();
    assert_eq!(tools(&run), (true, false, false), "`*` alone never takes part");
    joined(&run, "run_quiet");

    let named = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [msg.send]");
    let run = assembler
        .assemble(launch(&home, named, Origin::Standalone, "run_named"))
        .await
        .unwrap();
    assert_eq!(tools(&run), (true, true, true));
    joined(&run, "run_named");
}
