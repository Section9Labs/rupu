//! The grant resolver (W2 §6.1–6.3): grammar, `*` meaning the whole catalog
//! with or without `actions:`, and wildcards skipping tools whose service is
//! missing.

use rupu_tools::{
    AliasScope, AmbientGrant, Effect, GrantError, GrantInputs, GrantReason, ResolvedGrant, Service,
    ServiceSet, ToolCatalog, ToolDescriptor, DEFAULT_GRANT,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn schema() -> Value {
    serde_json::json!({"type": "object"})
}

/// Two stand-in connector tools, as `rupu-agent` adds the MCP catalog's.
static ISSUES_GET: ToolDescriptor = ToolDescriptor {
    name: "issues.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "",
    input_schema: schema,
};
static SCM_PRS_GET: ToolDescriptor = ToolDescriptor {
    name: "scm.prs.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "",
    input_schema: schema,
};

fn catalog() -> ToolCatalog {
    ToolCatalog::builtin().with([&ISSUES_GET, &SCM_PRS_GET])
}

/// What a plain `rupu run` with an SCM registry and a dispatcher provides.
fn plain_run() -> ServiceSet {
    [
        Service::Findings,
        Service::Netflow,
        Service::Scm,
        Service::AgentDispatcher,
    ]
    .into_iter()
    .collect()
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn resolve(declared: Option<&[&str]>, actions: &[&str]) -> Result<ResolvedGrant, GrantError> {
    resolve_with(declared, actions, &[], &plain_run())
}

fn resolve_with(
    declared: Option<&[&str]>,
    actions: &[&str],
    ambient: &[AmbientGrant],
    available: &ServiceSet,
) -> Result<ResolvedGrant, GrantError> {
    let declared = declared.map(strs);
    let actions = strs(actions);
    catalog().resolve_grant(GrantInputs {
        declared: declared.as_deref(),
        step_actions: &actions,
        ambient,
        available,
        alias_scope: AliasScope::Everywhere,
    })
}

fn offered(g: &ResolvedGrant) -> BTreeSet<&'static str> {
    g.entries.keys().copied().collect()
}

fn set(v: &[&'static str]) -> BTreeSet<&'static str> {
    v.iter().copied().collect()
}

const CORE: &[&str] = &[
    "bash",
    "read_file",
    "write_file",
    "edit_file",
    "grep",
    "glob",
    "ast_grep",
];

#[test]
fn grant_grammar() {
    // exact canonical names
    let g = resolve(Some(&["bash", "read_file"]), &[]).unwrap();
    assert_eq!(offered(&g), set(&["bash", "read_file"]));
    assert_eq!(g.entries["bash"].reasons, vec![GrantReason::Declared]);

    // an alias resolves to its canonical tool
    let g = resolve(Some(&["report_finding"]), &[]).unwrap();
    assert_eq!(offered(&g), set(&["findings.report"]));

    // ns.* = every tool in that namespace
    let g = resolve(Some(&["findings.*"]), &[]).unwrap();
    assert_eq!(
        offered(&g),
        set(&[
            "findings.report",
            "findings.verify",
            "findings.query",
            "findings.tag"
        ])
    );
    assert_eq!(
        g.entries["findings.tag"].reasons,
        vec![GrantReason::DeclaredWildcard("findings.*".into())]
    );

    // core.* = the unqualified core tools
    let g = resolve(Some(&["core.*"]), &[]).unwrap();
    assert_eq!(offered(&g), CORE.iter().copied().collect());

    // * = the whole catalog this run can serve
    let g = resolve(Some(&["*"]), &[]).unwrap();
    for t in CORE
        .iter()
        .chain(&["issues.get", "scm.prs.get", "findings.report"])
    {
        assert!(g.offers(t), "* should offer {t}");
    }

    // omitted = DEFAULT_GRANT
    let g = resolve(None, &[]).unwrap();
    let mut want: BTreeSet<&str> = CORE.iter().copied().collect();
    want.extend([
        "dispatch_agent",
        "dispatch_agents_parallel",
        "issues.get",
        "scm.prs.get",
    ]);
    assert_eq!(offered(&g), want);
    assert!(g
        .entries
        .values()
        .all(|e| e.reasons == vec![GrantReason::Default]));
    assert!(DEFAULT_GRANT.contains(&"core.*"));

    // an explicit empty list grants nothing
    let g = resolve(Some(&[]), &[]).unwrap();
    assert!(g.entries.is_empty());

    // unknown -> error with a did-you-mean
    let err = resolve(Some(&["repot_finding"]), &[]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown tool \"repot_finding\" in tools: (did you mean \"report_finding\" → findings.report?)"
    );
    let err = resolve(Some(&["bahs"]), &[]).unwrap_err();
    assert!(err.to_string().contains("did you mean \"bash\"?"), "{err}");
    // no near spelling -> no suggestion
    let err = resolve(Some(&["completely_made_up_tool"]), &[]).unwrap_err();
    assert_eq!(
        err,
        GrantError::UnknownTool {
            field: "tools:",
            name: "completely_made_up_tool".into(),
            suggestion: None
        }
    );

    // unknown namespace -> error
    let err = resolve(Some(&["nosuch.*"]), &[]).unwrap_err();
    assert!(matches!(err, GrantError::UnknownNamespace { .. }), "{err}");
    // there are no other globs (D9)
    let err = resolve(Some(&["scm*"]), &[]).unwrap_err();
    assert!(err.to_string().contains("did you mean \"scm.*\"?"), "{err}");
    assert!(resolve(Some(&["coverage_*"]), &[]).is_err());
}

#[test]
fn validate_names_checks_names_only() {
    let c = catalog();
    // board.post's service is missing in most runs, but the name is valid.
    c.validate_names(&strs(&["board.post", "*", "scm.*", "asset_mark"]))
        .unwrap();
    let err = c
        .validate_names(&strs(&["read_file", "repot_finding"]))
        .unwrap_err();
    assert!(err.to_string().contains("repot_finding"));
}

#[test]
fn star_means_catalog_everywhere() {
    // Regression for T8: `*` meant zero builtins in `rupu run` and every
    // builtin under a step with `actions:`.
    let free = resolve(Some(&["*"]), &[]).unwrap();
    let narrowed = resolve(Some(&["*"]), &["issues.get"]).unwrap();

    let non_connector = |g: &ResolvedGrant| -> BTreeSet<&'static str> {
        g.entries
            .values()
            .filter(|e| !e.descriptor.is_connector())
            .map(|e| e.canonical)
            .collect()
    };
    let connector = |g: &ResolvedGrant| -> BTreeSet<&'static str> {
        g.entries
            .values()
            .filter(|e| e.descriptor.is_connector())
            .map(|e| e.canonical)
            .collect()
    };
    assert_eq!(non_connector(&free), non_connector(&narrowed));
    assert!(CORE.iter().all(|t| free.offers(t)));
    assert_eq!(connector(&free), set(&["issues.get", "scm.prs.get"]));
    assert_eq!(connector(&narrowed), set(&["issues.get"]));
    assert_eq!(narrowed.narrowed, vec!["scm.prs.get"]);
}

#[test]
fn actions_never_escalate_and_report_ungranted_names() {
    let g = resolve(Some(&["read_file", "issues.get"]), &["scm.prs.get"]).unwrap();
    assert_eq!(offered(&g), set(&["read_file"]));
    assert_eq!(g.narrowed, vec!["issues.get"]);
    assert_eq!(g.actions_not_granted, vec!["scm.prs.get"]);
    assert!(g.declared_by_actions("scm.prs.get"));
    assert!(!g.granted_before_narrowing("scm.prs.get"));
    assert!(g.granted_before_narrowing("issues.get"));
}

#[test]
fn actions_vocabulary() {
    let c = catalog();
    let s = AliasScope::Everywhere;
    assert_eq!(
        c.resolve_actions(&strs(&["issues.*"]), s).unwrap(),
        set(&["issues.get"])
    );
    assert_eq!(
        c.resolve_actions(&strs(&["*"]), s).unwrap(),
        set(&["issues.get", "scm.prs.get"])
    );
    assert!(matches!(
        c.resolve_actions(&strs(&["bash"]), s),
        Err(GrantError::NotConnector { .. })
    ));
    assert!(matches!(
        c.resolve_actions(&strs(&["findings.*"]), s),
        Err(GrantError::NotConnector { .. })
    ));
    let err = c.resolve_actions(&strs(&["issues.gte"]), s).unwrap_err();
    assert!(err.to_string().contains("in actions:"), "{err}");
}

#[test]
fn wildcard_skips_missing_service() {
    // A plain `rupu run` has no message bus: `*` offers no `board.*`, and
    // says nothing about it.
    let g = resolve(Some(&["*"]), &[]).unwrap();
    assert!(!g.entries.keys().any(|k| k.starts_with("board.")));
    assert!(g.unavailable.is_empty(), "{:?}", g.unavailable);
    assert!(g.skipped.iter().any(|u| u.tool == "board.post"));

    // Named exactly: not offered, and reported as unavailable (one notice).
    let g = resolve(Some(&["board.post"]), &[]).unwrap();
    assert!(g.entries.is_empty());
    assert_eq!(g.unavailable.len(), 1);
    assert_eq!(g.unavailable[0].tool, "board.post");
    assert_eq!(g.unavailable[0].missing, vec![Service::MessageBus]);
    assert_eq!(
        g.unavailable[0].notice_message(),
        "tool \"board.post\" is not available in this run: it needs message_bus, which this run does not provide"
    );

    // Named exactly after a wildcard skipped it: unavailable, not skipped.
    let g = resolve(Some(&["*", "board.post"]), &[]).unwrap();
    assert_eq!(g.unavailable.len(), 1);
    assert!(!g.skipped.iter().any(|u| u.tool == "board.post"));
}

#[test]
fn ambient_is_visible() {
    let available = plain_run().with(Service::Coverage);
    let g = resolve_with(
        Some(&["read_file"]),
        &["issues.get"],
        &[AmbientGrant::concerns()],
        &available,
    )
    .unwrap();
    for t in [
        "coverage.mark",
        "coverage.status",
        "coverage.remaining",
        "coverage.concerns.search",
        "coverage.concerns.detail",
        "findings.report",
    ] {
        assert_eq!(
            g.entries[t].reasons,
            vec![GrantReason::Ambient("concerns")],
            "{t}"
        );
        assert_eq!(g.entries[t].reasons_string(), "ambient:concerns");
    }
    assert_eq!(g.entries["read_file"].reasons, vec![GrantReason::Declared]);

    // A tool both declared and ambient carries both reasons.
    let g = resolve_with(
        Some(&["report_finding"]),
        &[],
        &[AmbientGrant::engagement()],
        &plain_run().with(Service::Engagement),
    )
    .unwrap();
    assert_eq!(
        g.entries["findings.report"].reasons_string(),
        "declared,ambient:engagement"
    );
    assert_eq!(
        g.entries["assets.mark"].reasons,
        vec![GrantReason::Ambient("engagement")]
    );
}

#[test]
fn services_gate_exactly() {
    // Core tools only *use* coverage / netflow: they are offered in a run
    // that provides nothing at all.
    let g = resolve_with(Some(&["core.*"]), &[], &[], &ServiceSet::new()).unwrap();
    assert_eq!(offered(&g), CORE.iter().copied().collect());

    // The sub-agent dispatch pair needs the in-process dispatcher.
    let no_dispatcher: ServiceSet = [Service::Findings].into_iter().collect();
    let g = resolve_with(Some(&["*", "dispatch_agent"]), &[], &[], &no_dispatcher).unwrap();
    assert!(!g.offers("dispatch_agent"));
    assert_eq!(g.unavailable.len(), 1);
    assert_eq!(g.unavailable[0].tool, "dispatch_agent");
    assert_eq!(g.unavailable[0].missing, vec![Service::AgentDispatcher]);
    assert!(g
        .skipped
        .iter()
        .any(|u| u.tool == "dispatch_agents_parallel"));

    // `assets.mark` needs an engagement; `findings.report` does not.
    let g = resolve(Some(&["asset_mark", "report_finding"]), &[]).unwrap();
    assert_eq!(offered(&g), set(&["findings.report"]));
    assert_eq!(g.unavailable[0].missing, vec![Service::Engagement]);
    let g = resolve_with(
        Some(&["asset_mark"]),
        &[],
        &[],
        &plain_run().with(Service::Engagement),
    )
    .unwrap();
    assert_eq!(offered(&g), set(&["assets.mark"]));

    // The flow `dispatch` is the unit launcher, not the sub-agent
    // dispatcher: a run with only the latter doesn't offer it.
    let g = resolve(Some(&["*"]), &[]).unwrap();
    assert!(g.offers("dispatch_agent"));
    assert!(!g.offers("dispatch") && !g.offers("join"));
}

#[test]
fn injected_tools_are_self_served() {
    // A unit's injected board tools are offered although the run provides
    // no message bus, and `*` does not drag in the board tools that were
    // not injected.
    let g = resolve_with(
        Some(&["*"]),
        &[],
        &[AmbientGrant::injected(vec!["board.post", "board.read"])],
        &plain_run(),
    )
    .unwrap();
    assert!(g.offers("board.post") && g.offers("board.read"));
    assert!(!g.offers("board.directive"));
    assert_eq!(
        g.entries["board.post"].reasons,
        vec![GrantReason::Origin("injected")]
    );
    assert!(!g.skipped.iter().any(|u| u.tool == "board.post"));
    assert!(g.skipped.iter().any(|u| u.tool == "board.directive"));
}

#[test]
fn flow_lead_alias_scope() {
    let c = catalog();
    assert_eq!(
        c.resolve_name("coverage.status", AliasScope::Everywhere)
            .unwrap()
            .name,
        "coverage.status"
    );
    assert_eq!(
        c.resolve_name("coverage.status", AliasScope::FlowLead)
            .unwrap()
            .name,
        "goal.coverage"
    );
    assert_eq!(
        c.resolve_name("coverage_status", AliasScope::FlowLead)
            .unwrap()
            .name,
        "coverage.status"
    );
}
