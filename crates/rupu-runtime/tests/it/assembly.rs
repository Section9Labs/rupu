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
use rupu_tools::{MessagingSwitch, PermissionMode, RunIdentity, Surface};
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
        netflow_stream: None,
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
                messaging: MessagingSwitch::Default,
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
                messaging_off: false,
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

/// What a run's owner hands it: a lead its bus and status, a fleet unit its
/// bus, a sub-agent a sibling bus on its parent's space (the launcher's job;
/// a run of these origins has no space of its own).
fn hand_bus(home: &Home, l: &mut LaunchSpec, origin: &Origin) {
    let flow_dir = home.global().join("agentiflows").join("af_1");
    match origin {
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
        Origin::SubAgent { .. } => {
            l.services.message_bus = Some(Arc::new(rupu_fleet::Bus::open(
                &home.runs().join("run_wf").join("messages"),
                "heron>otter#1",
            )));
        }
        _ => {}
    }
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
    /// The root's netflow stream this run tees into, with the ledger id it
    /// tags its lines with (spec 2026-10-14 remote netflow transport §N1).
    netflow_stream: Option<String>,
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
    /// Why `msg.send` is offered (spec F1d D1); `None` = messaging off.
    messaging: Option<String>,
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
        netflow_stream: s
            .netflow_stream
            .as_ref()
            .map(|st| format!("{} as {}", rel(home.root(), &st.path), st.ledger_id)),
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
        messaging: o.grant.entry("msg.send").map(|e| e.reasons_string()),
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
        hand_bus(&home, &mut l, &origin);
        if matches!(origin, Origin::SubAgent { .. }) {
            l.ceiling = Some(PermissionMode::Ask);
        }
        let _m = MockScript::set(DONE);
        let run = assembler.assemble(l).await.expect("assembles");
        rows.insert(name, derived(&home, &origin, &run));
    }
    insta::assert_yaml_snapshot!(rows);
}

