#![cfg(test)]
//! Pure call-binding tests: no provider or actual tool dispatch.
use super::{approval_fingerprint, approval_request};
use async_trait::async_trait;
use rupu_tools::approval::*;
use rupu_tools::{PermissionMode, PermissionPolicy, ResolvedGrant, ToolContext, ToolDefinition};
use serde_json::json;
use std::sync::Arc;

struct Authorizer;
#[async_trait]
impl CallAuthorizer for Authorizer {
    fn root_owner(&self) -> ApprovalOwner {
        ApprovalOwner {
            session_id: "ses_fixture".into(),
            root_turn_run_id: "run_fixture".into(),
            run_id: "run_fixture".into(),
            root_run_id: "run_fixture".into(),
            parent_run_id: None,
            workspace_id: "ws_fixture".into(),
            workspace_path: "/fixture/project".into(),
            settings_revision: 5,
            orchestration_revision: 3,
            host_mcp_registry_fingerprint: Some("sha256:fixture".into()),
        }
    }
    async fn check_owner(&self, _: &ApprovalOwner) -> Result<(), ApprovalError> {
        panic!("pure fixture")
    }
    async fn authorize(
        &self,
        _: CallApprovalRequest,
        _: &mut ApprovalObserver<'_>,
    ) -> Result<ApprovalOutcome, ApprovalError> {
        panic!("pure fixture")
    }
}
fn context() -> ToolContext {
    let mut ctx = ToolContext::default();
    ctx.workspace.id = "ws_fixture".into();
    ctx.workspace.path = "/fixture/project".into();
    Arc::make_mut(&mut ctx.identity).run_id = "run_fixture".into();
    ctx
}
fn definition(effect: rupu_tools::Effect) -> Arc<ToolDefinition> {
    ToolDefinition::remote(
        "mcp.fixture".into(),
        rupu_tools::RemoteToolIdentity {
            server: "fixture_server".into(),
            tool: "fixture_tool".into(),
        },
        "Fixture tool".into(),
        json!({"type":"object"}),
        effect,
    )
    .unwrap()
}
#[test]
fn approval_binds_exact_owner_call_and_remote_without_expanding_grants() {
    let grant = ResolvedGrant::default();
    let request = approval_request(
        &Authorizer,
        &context(),
        &grant,
        &definition(rupu_tools::Effect::External),
        &json!({"query":"fixture"}),
        "call_fixture",
        "mcp.fixture",
        7,
        2,
    )
    .unwrap();
    assert_eq!(request.owner, Authorizer.root_owner());
    assert_eq!((request.turn_index, request.call_sequence), (7, 2));
    assert_eq!(request.tool_call_id, "call_fixture");
    assert_eq!(
        request.remote.unwrap(),
        ApprovalRemote {
            server_id: "fixture_server".into(),
            tool: "fixture_tool".into()
        }
    );
    assert_eq!(request.grant_fingerprint, approval_fingerprint(&json!([])));
}
#[test]
fn approval_rejects_workspace_drift_read_effect_and_oversized_input() {
    let mut ctx = context();
    ctx.workspace.id = "ws_other".into();
    let call = |ctx: &ToolContext, effect, input: &serde_json::Value| {
        approval_request(
            &Authorizer,
            ctx,
            &ResolvedGrant::default(),
            &definition(effect),
            input,
            "call_fixture",
            "mcp.fixture",
            0,
            0,
        )
    };
    assert!(call(&ctx, rupu_tools::Effect::Write, &json!({})).is_err());
    assert!(call(&context(), rupu_tools::Effect::Read, &json!({})).is_err());
    assert!(call(
        &context(),
        rupu_tools::Effect::Write,
        &json!({"content":"x".repeat(256 * 1024)})
    )
    .is_err());
}
#[test]
fn canonical_fingerprints_preserve_array_order_and_ignore_object_order() {
    assert_eq!(
        approval_fingerprint(&json!({"a":1,"b":{"c":2,"d":3}})),
        approval_fingerprint(
            &serde_json::from_str::<serde_json::Value>(r#"{"b":{"d":3,"c":2},"a":1}"#).unwrap()
        )
    );
    assert_ne!(
        approval_fingerprint(&json!([1, 2])),
        approval_fingerprint(&json!([2, 1]))
    );
}
#[test]
fn asynchronous_authorizer_never_becomes_synchronous_allow_always() {
    use rupu_tools::permission::{AllowAlways, Decision};
    let policy = PermissionPolicy::unattended(PermissionMode::Ask)
        .with_authorizer(Some(Arc::new(Authorizer)));
    let mut always = AllowAlways::default();
    assert!(policy.has_approval_channel());
    assert!(matches!(
        policy.decide_call(
            "fixture",
            rupu_tools::Effect::Write,
            &json!({}),
            &mut always
        ),
        Decision::Deny { .. }
    ));
    assert!(matches!(
        policy.decide_call("fixture", rupu_tools::Effect::Read, &json!({}), &mut always),
        Decision::Allow
    ));
    let readonly = PermissionPolicy::unattended(PermissionMode::Readonly)
        .with_authorizer(Some(Arc::new(Authorizer)));
    assert!(matches!(
        readonly.decide_call(
            "fixture",
            rupu_tools::Effect::External,
            &json!({}),
            &mut always
        ),
        Decision::Deny { .. }
    ));
}

struct CancelOnPermit {
    owner: ApprovalOwner,
    cancel: tokio_util::sync::CancellationToken,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}
struct UnusedPermit(Arc<std::sync::atomic::AtomicBool>);
impl Drop for UnusedPermit {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
#[async_trait]
impl ApprovalPermit for UnusedPermit {
    fn approval_id(&self) -> &str {
        "approval_fixture"
    }
    fn intent_fingerprint(&self) -> &str {
        "intent_fixture"
    }
    async fn complete(self: Box<Self>, _: ApprovalCompletion) -> Result<(), ApprovalError> {
        panic!("cancelled permit must never dispatch or complete")
    }
}
#[async_trait]
impl CallAuthorizer for CancelOnPermit {
    fn root_owner(&self) -> ApprovalOwner {
        self.owner.clone()
    }
    async fn check_owner(&self, _: &ApprovalOwner) -> Result<(), ApprovalError> {
        Ok(())
    }
    async fn authorize(
        &self,
        _: CallApprovalRequest,
        _: &mut ApprovalObserver<'_>,
    ) -> Result<ApprovalOutcome, ApprovalError> {
        // Becomes cancelled in the very poll returning a ready reservation:
        // either select order must still hit the post-permit dispatch guard.
        self.cancel.cancel();
        Ok(ApprovalOutcome::Permitted(Box::new(UnusedPermit(
            self.dropped.clone(),
        ))))
    }
}
#[tokio::test]
async fn ready_permit_that_cancels_the_run_never_invokes_write_handler() {
    use super::{run_agent, AgentRunOpts, MockProvider, ScriptedTurn};
    use rupu_providers::types::StopReason;
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("must-not-exist.txt");
    let pause = tokio_util::sync::CancellationToken::new();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut ctx = context();
    ctx.workspace.path = tmp.path().into();
    let mut owner = Authorizer.root_owner();
    owner.workspace_path = tmp.path().to_string_lossy().into_owned();
    let authorizer = Arc::new(CancelOnPermit {
        owner,
        cancel: pause.clone(),
        dropped: dropped.clone(),
    });
    let opts = crate::grant::with_grant(
        AgentRunOpts {
            system_prompt: "fixture".into(),
            prompt: crate::UserTurn::new("fixture"),
            provider: Box::new(MockProvider::new(vec![ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_fixture".into(),
                tool_name: "write_file".into(),
                tool_input: json!({"path":target,"content":"must not write"}),
                stop: StopReason::ToolUse,
            }])),
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            recovery: Default::default(),
            permission: PermissionPolicy::unattended(PermissionMode::Ask)
                .with_authorizer(Some(authorizer)),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: ctx,
            pins: Default::default(),
            concerns: None,
            max_turns: 2,
            stream: crate::StreamOpts {
                no_stream: true,
                suppress_stdout: true,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: Some(pause),
            collectors: Vec::new(),
            transcript_path: tmp.path().join("fixture.jsonl"),
        },
        Some(&["write_file".into()]),
        &[],
    )
    .unwrap();
    let result = run_agent(opts).await;
    assert!(matches!(result, Err(super::RunError::Approval(_))));
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        !target.exists(),
        "approval cancellation must precede actual handler dispatch"
    );
}
