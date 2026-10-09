use rupu_agent::ToolRegistry;
use rupu_tools::{AliasScope, Tool, ToolCatalog, ToolContext};
use std::sync::Arc;

/// The catalog body of `name` in a context that can dispatch.
fn builtin_tool(name: &str) -> Option<Arc<dyn Tool>> {
    let mut ctx = ToolContext::default();
    ctx.services.dispatcher = Some(Arc::new(NoDispatch));
    rupu_tools::bodies::body(name, &ctx)
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

const BUILTINS: [&str; 9] = [
    "ast_grep",
    "bash",
    "dispatch_agent",
    "dispatch_agents_parallel",
    "edit_file",
    "glob",
    "grep",
    "read_file",
    "write_file",
];

fn registry_of(names: &[&str]) -> ToolRegistry {
    let mut r = ToolRegistry::new();
    for n in names {
        r.insert(builtin_tool(n).unwrap_or_else(|| panic!("no builtin {n}")));
    }
    r
}

#[test]
fn every_builtin_has_a_body_under_its_canonical_name() {
    for name in BUILTINS {
        let t = builtin_tool(name).unwrap_or_else(|| panic!("expected tool {name}"));
        assert_eq!(t.name(), name);
    }
    assert!(builtin_tool("teleport").is_none());
}

#[test]
fn known_tools_returns_sorted_list() {
    let r = registry_of(&BUILTINS);
    assert_eq!(r.known_tools(), BUILTINS.to_vec());
}

#[test]
fn unknown_tool_is_none() {
    assert!(registry_of(&["bash"]).get("teleport").is_none());
}

#[test]
fn to_tool_definitions_match_the_registry() {
    let r = registry_of(&["bash", "read_file"]);
    let defs = r.to_tool_definitions();
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["bash", "read_file"]);
    for d in r.to_tool_definitions() {
        assert!(!d.description.is_empty(), "{}: empty description", d.name);
        assert_eq!(
            d.input_schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "{}: schema.type should be 'object'",
            d.name
        );
        assert!(
            d.input_schema.get("properties").is_some(),
            "{}: missing properties",
            d.name
        );
    }
}

#[test]
fn the_catalog_holds_the_connector_tools_and_one_findings_report() {
    let c = ToolCatalog::builtin();
    assert!(c.descriptor("scm.prs.get").is_some());
    assert!(c.descriptor("issues.comment").is_some());
    assert!(c.descriptor("github.workflows_dispatch").is_some());
    // `findings.*` are the catalog's own tools; `findings.record` is an
    // alias of `findings.report`, not a second tool.
    assert_eq!(
        c.descriptors()
            .filter(|d| d.name == "findings.query")
            .count(),
        1
    );
    assert_eq!(
        c.resolve_name("findings.record", AliasScope::Everywhere)
            .unwrap()
            .name,
        "findings.report"
    );
    assert_eq!(c.descriptors().count(), ToolCatalog::all().len());
}