/// Spec 2026-10-14 remote netflow transport §N1: a root that a coordinator
/// launched begins `<runs>/<root>/netflow.jsonl`; a step of that run and a
/// sub-agent of that step tee into it under their own ledger ids, while a
/// run whose root never began a stream tees nowhere.
#[tokio::test]
#[serial]
async fn a_begun_root_stream_is_teed_by_steps_and_sub_agents() {
    let home = Home::new();
    let assembler = RunAssembler::new(home.context(config()));
    let spec = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6");
    let root_stream = home.runs().join("run_wf").join("netflow.jsonl");
    rupu_netflow::stream::write_stream_begin(&root_stream, "run_wf").unwrap();

    let step_origin = Origin::WorkflowStep {
        workflow_run_id: "run_wf".into(),
        workflow_name: "security-review".into(),
        step_id: "assess".into(),
        unit: None,
        step_actions: Vec::new(),
        scope_override: None,
        messaging: MessagingSwitch::Default,
    };
    let _m = MockScript::set(DONE);
    let step = assembler
        .assemble(launch(&home, spec.clone(), step_origin, "run_step"))
        .await
        .expect("assembles");
    let step_stream = step
        .opts
        .tool_context
        .services
        .netflow_stream
        .clone()
        .expect("a step of a begun root tees");
    assert_eq!(step_stream.path, root_stream);
    assert_eq!(step_stream.ledger_id, "run_step");

    // A sub-agent of that step: the same file, its own id.
    let mut child = launch(
        &home,
        spec.clone(),
        Origin::SubAgent {
            parent: ParentRun {
                identity: Arc::clone(&step.opts.tool_context.identity),
                usage: None,
                coverage_stream: None,
                netflow_stream: Some(step_stream.clone()),
            },
        },
        "sub_1",
    );
    child.ceiling = Some(PermissionMode::Bypass);
    let child = assembler.assemble(child).await.expect("assembles");
    let child_stream = child
        .opts
        .tool_context
        .services
        .netflow_stream
        .clone()
        .expect("a sub-agent tees where its parent does");
    assert_eq!(child_stream.path, root_stream);
    assert_eq!(child_stream.ledger_id, "sub_1");

    // A standalone run whose own directory holds no begun stream: none.
    let solo = assembler
        .assemble(launch(&home, spec.clone(), Origin::Standalone, "run_solo"))
        .await
        .expect("assembles");
    assert_eq!(solo.opts.tool_context.services.netflow_stream, None);

    // A step of a workflow run that never began one: none either.
    let other = assembler
        .assemble(launch(
            &home,
            spec,
            Origin::WorkflowStep {
                workflow_run_id: "run_other".into(),
                workflow_name: "security-review".into(),
                step_id: "assess".into(),
                unit: None,
                step_actions: Vec::new(),
                scope_override: None,
                messaging: MessagingSwitch::Default,
            },
            "run_other_step",
        ))
        .await
        .expect("assembles");
    assert_eq!(other.opts.tool_context.services.netflow_stream, None);
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
    let mut rows = std::collections::BTreeMap::new();
    for (name, origin) in origins(&home) {
        let mut l = launch(&home, spec.clone(), origin.clone(), &format!("run_{name}"));
        l.services.launcher = Some(Arc::new(crate::support::NoLaunch));
        hand_bus(&home, &mut l, &origin);
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
        no_messaging: false,
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
            messaging_off: false,
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
                netflow_stream: None,
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

/// The run's space, its bus behind the tools, and whether `msg.send` is
/// offered.
fn messaging_of(run: &AssembledRun) -> (bool, bool, bool) {
    let s = &run.opts.tool_context.services;
    (
        s.message_space.is_some(),
        s.message_bus.is_some(),
        run.opts.grant.offers("msg.send"),
    )
}

/// Every run with a space joins it as it starts, so the operator can reach
/// it (F1b), and is offered the messaging tools by default (spec F1d D1):
/// `*` alone, or no `tools:` at all, gets them as `ambient:messaging_default`.
/// An agent's `messaging: false` leaves it in the space receive-only (D4).
/// The log waits for a first message.
#[tokio::test]
#[serial]
async fn every_run_joins_its_space_and_takes_part_unless_opted_out() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let tools = messaging_of;
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

    let star = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: ['*']");
    let _m = MockScript::set(DONE);
    let run = assembler
        .assemble(launch(&home, star, Origin::Standalone, "run_star"))
        .await
        .unwrap();
    assert_eq!(tools(&run), (true, true, true), "`*` takes part by default");
    assert_eq!(
        run.opts.grant.entry("msg.send").unwrap().reasons_string(),
        "declared:*,ambient:messaging_default"
    );
    joined(&run, "run_star");

    let default = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6");
    let run = assembler
        .assemble(launch(&home, default, Origin::Standalone, "run_default"))
        .await
        .unwrap();
    assert_eq!(tools(&run), (true, true, true), "no `tools:` takes part too");
    for tool in rupu_tools::WORKFLOW_MESSAGING {
        assert_eq!(
            run.opts.grant.entry(tool).unwrap().reasons_string(),
            "ambient:messaging_default",
            "{tool}"
        );
    }
    assert!(!run.opts.grant.offers("board.directive"), "never the authority tools");
    joined(&run, "run_default");

    let quiet = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmessaging: false\ntools: ['*']",
    );
    let run = assembler
        .assemble(launch(&home, quiet, Origin::Standalone, "run_quiet"))
        .await
        .unwrap();
    assert_eq!(tools(&run), (true, false, false), "`messaging: false` is receive-only");
    assert!(run.opts.grant.unavailable.is_empty(), "nothing named, no notice");
    assert!(run.opts.identity().launch.messaging_off);
    joined(&run, "run_quiet");
}

