//! `descriptors_complete` (W1 §6.2): the catalog lists every tool rupu
//! defines, canonical names are unique, and no alias collides with another
//! tool's name or alias (except the flow-lead-scoped `coverage.status`).

use rupu_tools::{
    AliasScope, AstGrepTool, BashTool, DispatchAgentTool, DispatchAgentsParallelTool, EditFileTool,
    GlobTool, GrepTool, ReadFileTool, Tool, ToolCatalog, ToolContext, WriteFileTool,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `impl Tool for` blocks in production code (before the file's test module),
/// per tool home. W5 moves the agentiflow bodies into this crate and this
/// list shrinks to it.
fn production_impls() -> usize {
    let homes: &[&str] = &[
        include_str!("../../src/bash.rs"),
        include_str!("../../src/read_file.rs"),
        include_str!("../../src/write_file.rs"),
        include_str!("../../src/edit_file.rs"),
        include_str!("../../src/grep.rs"),
        include_str!("../../src/glob.rs"),
        include_str!("../../src/ast_grep.rs"),
        include_str!("../../src/dispatch_agent.rs"),
        include_str!("../../src/dispatch_agents_parallel.rs"),
        include_str!("../../src/coverage/mod.rs"),
        include_str!("../../src/findings/report.rs"),
        include_str!("../../src/findings/verify.rs"),
        include_str!("../../src/findings/query.rs"),
        include_str!("../../src/findings/tag.rs"),
        include_str!("../../src/assets/mark.rs"),
        include_str!("../../src/scm/repos.rs"),
        include_str!("../../src/scm/branches.rs"),
        include_str!("../../src/scm/files.rs"),
        include_str!("../../src/scm/prs.rs"),
        include_str!("../../src/issues/mod.rs"),
        include_str!("../../src/github/mod.rs"),
        include_str!("../../src/gitlab/mod.rs"),
        include_str!("../../../rupu-agentiflow/src/tools.rs"),
        include_str!("../../../rupu-agentiflow/src/status_tools.rs"),
        include_str!("../../../rupu-agentiflow/src/dispatch_tools.rs"),
        include_str!("../../../rupu-agentiflow/src/roster.rs"),
    ];
    homes
        .iter()
        .map(|src| {
            let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
            prod.matches("impl Tool for ").count()
        })
        .sum()
}

#[test]
fn every_tool_impl_has_a_catalog_descriptor() {
    assert_eq!(
        production_impls(),
        ToolCatalog::all().len(),
        "a `Tool` impl was added or removed without updating `catalog::ALL` \
         (or a descriptor was listed for a tool that no longer exists)"
    );
    // The builtins return the very descriptors the catalog lists.
    let builtins: [&dyn Tool; 9] = [
        &BashTool,
        &ReadFileTool,
        &WriteFileTool,
        &EditFileTool,
        &GrepTool,
        &GlobTool,
        &AstGrepTool,
        &DispatchAgentTool,
        &DispatchAgentsParallelTool,
    ];
    for t in builtins {
        assert!(
            ToolCatalog::all()
                .iter()
                .any(|d| std::ptr::eq(*d, t.descriptor())),
            "{} is not in the catalog",
            t.name()
        );
    }
}

#[test]
fn names_are_unique_and_aliases_never_collide() {
    let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
    for d in ToolCatalog::all() {
        assert!(
            owner.insert(d.name, d.name).is_none(),
            "canonical name `{}` is declared twice",
            d.name
        );
    }
    for d in ToolCatalog::all() {
        for a in d.aliases {
            if a.scope == AliasScope::FlowLead {
                // Resolved by scope, not by name: the one such alias is the
                // ledger tool's canonical name outside a lead.
                assert_eq!((d.name, a.name), ("goal.coverage", "coverage.status"));
                continue;
            }
            if let Some(prev) = owner.insert(a.name, d.name) {
                panic!("alias `{}` of `{}` collides with `{prev}`", a.name, d.name);
            }
        }
    }
}

#[test]
fn every_descriptor_has_a_schema_object_and_a_description() {
    for d in ToolCatalog::all() {
        assert!(!d.description.trim().is_empty(), "{}", d.name);
        let schema = (d.input_schema)();
        assert_eq!(schema["type"], "object", "{}", d.name);
    }
}

#[test]
fn canonical_names_are_namespaced_except_core_and_legacy_dispatch() {
    for d in ToolCatalog::all() {
        let legacy = matches!(
            d.name,
            "dispatch" | "join" | "run_workflow" | "dispatch_agent" | "dispatch_agents_parallel"
        );
        assert!(
            d.is_core() || legacy || d.name.contains('.'),
            "`{}` is neither core nor `namespace.verb`",
            d.name
        );
    }
}

#[test]
fn non_shell_names_cover_legacy_and_canonical_but_never_real_commands() {
    let names: Vec<&str> = ToolCatalog::non_shell_names().collect();
    for n in [
        "findings.report",
        "report_finding",
        "assets.mark",
        "asset_mark",
        "finding.verify",
        "coverage.status",
        "coverage_status",
    ] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
    for n in ["grep", "glob", "bash", "join", "dispatch"] {
        assert!(!names.contains(&n), "{n} is a shell command, not reserved");
    }
}

/// Every descriptor whose body lives in this crate is built by
/// `bodies::body` for a context that provides every service, and the body
/// answers with that very descriptor. The agentiflow tools (bodies in
/// `rupu-agentiflow` until W5) are the only ones without a body here.
#[test]
fn every_catalog_tool_but_the_flow_tools_has_a_body() {
    let ctx = full_context();
    for d in ToolCatalog::all() {
        let flow = rupu_tools::catalog::flow::ALL
            .iter()
            .any(|f| std::ptr::eq(*f, *d));
        match rupu_tools::bodies::body(d.name, &ctx) {
            Some(t) => {
                assert!(!flow, "{} is a flow tool with a body here", d.name);
                assert!(std::ptr::eq(t.descriptor(), *d), "{}", d.name);
            }
            None => assert!(flow, "{} has no body", d.name),
        }
    }
}

/// `ToolServices::provided` and `bodies::body` agree: in a bare context
/// (no registry, no catalog, no engagement, no dispatcher) a tool has a body
/// exactly when the services it needs are provided.
#[test]
fn provided_services_match_the_bodies_that_build() {
    for (ctx, coverage) in [(ToolContext::default(), false), (full_context(), true)] {
        let provided = ctx.services.provided(coverage);
        for d in ToolCatalog::all() {
            if rupu_tools::catalog::flow::ALL
                .iter()
                .any(|f| std::ptr::eq(*f, *d))
            {
                continue;
            }
            assert_eq!(
                rupu_tools::bodies::body(d.name, &ctx).is_some(),
                provided.missing_for(d).is_empty(),
                "{}",
                d.name
            );
        }
    }
}

#[derive(Debug)]
struct NoDispatch;

#[async_trait::async_trait]
impl rupu_tools::AgentDispatcher for NoDispatch {
    async fn dispatch(
        &self,
        _agent: &str,
        _prompt: String,
        _parent: &rupu_tools::RunIdentity,
        _permission: rupu_tools::SpawnPermission,
    ) -> Result<rupu_tools::DispatchOutcome, rupu_tools::DispatchError> {
        unreachable!("never called")
    }
}

/// A context providing every service a body in this crate can need.
fn full_context() -> ToolContext {
    let mut ctx = ToolContext::default();
    let s = &mut ctx.services;
    s.scm = Some(Arc::new(rupu_scm::Registry::default()));
    s.dispatcher = Some(Arc::new(NoDispatch));
    s.coverage_catalog = Some(Arc::new(
        rupu_coverage::flatten(&rupu_coverage::ConcernsBlock::default()).unwrap(),
    ));
    s.findings = Some(rupu_coverage::FindingWriteOptions {
        engagement: Some(Arc::new(
            rupu_coverage::builtin_registry()
                .unwrap()
                .active_set(&["network".into()])
                .unwrap(),
        )),
        ..Default::default()
    });
    ctx
}

/// Which tools an `action:` step may call (W4 §3.3): the connector tools and
/// the findings tools — never a core tool, a spawn, or one needing a
/// service an action step can't provide (a concern catalog, an engagement, a
/// dispatcher, a flow).
#[test]
fn action_eligibility_is_connectors_and_findings() {
    let eligible: Vec<&str> = ToolCatalog::all()
        .iter()
        .filter(|d| d.is_action_eligible())
        .map(|d| d.name)
        .collect();
    let mut want: Vec<&str> = ToolCatalog::all()
        .iter()
        .filter(|d| d.is_connector())
        .map(|d| d.name)
        .collect();
    want.extend([
        "findings.report",
        "findings.verify",
        "findings.query",
        "findings.tag",
    ]);
    let mut got = eligible.clone();
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want);
    assert_eq!(want.len(), 22);
}

/// W4 §6.6: no `impl Tool for` in production code outside this crate. Test
/// modules may implement it (fakes); the agentiflow tools move in with W5.
#[test]
fn no_tool_is_implemented_outside_this_crate() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap() {
        let krate = entry.unwrap().path();
        let name = krate.file_name().unwrap().to_string_lossy().to_string();
        if name == "rupu-tools" || name == "rupu-agentiflow" {
            continue;
        }
        scan(&krate.join("src"), &mut offenders);
    }
    assert!(
        offenders.is_empty(),
        "`impl Tool for` outside rupu-tools (production code): {offenders:?}"
    );
}

fn scan(dir: &std::path::Path, offenders: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, offenders);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let src = std::fs::read_to_string(&path).unwrap();
            let prod = src.split("#[cfg(test)]").next().unwrap_or(&src);
            if prod.contains("impl Tool for ") || prod.contains("impl rupu_tools::Tool for ") {
                offenders.push(path.display().to_string());
            }
        }
    }
}
