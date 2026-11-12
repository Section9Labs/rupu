#![cfg(test)]
use super::*;
use httpmock::{Method::POST, MockServer};
use rupu_tools::{AliasScope, GrantInputs, PermissionMode, PermissionPolicy, ServiceSet};
use std::sync::atomic::{AtomicUsize, Ordering};

struct NoCredentials(AtomicUsize);
impl Credentials for NoCredentials {
    fn bearer(&self, _: &str) -> Result<String, rupu_mcp_client::Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(rupu_mcp_client::Error::Credential)
    }
}
fn credentials() -> NoCredentials {
    NoCredentials(AtomicUsize::new(0))
}
fn registry(server: &MockServer) -> Registry {
    Registry::parse(&format!("version=1\n[servers.fixture]\nlabel='Friendly fixture'\nurl='{}'\nenabled=true\n[servers.fixture.tool_effects]\nquery='read'", server.url("/mcp"))).unwrap()
}
fn metadata(name: &str) -> Value {
    json!({"name":name,"description":"Find fixture records","inputSchema":{"type":"object","properties":{"q":{"type":"string"}}}})
}
fn install(server: &MockServer, tools: Vec<Value>) -> httpmock::Mock<'_> {
    server.mock(|when,then| {
        when.method(POST).path("/mcp").json_body_partial(r#"{"method":"initialize"}"#);
        then.status(200).header("content-type","application/json").json_body(json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":rupu_mcp_client::PROTOCOL_VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}));
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/mcp")
            .json_body_partial(r#"{"method":"notifications/initialized"}"#);
        then.status(202);
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/mcp")
            .json_body_partial(r#"{"method":"tools/list"}"#);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({"jsonrpc":"2.0","id":2,"result":{"tools":tools}}));
    })
}
fn owner() -> Owner {
    Owner {
        run_id: "run_fixture".into(),
        workspace_id: "ws_fixture".into(),
        workspace_path: PathBuf::from("/fixture"),
    }
}
fn context() -> ToolContext {
    let mut ctx = ToolContext::in_workspace("/fixture");
    ctx.identity_mut().surface = Surface::Session;
    ctx.identity_mut().run_id = "run_fixture".into();
    ctx.workspace.id = "ws_fixture".into();
    ctx
}

#[tokio::test]
async fn enabled_host_default_freezes_friendly_exact_tools_and_empty_does_no_io() {
    let server = MockServer::start_async().await;
    install(&server, vec![metadata("query")]);
    let creds = credentials();
    let mut registry = registry(&server);
    let mut disabled = registry.servers["fixture"].clone();
    disabled.enabled = false;
    disabled.bearer_env = Some("RUPU_MCP_UNUSED".into());
    registry.servers.insert("disabled".into(), disabled);
    let catalog = discover(&registry, &creds).await.unwrap();
    assert_eq!(catalog.tools.len(), 1);
    assert_eq!(catalog.tools[0].server_label, "Friendly fixture");
    assert_eq!(catalog.tools[0].effect, ToolEffect::Read);
    let frozen = catalog.freeze(None, None).unwrap();
    assert_eq!(
        frozen.tools[0].identity,
        ToolRef {
            server: "fixture".into(),
            tool: "query".into()
        }
    );
    registry.version = 999; // Empty selection skips even registry validation.
    assert_eq!(
        freeze(&registry, &creds, Some(&[]), None).await.unwrap(),
        FrozenSelection::default()
    );
    assert_eq!(creds.0.load(Ordering::SeqCst), 0);
}

#[test]
fn explicit_choices_require_revision_and_reject_duplicates_and_unknowns() {
    let catalog = snapshot(vec![CatalogTool {
        server: "s".into(),
        server_label: "Friendly".into(),
        tool: "query".into(),
        name: "unused".into(),
        description: String::new(),
        effect: ToolEffect::External,
        fingerprint: digest(&json!("fixture")),
    }])
    .unwrap();
    let choice = ToolRef {
        server: "s".into(),
        tool: "query".into(),
    };
    assert!(matches!(
        catalog.freeze(Some(std::slice::from_ref(&choice)), None),
        Err(Error::Stale)
    ));
    assert!(matches!(
        catalog.freeze(Some(std::slice::from_ref(&choice)), Some("old")),
        Err(Error::Stale)
    ));
    assert!(catalog
        .freeze(Some(std::slice::from_ref(&choice)), Some(&catalog.revision))
        .is_ok());
    assert!(catalog
        .freeze(Some(&[choice.clone(), choice]), Some(&catalog.revision))
        .is_err());
    assert!(catalog
        .freeze(
            Some(&[ToolRef {
                server: "s".into(),
                tool: "invented".into()
            }]),
            Some(&catalog.revision)
        )
        .is_err());
    assert!(catalog.freeze(Some(&[]), None).unwrap().tools.is_empty());
}

#[test]
fn names_are_bounded_provider_safe_and_preserve_exact_remote_identity() {
    let identities = [
        ToolRef {
            server: "first".into(),
            tool: "search.records".into(),
        },
        ToolRef {
            server: "second".into(),
            tool: "search.records".into(),
        },
        ToolRef {
            server: "first".into(),
            tool: "查询 records/__dot__".into(),
        },
    ];
    let names: BTreeSet<_> = identities.iter().map(ToolRef::local_name).collect();
    assert_eq!(names.len(), identities.len());
    for identity in identities {
        ToolDefinition::remote(
            identity.local_name(),
            RemoteToolIdentity {
                server: identity.server,
                tool: identity.tool,
            },
            String::new(),
            json!({"type":"object"}),
            Effect::External,
        )
        .unwrap();
    }
}

#[test]
fn missing_and_explicit_empty_builtin_grants_keep_their_authority() {
    let selected = FrozenSelection {
        tools: vec![FrozenTool {
            identity: ToolRef {
                server: "s".into(),
                tool: "q".into(),
            },
            fingerprint: digest(&json!(1)),
        }],
    };
    let mut defaults = None;
    selected.append_to(&mut defaults);
    let mut expected: Vec<String> = rupu_tools::DEFAULT_GRANT
        .iter()
        .map(|s| (*s).into())
        .collect();
    expected.push(selected.tools[0].identity.local_name());
    assert_eq!(defaults, Some(expected));
    let mut empty = Some(vec![]);
    selected.append_to(&mut empty);
    assert_eq!(empty, Some(vec![selected.tools[0].identity.local_name()]));
    let mut untouched = None;
    FrozenSelection::default().append_to(&mut untouched);
    assert_eq!(untouched, None);
}

#[tokio::test]
async fn old_empty_selection_ignores_new_host_defaults_and_invalid_configuration() {
    let registry = Registry {
        version: 999,
        ..Registry::default()
    };
    let creds = credentials();
    let catalog = assemble(
        &registry,
        &creds,
        &FrozenSelection::default(),
        owner(),
        Arc::new(rupu_netflow::NullSink),
    )
    .await
    .unwrap();
    assert!(catalog.definitions().all(|d| d.remote_identity().is_none()));
    assert_eq!(creds.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn changed_policy_or_schema_refuses_before_invocation_and_new_tools_do_not_expand() {
    let server = MockServer::start_async().await;
    let mut listing = install(&server, vec![metadata("query")]);
    let mut registry = registry(&server);
    let creds = credentials();
    let frozen = freeze(&registry, &creds, None, None).await.unwrap();
    registry
        .servers
        .get_mut("fixture")
        .unwrap()
        .tool_effects
        .insert("query".into(), ToolEffect::External);
    assert!(matches!(
        assemble(
            &registry,
            &creds,
            &frozen,
            owner(),
            Arc::new(rupu_netflow::NullSink)
        )
        .await,
        Err(Error::Stale)
    ));
    registry
        .servers
        .get_mut("fixture")
        .unwrap()
        .tool_effects
        .insert("query".into(), ToolEffect::Read);
    listing.delete_async().await;
    let mut changed = metadata("query");
    changed["inputSchema"]["required"] = json!(["q"]);
    listing = install(&server, vec![changed]);
    assert!(matches!(
        assemble(
            &registry,
            &creds,
            &frozen,
            owner(),
            Arc::new(rupu_netflow::NullSink)
        )
        .await,
        Err(Error::Stale)
    ));
    listing.delete_async().await;
    install(&server, vec![metadata("query"), metadata("new_tool")]);
    let calls = server.mock(|when, then| {
        when.method(POST)
            .json_body_partial(r#"{"method":"tools/call"}"#);
        then.status(500);
    });
    let assembled = assemble(
        &registry,
        &creds,
        &frozen,
        owner(),
        Arc::new(rupu_netflow::NullSink),
    )
    .await
    .unwrap();
    assert_eq!(
        assembled
            .definitions()
            .filter(|d| d.remote_identity().is_some())
            .count(),
        1
    );
    calls.assert_hits(0);
}

#[tokio::test]
async fn bound_call_checks_run_workspace_surface_and_keeps_structured_remote_results() {
    let server = MockServer::start_async().await;
    install(&server, vec![metadata("query")]);
    let calls=server.mock(|when,then| {
        when.method(POST).json_body_partial(r#"{"method":"tools/call","params":{"name":"query"}}"#);
        then.status(200).header("content-type","application/json").json_body(json!({"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"fixture result"}],"structuredContent":{"answer":42},"isError":true}}));
    });
    let registry = registry(&server);
    let creds = credentials();
    let frozen = freeze(&registry, &creds, None, None).await.unwrap();
    let assembled = assemble(
        &registry,
        &creds,
        &frozen,
        owner(),
        Arc::new(rupu_netflow::NullSink),
    )
    .await
    .unwrap();
    let ctx = context();
    let binding = assembled
        .body(&frozen.tools[0].identity.local_name(), &ctx)
        .unwrap();
    for change in 0..4 {
        let mut wrong = ctx.clone();
        match change {
            0 => wrong.identity_mut().run_id = "other".into(),
            1 => wrong.workspace.id = "other".into(),
            2 => wrong.workspace.path = PathBuf::from("/other"),
            _ => wrong.identity_mut().surface = Surface::Agent,
        }
        assert!(matches!(
            binding.invoke(json!({}), &wrong).await,
            Err(ToolError::PermissionDenied)
        ));
    }
    calls.assert_hits(0);
    let result = binding.invoke(json!({"q":"fixture"}), &ctx).await.unwrap();
    assert!(result.error.is_some());
    assert_eq!(
        result.structured.unwrap()["structuredContent"]["answer"],
        42
    );
    calls.assert_hits(1);
}

#[tokio::test]
async fn builtin_wildcard_never_implicitly_grants_remote_and_readonly_denies_external() {
    let server = MockServer::start_async().await;
    install(&server, vec![metadata("external")]);
    let calls = server.mock(|when, then| {
        when.method(POST)
            .json_body_partial(r#"{"method":"tools/call"}"#);
        then.status(500);
    });
    let registry = registry(&server);
    let creds = credentials();
    let frozen = freeze(&registry, &creds, None, None).await.unwrap();
    let catalog = assemble(
        &registry,
        &creds,
        &frozen,
        owner(),
        Arc::new(rupu_netflow::NullSink),
    )
    .await
    .unwrap();
    let available = ServiceSet::default();
    let names = vec!["*".into()];
    let grant = catalog
        .resolve_grant(GrantInputs {
            declared: Some(&names),
            step_actions: &[],
            connector_cap: None,
            ambient: &[],
            available: &available,
            alias_scope: AliasScope::Everywhere,
        })
        .unwrap();
    assert!(grant
        .entries
        .values()
        .all(|e| e.descriptor.remote_identity().is_none()));
    let descriptor = catalog
        .definition(&frozen.tools[0].identity.local_name())
        .unwrap();
    assert_eq!(descriptor.effect, Effect::External);
    let policy = PermissionPolicy::unattended(PermissionMode::Readonly);
    let decision = policy.decide_call(
        &descriptor.name,
        descriptor.effect,
        &json!({}),
        &mut Default::default(),
    );
    assert!(matches!(decision, rupu_tools::Decision::Deny { .. }));
    calls.assert_hits(0);
}