/// Spec F1d D4, D5: an agent's `messaging: false` wins under every origin,
/// a handed bus and an origin grant included. The lead keeps the rest of its
/// coordination set.
#[tokio::test]
#[serial]
async fn agent_messaging_off_wins_under_every_origin() {
    let home = Home::new();
    let assembler = RunAssembler::new(home.context(config()));
    let spec = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\nmessaging: false\ntools: [\"*\"]",
    );
    for (name, origin) in origins(&home) {
        let mut l = launch(&home, spec.clone(), origin.clone(), &format!("run_{name}"));
        l.services.launcher = Some(Arc::new(crate::support::NoLaunch));
        hand_bus(&home, &mut l, &origin);
        let _m = MockScript::set(DONE);
        let run = assembler.assemble(l).await.expect("assembles");
        assert_eq!(messaging_of(&run), (true, false, false), "{name}");
        let offered: Vec<&str> = run
            .opts
            .grant
            .entries
            .keys()
            .copied()
            .filter(|t| t.starts_with("board.") || t.starts_with("msg."))
            .collect();
        assert!(offered.is_empty(), "{name}: {offered:?}");
        assert!(run.opts.identity().launch.messaging_off, "{name}");
        if matches!(origin, Origin::FlowLead { .. }) {
            assert!(run.opts.grant.offers("goal.status"), "the lead keeps its status tools");
        }
    }
}

/// Spec F1d D2, D5, D6: a workflow's `messaging: false` reaches its steps
/// (a tool the agent names is reported unavailable, never silently dropped)
/// and the sub-agents they launch, which never have more than their parent.
/// `{ tools: [...] }` grants only those, as `ambient:workflow_messaging`.
#[tokio::test]
#[serial]
async fn workflow_messaging_off_reaches_the_steps_children() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let step_origin = |messaging| Origin::WorkflowStep {
        workflow_run_id: "run_wf".into(),
        workflow_name: "review".into(),
        step_id: "assess".into(),
        unit: None,
        step_actions: Vec::new(),
        scope_override: None,
        messaging,
    };
    let naming = agent(
        &home,
        "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [read_file, msg.send]",
    );
    let _m = MockScript::set(DONE);
    let step = assembler
        .assemble(launch(
            &home,
            naming.clone(),
            step_origin(MessagingSwitch::Off),
            "run_step_off",
        ))
        .await
        .unwrap();
    assert_eq!(messaging_of(&step), (true, false, false));
    assert_eq!(
        step.opts
            .grant
            .unavailable
            .iter()
            .map(|u| (u.tool, u.missing.clone()))
            .collect::<Vec<_>>(),
        vec![("msg.send", vec![rupu_tools::Service::MessageBus])]
    );
    assert!(step.opts.identity().launch.messaging_off);

    let star = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: ['*']");
    let mut c = launch(
        &home,
        star.clone(),
        Origin::SubAgent {
            parent: ParentRun {
                identity: Arc::clone(&step.opts.tool_context.identity),
                usage: None,
                coverage_stream: None,
                netflow_stream: None,
            },
        },
        "run_step_off_child",
    );
    let parent_bus = step.opts.tool_context.services.message_space.clone().unwrap();
    c.services.message_bus = Some(Arc::new(parent_bus.sibling("assess>otter#1")));
    let child = assembler.assemble(c).await.unwrap();
    assert_eq!(messaging_of(&child), (true, false, false), "the cap reaches a sub-agent");
    assert!(child.opts.identity().launch.messaging_off, "and its children's cap");

    let on = assembler
        .assemble(launch(
            &home,
            star,
            step_origin(MessagingSwitch::Default),
            "run_step_on",
        ))
        .await
        .unwrap();
    assert_eq!(messaging_of(&on), (true, true, true));
    assert_eq!(
        on.opts.grant.entry("msg.send").unwrap().reasons_string(),
        "declared:*,ambient:messaging_default"
    );
    assert!(!on.opts.identity().launch.messaging_off);

    let some = assembler
        .assemble(launch(
            &home,
            agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [read_file]"),
            step_origin(MessagingSwitch::Tools(vec!["board.post", "msg.send"])),
            "run_step_some",
        ))
        .await
        .unwrap();
    assert_eq!(messaging_of(&some), (true, true, true));
    assert_eq!(
        some.opts.grant.entry("msg.send").unwrap().reasons_string(),
        "ambient:workflow_messaging"
    );
    assert!(!some.opts.grant.offers("board.claim"), "only the tools named");
}

