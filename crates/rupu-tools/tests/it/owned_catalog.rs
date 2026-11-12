use async_trait::async_trait;
use rupu_tools::{
    BoundTool, CatalogError, Effect, GrantInputs, RemoteToolIdentity, ResolvedGrant, ToolCatalog,
    ToolContext, ToolDefinition, ToolError, ToolInvoker, ToolOutput,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Counter(AtomicUsize);
#[async_trait]
impl ToolInvoker for Counter {
    async fn invoke(&self, _: Value, _: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput {
            stdout: "fixture".into(),
            error: None,
            duration_ms: 0,
            derived: None,
            structured: None,
        })
    }
}
fn tool(name: &str, server: &str, remote: &str, effect: Effect, body: Arc<Counter>) -> BoundTool {
    BoundTool::owned(
        ToolDefinition::remote(
            name.into(),
            RemoteToolIdentity {
                server: server.to_owned(),
                tool: remote.to_owned(),
            },
            "fixture".into(),
            json!({"type":"object","properties":{}}),
            effect,
        )
        .unwrap(),
        body,
    )
    .unwrap()
}
fn grant(catalog: &ToolCatalog, declared: Option<&[&str]>) -> ResolvedGrant {
    let names = declared.map(|items| items.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    catalog
        .resolve_grant(GrantInputs {
            declared: names.as_deref(),
            step_actions: &[],
            connector_cap: None,
            ambient: &[],
            available: &Default::default(),
            alias_scope: Default::default(),
        })
        .unwrap()
}

#[tokio::test]
async fn grant_owns_descriptor_and_body_after_discovery_owner_is_dropped() {
    let body = Arc::new(Counter(AtomicUsize::new(0)));
    let weak = Arc::downgrade(&body);
    let resolved = {
        let name = String::from("mcp.fixture");
        let server = String::from("server-a");
        let catalog = ToolCatalog::builtin()
            .with_owned(tool(&name, &server, "query", Effect::Read, body))
            .unwrap();
        grant(&catalog, Some(&[&name]))
    };
    let descriptor = &resolved.entry("mcp.fixture").unwrap().descriptor;
    assert_eq!(descriptor.remote_identity().unwrap().server, "server-a");
    let bound = resolved
        .catalog
        .body("mcp.fixture", &ToolContext::default())
        .unwrap();
    assert!(Arc::ptr_eq(descriptor, bound.definition()));
    bound
        .invoke(json!({}), &ToolContext::default())
        .await
        .unwrap();
    assert_eq!(weak.upgrade().unwrap().0.load(Ordering::SeqCst), 1);
    drop(bound);
    drop(resolved);
    assert!(
        weak.upgrade().is_none(),
        "no leaked static body/metadata owner"
    );
}

#[test]
fn duplicate_names_and_remote_identities_cannot_shadow_an_existing_binding() {
    let body = Arc::new(Counter(AtomicUsize::new(0)));
    let catalog = ToolCatalog::builtin()
        .with_owned(tool("mcp.a", "a", "query", Effect::Read, body.clone()))
        .unwrap();
    assert!(matches!(
        catalog
            .clone()
            .with_owned(tool("mcp.a", "b", "other", Effect::External, body.clone())),
        Err(CatalogError::DuplicateName(_))
    ));
    assert!(matches!(
        catalog.with_owned(tool("mcp.b", "a", "query", Effect::External, body)),
        Err(CatalogError::DuplicateIdentity { .. })
    ));
}

#[test]
fn same_remote_name_on_two_servers_preserves_distinct_identity_and_effect() {
    let body = Arc::new(Counter(AtomicUsize::new(0)));
    let catalog = ToolCatalog::builtin()
        .with_owned(tool("mcp.a", "a", "query", Effect::Read, body.clone()))
        .unwrap()
        .with_owned(tool("mcp.b", "b", "query", Effect::External, body))
        .unwrap();
    let resolved = grant(&catalog, Some(&["mcp.a", "mcp.b"]));
    let a = &resolved.entry("mcp.a").unwrap().descriptor;
    let b = &resolved.entry("mcp.b").unwrap().descriptor;
    assert_ne!(a.remote_identity(), b.remote_identity());
    assert_eq!(a.effect, Effect::Read);
    assert_eq!(b.effect, Effect::External);
    assert_eq!(
        resolved.entry("mcp.a").unwrap().reasons_string(),
        "declared"
    );
}

#[test]
fn owned_catalog_does_not_expand_defaults_empty_declarations_or_global_star() {
    let catalog = ToolCatalog::builtin()
        .with_owned(tool(
            "mcp.a",
            "a",
            "query",
            Effect::Read,
            Arc::new(Counter(AtomicUsize::new(0))),
        ))
        .unwrap();
    assert_eq!(
        grant(&catalog, None).entries.keys().collect::<Vec<_>>(),
        grant(&ToolCatalog::builtin(), None)
            .entries
            .keys()
            .collect::<Vec<_>>()
    );
    assert!(grant(&catalog, Some(&[])).entries.is_empty());
    let star = grant(&catalog, Some(&["*"]));
    assert!(!star.offers("mcp.a"));
    assert!(!star.offers("board.directive"));
    assert!(star.offers("read_file"));
    assert!(grant(&catalog, Some(&["mcp.*"])).offers("mcp.a"));
    assert!(catalog.validate_names(&["mcp.a".into()]).is_ok());
    assert!(ToolCatalog::builtin()
        .validate_names(&["mcp.a".into()])
        .is_err());
}

#[test]
fn remote_definitions_cannot_claim_bookkeeping_spawn_or_ambiguous_provider_names() {
    for (name, effect) in [
        ("read_file", Effect::Read),
        ("mcp.a__dot__b", Effect::Read),
        ("mcp.a", Effect::Record),
        ("mcp.a", Effect::Spawn),
    ] {
        assert!(ToolDefinition::remote(
            name.into(),
            RemoteToolIdentity {
                server: "a".into(),
                tool: "query".into()
            },
            "fixture".into(),
            json!({"type":"object"}),
            effect
        )
        .is_err());
    }
}

#[test]
fn allow_always_owns_only_the_exact_selected_name() {
    let mut allowed = rupu_tools::AllowAlways::default();
    {
        let temporary = String::from("mcp.a");
        allowed.insert(&temporary);
    }
    assert!(allowed.contains("mcp.a"));
    assert!(!allowed.contains("mcp.b"));
    let decision = rupu_tools::PermissionPolicy::unattended(rupu_tools::PermissionMode::Readonly)
        .decide_call("mcp.a", Effect::External, &json!({}), &mut allowed);
    assert!(
        matches!(decision, rupu_tools::Decision::Deny { .. }),
        "allow always cannot change the mode"
    );
}

#[test]
fn static_extension_cannot_shadow_owned_identity_with_a_scoped_alias() {
    static ALIAS: rupu_tools::ToolDescriptor = rupu_tools::ToolDescriptor {
        name: "fixture.alias",
        aliases: &[rupu_tools::Alias::flow_lead("mcp.a")],
        effect: Effect::External,
        needs: &[],
        uses: &[],
        wildcard: true,
        description: "fixture",
        input_schema: || json!({"type":"object"}),
    };
    let catalog = ToolCatalog::builtin()
        .with_owned(tool(
            "mcp.a",
            "a",
            "query",
            Effect::Read,
            Arc::new(Counter(AtomicUsize::new(0))),
        ))
        .unwrap()
        .with([&ALIAS]);
    let definition = catalog
        .resolve_definition("mcp.a", rupu_tools::AliasScope::FlowLead)
        .unwrap();
    assert_eq!(definition.remote_identity().unwrap().server, "a");
    assert!(catalog.definition("fixture.alias").is_none());
}
