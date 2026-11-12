//! `descriptors_complete` (W1 §6.2): the catalog lists every tool rupu
//! defines, canonical names are unique, and no alias collides with another
//! tool's name or alias (except the flow-lead-scoped `coverage.status`).

use rupu_tools::{
    AliasScope, AstGrepTool, BashTool, EditFileTool, GlobTool, GrepTool, ReadFileTool, Tool,
    ToolCatalog, ToolContext, WriteFileTool,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `impl Tool for` blocks in production code (before the file's test module),
/// per tool home. Since W7 every home is in this crate.
fn production_impls() -> usize {
    let homes: &[&str] = &[
        include_str!("../../src/bash.rs"),
        include_str!("../../src/read_file.rs"),
        include_str!("../../src/write_file.rs"),
        include_str!("../../src/edit_file.rs"),
        include_str!("../../src/grep.rs"),
        include_str!("../../src/glob.rs"),
        include_str!("../../src/ast_grep.rs"),
        include_str!("../../src/launch/dispatch.rs"),
        include_str!("../../src/launch/join.rs"),
        include_str!("../../src/launch/generate.rs"),
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
        include_str!("../../src/board/mod.rs"),
        include_str!("../../src/msg/mod.rs"),
        include_str!("../../src/roster/mod.rs"),
        include_str!("../../src/goal/mod.rs"),
        include_str!("../../src/budget/mod.rs"),
        include_str!("../../src/skills/mod.rs"),
        include_str!("../../src/session_reads.rs"),
        include_str!("../../src/work_queue.rs"),
    ];
    homes
        .iter()
        .map(|src| {
            let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
            prod.matches("impl Tool for ").count()
        })
        .sum::<usize>()
}

#[test]
fn every_tool_impl_has_a_catalog_descriptor() {
    // These bodies intentionally serve explicit descriptor families. Count
    // their additional descriptors, while the body test below checks each
    // actual catalog entry resolves to its identical descriptor pointer.
    let session_family = [
        &rupu_tools::session_reads::CONTEXT,
        &rupu_tools::session_reads::WORKFLOWS,
        &rupu_tools::session_reads::INSPECT,
        &rupu_tools::session_reads::NETFLOWS,
    ];
    for descriptor in session_family
        .iter()
        .copied()
        .chain(rupu_tools::work_queue::DESCRIPTORS.iter().copied())
    {
        assert!(ToolCatalog::all()
            .iter()
            .any(|known| std::ptr::eq(*known, descriptor)));
    }
    assert_eq!(
        production_impls() + session_family.len() - 1 + rupu_tools::work_queue::DESCRIPTORS.len()
            - 1,
        ToolCatalog::all().len(),
        "a `Tool` impl was added or removed without updating `catalog::ALL` \
         (or a descriptor was listed for a tool that no longer exists)"
    );
    // The builtins return the very descriptors the catalog lists.
    let builtins: [&dyn Tool; 10] = [
        &BashTool,
        &ReadFileTool,
        &WriteFileTool,
        &EditFileTool,
        &GrepTool,
        &GlobTool,
        &AstGrepTool,
        &rupu_tools::launch::DispatchTool,
        &rupu_tools::launch::JoinTool,
        &rupu_tools::launch::WorkflowsGenerateTool,
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
fn canonical_names_are_namespaced_except_core_and_the_launch_pair() {
    for d in ToolCatalog::all() {
        let launch = matches!(d.name, "dispatch" | "join");
        assert!(
            d.is_core() || launch || d.name.contains('.'),
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

/// Every descriptor is built by `bodies::body` for a context that provides
/// every service, and the body answers with that very descriptor.
#[test]
fn every_catalog_tool_has_a_body() {
    let ctx = full_context();
    for d in ToolCatalog::all() {
        let t = rupu_tools::bodies::body(d.name, &ctx)
            .unwrap_or_else(|| panic!("{} has no body", d.name));
        assert!(std::ptr::eq(t.descriptor(), *d), "{}", d.name);
    }
}

/// `ToolServices::provided` and `bodies::body` agree: in a bare context
/// (no registry, no catalog, no engagement, no launcher) a tool has a body
/// exactly when the services it needs are provided.
#[test]
fn provided_services_match_the_bodies_that_build() {
    for (ctx, coverage) in [(ToolContext::default(), false), (full_context(), true)] {
        let provided = ctx.services.provided(coverage);
        for d in ToolCatalog::all() {
            assert_eq!(
                rupu_tools::bodies::body(d.name, &ctx).is_some(),
                provided.missing_for(d).is_empty(),
                "{}",
                d.name
            );
        }
    }
}

/// A context providing every service a body in this crate can need.
fn full_context() -> ToolContext {
    let mut ctx = ToolContext::default();
    let s = &mut ctx.services;
    s.scm = Some(Arc::new(rupu_scm::Registry::default()));
    s.launcher = Some(Arc::new(crate::support::ScriptedLauncher::default()));
    s.workflow_generator = Some(Arc::new(crate::support::StubGenerator));
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
    s.message_bus = Some(Arc::new(rupu_fleet::Bus::open(
        std::path::Path::new("/nonexistent/af"),
        "lead",
    )));
    s.work_queue = Some(Arc::new(DescriptorQueue));
    s.session_reads = Some(Arc::new(DescriptorSessionReads));
    s.run_status = Some(Arc::new(crate::support::StubStatus::default()));
    s.catalog = Some(Arc::new(crate::support::StubCatalog::default()));
    let global = tempfile::tempdir().unwrap();
    let skill = global.path().join("skills/pdf");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: pdf\ndescription: d\n---\nx\n",
    )
    .unwrap();
    s.skills = Some(crate::support::resolved(global.path(), &["pdf"]));
    ctx
}

/// Catalog construction only: invoking this fixture is a test error. No owner
/// record, Queue store, provider, or filesystem mutation is needed to enumerate.
struct DescriptorQueue;
impl rupu_tools::work_queue::WorkQueuePort for DescriptorQueue {
    fn binding(&self) -> rupu_tools::work_queue::WorkOwnerBinding {
        panic!("descriptor-only fixture")
    }
    fn actor(&self) -> rupu_fleet::WorkActor {
        panic!("descriptor-only fixture")
    }
    fn finish_execution(&self) {}
    fn list(
        &self,
        _: rupu_fleet::WorkPageQuery,
    ) -> Result<rupu_fleet::WorkPage, rupu_fleet::WorkError> {
        panic!("descriptor-only fixture")
    }
    fn get(&self, _: &str) -> Result<rupu_fleet::WorkItem, rupu_fleet::WorkError> {
        panic!("descriptor-only fixture")
    }
    fn timeline(
        &self,
        _: rupu_fleet::WorkTimelineQuery,
    ) -> Result<rupu_fleet::WorkTimelinePage, rupu_fleet::WorkError> {
        panic!("descriptor-only fixture")
    }
    fn apply(
        &self,
        _: rupu_fleet::WorkCommand,
        _: rupu_fleet::WorkSource,
    ) -> Result<rupu_fleet::WorkReply, rupu_fleet::WorkError> {
        panic!("descriptor-only fixture")
    }
}
struct DescriptorSessionReads;
impl rupu_tools::ports::SessionReadPort for DescriptorSessionReads {
    fn permits(&self, name: &str) -> bool {
        matches!(
            name,
            "session.context" | "session.workflows" | "run.inspect" | "run.netflows"
        )
    }
    fn read(
        &self,
        _: rupu_tools::ports::SessionReadRequest,
        _: &ToolContext,
    ) -> Result<serde_json::Value, rupu_tools::PortError> {
        panic!("descriptor-only fixture")
    }
}

#[test]
fn queue_body_and_service_require_both_bus_and_exact_runtime_binding() {
    let mut context = full_context();
    context.services.work_queue = None;
    for descriptor in rupu_tools::work_queue::DESCRIPTORS {
        assert!(rupu_tools::bodies::body(descriptor.name, &context).is_none());
        assert!(!context
            .services
            .provided(false)
            .missing_for(descriptor)
            .is_empty());
    }
    context.services.work_queue = Some(Arc::new(DescriptorQueue));
    context.services.message_bus = None;
    assert!(!context
        .services
        .provided(false)
        .contains(rupu_tools::Service::WorkQueue));
    assert!(rupu_tools::bodies::body("board.work_post", &context).is_none());
}

/// `skills.load` needs a skill the model may load (spec §2.4): a run whose
/// every granted skill is `disable-model-invocation` and not preloaded keeps
/// its skills but is not offered the loader.
#[test]
fn skills_load_needs_a_model_loadable_skill() {
    let mut ctx = ToolContext::default();
    ctx.services.skills = Some(Arc::new(rupu_skills::ResolvedSkills::default()));
    assert!(rupu_tools::bodies::body("skills.load", &ctx).is_none());
    assert!(!ctx
        .services
        .provided(false)
        .missing_for(&rupu_tools::skills::LOAD)
        .is_empty());
    let full = full_context();
    assert!(rupu_tools::bodies::body("skills.load", &full).is_some());
    assert!(full
        .services
        .provided(false)
        .missing_for(&rupu_tools::skills::LOAD)
        .is_empty());
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

/// W4 §6.6 / W7 §7: no `impl Tool for` in production code outside this
/// crate. Test modules may implement it (fakes).
#[test]
fn no_tool_is_implemented_outside_this_crate() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap() {
        let krate = entry.unwrap().path();
        let name = krate.file_name().unwrap().to_string_lossy().to_string();
        if name == "rupu-tools" {
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