/// Spec F1d D6: a process child launched by an opted-out run reads
/// `--launch-no-messaging` and has messaging off, whatever its agent says.
#[tokio::test]
#[serial]
async fn a_process_child_of_an_opted_out_run_has_messaging_off() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let spec = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: [msg.send]");
    let link = |no_messaging| rupu_runtime::argv::LaunchLink {
        depth: 1,
        ceiling: PermissionMode::Bypass,
        parent_run: "run_parent".into(),
        root_run: "run_parent".into(),
        usage_root: None,
        agents: vec![],
        workflows: vec![],
        actions: None,
        no_messaging,
    };
    let mut l = launch(&home, spec.clone(), Origin::Standalone, "run_child_off");
    l.link = Some(link(true));
    let _m = MockScript::set(DONE);
    let off = assembler.assemble(l).await.unwrap();
    assert_eq!(messaging_of(&off), (true, false, false));
    assert_eq!(
        off.opts.grant.unavailable.iter().map(|u| u.tool).collect::<Vec<_>>(),
        vec!["msg.send"]
    );
    assert!(off.opts.identity().launch.messaging_off);

    let mut l = launch(&home, spec, Origin::Standalone, "run_child_on");
    l.link = Some(link(false));
    let on = assembler.assemble(l).await.unwrap();
    assert_eq!(messaging_of(&on), (true, true, true));
    assert!(!on.opts.identity().launch.messaging_off);
}

/// Spec F1d D3: an agentiflow's `messaging: false` turns the lead's
/// messaging off, its status and roster tools stay, and a unit launched
/// under its cap is off too.
#[tokio::test]
#[serial]
async fn agentiflow_messaging_off_reaches_lead_and_units() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let flow_dir = home.global().join("agentiflows").join("af_1");
    let star = agent(&home, "provider: anthropic\nmodel: claude-sonnet-4-6\ntools: ['*']");
    let mut l = launch(
        &home,
        star.clone(),
        Origin::FlowLead {
            flow_id: "af_1".into(),
            flow_dir: flow_dir.clone(),
            messaging_off: true,
        },
        "af_1",
    );
    l.services.launcher = Some(Arc::new(crate::support::NoLaunch));
    l.services.message_bus = Some(Arc::new(rupu_fleet::Bus::open(&flow_dir, "lead")));
    l.services.run_status = Some(Arc::new(crate::support::NoStatus));
    let _m = MockScript::set(DONE);
    let lead = assembler.assemble(l).await.unwrap();
    assert_eq!(messaging_of(&lead), (true, false, false));
    for tool in ["board.directive", "board.retract", "board.post", "msg.send"] {
        assert!(!lead.opts.grant.offers(tool), "{tool}");
    }
    for tool in ["goal.status", "budget.status", "agents.list", "dispatch"] {
        assert!(lead.opts.grant.offers(tool), "{tool}");
    }
    assert!(lead.opts.identity().launch.messaging_off);

    let mut u = launch(
        &home,
        star,
        Origin::FlowUnit {
            flow_id: "af_1".into(),
            flow_dir: flow_dir.clone(),
            participant: "probe#1".into(),
        },
        "run_unit",
    );
    u.services.message_bus = Some(Arc::new(rupu_fleet::Bus::open(&flow_dir, "probe#1")));
    u.link = Some(rupu_runtime::argv::LaunchLink {
        depth: 1,
        ceiling: PermissionMode::Bypass,
        parent_run: "af_1".into(),
        root_run: "af_1".into(),
        usage_root: None,
        agents: vec![],
        workflows: vec![],
        actions: None,
        no_messaging: lead.opts.identity().launch.messaging_off,
    });
    let unit = assembler.assemble(u).await.unwrap();
    assert_eq!(messaging_of(&unit), (true, false, false));
    assert!(!unit.opts.grant.offers("board.claim"));
}
