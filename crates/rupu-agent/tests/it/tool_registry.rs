use rupu_agent::ToolRegistry;
use rupu_tools::{AliasScope, Tool, ToolCatalog, ToolContext};
use std::sync::Arc;

/// The catalog body of `name` in a context that can launch children.
fn builtin_tool(name: &str) -> Option<Arc<dyn Tool>> {
    let mut ctx = ToolContext::default();
    ctx.services.launcher = Some(Arc::new(NoLaunch));
    rupu_tools::bodies::body(name, &ctx)
}

struct NoLaunch;

#[async_trait::async_trait]
impl rupu_tools::Launcher for NoLaunch {
    async fn check(
        &self,
        _req: &rupu_tools::ChildRequest,
        _from: &ToolContext,
    ) -> Result<(), rupu_tools::LaunchError> {
        unreachable!("never called")
    }
    async fn launch(
        &self,
        _req: rupu_tools::ChildRequest,
        _from: &ToolContext,
    ) -> Result<rupu_tools::ChildHandle, rupu_tools::LaunchError> {
        unreachable!("never called")
    }
    async fn join(
        &self,
        _from: &rupu_tools::RunIdentity,
        _which: rupu_tools::JoinSet,
        _timeout: std::time::Duration,
        _until: rupu_tools::Until,
    ) -> Vec<rupu_tools::ChildStatus> {
        unreachable!("never called")
    }
    async fn finish(&self, _from: &rupu_tools::RunIdentity, _how: rupu_tools::ParentEnd) {}
    fn children(&self, _from: &rupu_tools::RunIdentity) -> Vec<rupu_tools::ChildStatus> {
        Vec::new()
    }
}

const BUILTINS: [&str; 9] = [
    "ast_grep",
    "bash",
    "dispatch",
    "edit_file",
    "glob",
    "grep",
    "join",
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
