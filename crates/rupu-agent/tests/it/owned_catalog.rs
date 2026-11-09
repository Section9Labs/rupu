//! Owned tools take the runner's existing grant -> policy -> body -> audit path.
use async_trait::async_trait;
use rupu_agent::runner::{CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_providers::{types::StopReason, ToolDefinition as ProviderTool};
use rupu_tools::{
    BoundTool, Effect, PermissionMode, PermissionPolicy, RemoteToolIdentity, ToolCatalog,
    ToolContext, ToolDefinition, ToolError, ToolInvoker, ToolOutput,
};
use rupu_transcript::{Event, JsonlReader};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Invocation = (String, String, Option<String>, Value);
#[derive(Default)]
struct Capture(Mutex<Vec<Invocation>>);
#[async_trait]
impl ToolInvoker for Capture {
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.0.lock().unwrap().push((
            ctx.identity.run_id.clone(),
            ctx.workspace.id.clone(),
            ctx.call.tool_call_id.clone(),
            input,
        ));
        Ok(ToolOutput {
            stdout: "fixture result".into(),
            error: None,
            duration_ms: 0,
            derived: None,
            structured: Some(json!({"fixture":true})),
        })
    }
}
fn catalog(effect: Effect, body: Arc<Capture>) -> ToolCatalog {
    ToolCatalog::builtin()
        .with_owned(
            BoundTool::owned(
                ToolDefinition::remote(
                    "mcp.fixture".into(),
                    RemoteToolIdentity {
                        server: "fixture-server".into(),
                        tool: "query".into(),
                    },
                    "original fixture".into(),
                    json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    effect,
                )
                .unwrap(),
                body,
            )
            .unwrap(),
        )
        .unwrap()
}
async fn run(
    catalog: ToolCatalog,
    declared: &[&str],
    mode: PermissionMode,
    replacement: Option<ToolCatalog>,
) -> (Option<String>, Vec<ProviderTool>, Vec<Event>) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.path().join("run.jsonl");
    let provider = CapturingMockProvider::new(vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "call-fixture".into(),
            tool_name: "mcp.fixture".into(),
            tool_input: json!({"query":"test"}),
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantText {
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]);
    let captured = provider.captured.clone();
    let mut context = ToolContext::in_workspace(tmp.path());
    context.identity_mut().agent = "fixture".into();
    context.identity_mut().run_id = "run_fixture".into();
    context.identity_mut().provider = "mock".into();
    context.identity_mut().model = "mock-model".into();
    context.workspace.id = "ws_fixture".into();
    context.services.tool_catalog = catalog;
    let names: Vec<String> = declared.iter().map(|s| s.to_string()).collect();
    let mut opts = rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "fixture".into(),
            prompt: rupu_agent::UserTurn::new("fixture"),
            provider: Box::new(provider),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: PermissionPolicy::unattended(mode),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: context,
            pins: Default::default(),
            concerns: None,
            max_turns: 3,
            stream: rupu_agent::StreamOpts {
                no_stream: true,
                suppress_stdout: true,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: None,
            collectors: Vec::new(),
            transcript_path: path.clone(),
        },
        Some(&names),
        &[],
    )
    .unwrap();
    if let Some(replacement) = replacement {
        opts.tool_context.services.tool_catalog = replacement;
    }
    let error = run_agent(opts).await.err().map(|e| e.to_string());
    let defs = captured
        .lock()
        .unwrap()
        .first()
        .map(|r| r.tools.clone())
        .unwrap_or_default();
    let events = if path.exists() {
        JsonlReader::iter(&path)
            .unwrap()
            .map(Result::unwrap)
            .collect()
    } else {
        Vec::new()
    };
    (error, defs, events)
}
fn audit(events: &[Event], decision: &str) -> bool {
    events.iter().any(|e| {
        matches!(e, Event::ToolAudit { tool, decision: Some(actual), .. }
        if tool == "mcp.fixture" && actual == decision)
    })
}

#[tokio::test]
async fn owned_read_tool_keeps_snapshot_body_schema_identity_and_call_context() {
    let original = Arc::new(Capture::default());
    let later = Arc::new(Capture::default());
    let (error, defs, events) = run(
        catalog(Effect::Read, original.clone()),
        &["mcp.fixture"],
        PermissionMode::Readonly,
        Some(catalog(Effect::External, later.clone())),
    )
    .await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "mcp.fixture");
    assert_eq!(defs[0].description, "original fixture");
    assert_eq!(
        defs[0].input_schema["properties"]["query"]["type"],
        "string"
    );
    assert_eq!(
        original.0.lock().unwrap().as_slice(),
        &[(
            "run_fixture".into(),
            "ws_fixture".into(),
            Some("call-fixture".into()),
            json!({"query":"test"})
        )]
    );
    assert!(
        later.0.lock().unwrap().is_empty(),
        "dispatch must use grant's snapshot"
    );
    assert!(events.iter().any(|e| matches!(e, Event::ToolGrant { entries, .. }
        if entries.iter().any(|e| e.remote.as_ref().is_some_and(|r| r.server == "fixture-server" && r.tool == "query")))));
    assert!(events.iter().any(|e| matches!(e, Event::ToolResult { structured: Some(value), .. } if value["fixture"] == true)));
    assert!(audit(&events, "allowed"));
}

#[tokio::test]
async fn readonly_external_tool_is_audited_without_invoking_body() {
    let body = Arc::new(Capture::default());
    let (error, defs, events) = run(
        catalog(Effect::External, body.clone()),
        &["mcp.fixture"],
        PermissionMode::Readonly,
        None,
    )
    .await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(defs.len(), 1);
    assert!(body.0.lock().unwrap().is_empty());
    assert!(audit(&events, "denied:readonly"));
    assert!(events.iter().any(|e| matches!(e, Event::ToolResult { error: Some(error), .. } if error == "permission_denied")));
}

#[tokio::test]
async fn bypass_does_not_admit_an_ungranted_owned_tool() {
    let body = Arc::new(Capture::default());
    let (error, defs, events) = run(
        catalog(Effect::Read, body.clone()),
        &[],
        PermissionMode::Bypass,
        None,
    )
    .await;
    assert!(error.is_none(), "{error:?}");
    assert!(defs.is_empty());
    assert!(body.0.lock().unwrap().is_empty());
    assert!(audit(&events, "not_granted"));
}

#[tokio::test]
async fn granted_static_extension_without_body_still_fails_before_provider_work() {
    static UNIMPLEMENTED: rupu_tools::ToolDescriptor = rupu_tools::ToolDescriptor {
        name: "fixture.unimplemented",
        aliases: &[],
        effect: Effect::Read,
        needs: &[],
        uses: &[],
        wildcard: true,
        description: "fixture",
        input_schema: || json!({"type":"object"}),
    };
    let (error, defs, _) = run(
        ToolCatalog::builtin().with([&UNIMPLEMENTED]),
        &["fixture.unimplemented"],
        PermissionMode::Readonly,
        None,
    )
    .await;
    assert!(error.unwrap().contains("no implementation"));
    assert!(defs.is_empty());
}
