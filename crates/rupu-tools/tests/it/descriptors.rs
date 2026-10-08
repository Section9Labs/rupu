//! `descriptors_complete` (W1 §6.2): the catalog lists every tool rupu
//! defines, canonical names are unique, and no alias collides with another
//! tool's name or alias (except the flow-lead-scoped `coverage.status`).

use rupu_tools::{
    AliasScope, AstGrepTool, BashTool, DispatchAgentTool, DispatchAgentsParallelTool, EditFileTool,
    GlobTool, GrepTool, ReadFileTool, Tool, ToolCatalog, WriteFileTool,
};
use std::collections::BTreeMap;

/// `impl Tool for` blocks in production code (before the file's test module),
/// per tool home. W4/W5 move the bodies into this crate and this list
/// shrinks to it.
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
        include_str!("../../../rupu-agent/src/coverage_tools.rs"),
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
