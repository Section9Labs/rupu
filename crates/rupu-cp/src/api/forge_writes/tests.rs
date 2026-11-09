use super::*;
use crate::{
    forge_writer::ForgeWriter,
    repos::{RepoEntry, RepoListError, RepoLister},
};
use rupu_scm::forge_write::{ForgeCapability, ForgeOwner, PreparedMutation};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Spy {
    calls: Arc<AtomicUsize>,
    changed: AtomicBool,
    unknown: bool,
    pause: std::sync::Mutex<Option<Arc<Pause>>>,
}
struct Pause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
struct Ready {
    calls: Arc<AtomicUsize>,
    unknown: bool,
    pause: Option<Arc<Pause>>,
}
#[async_trait::async_trait]
impl PreparedMutation for Ready {
    async fn dispatch(self: Box<Self>) -> DispatchResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(pause) = &self.pause {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
        if self.unknown {
            DispatchResult::unknown()
        } else {
            DispatchResult {
                outcome: DispatchOutcome::Accepted,
                reason: "provider_acknowledged".into(),
                result: json!({"object_id":"99"}),
            }
        }
    }
}
#[async_trait::async_trait]
impl ForgeWriter for Spy {
    fn capabilities(&self) -> Vec<ForgeCapability> {
        vec![]
    }
    async fn prepare(
        &self,
        request: &ForgeRequest,
        _: Option<&ForgeOwner>,
    ) -> std::result::Result<PreparedWrite, ForgeError> {
        Ok(PreparedWrite {
            request: request.clone(),
            authority: json!({"provider":"github","account":"work","api_origin":"https://api.github.com","provider_project_id":"12","provider_object_id":"7"}),
            observed: json!({"version":"v1"}),
            preconditions: json!({"version":"v1"}),
            limitations: vec!["No provider CAS".into()],
            generation: "epoch1".into(),
        })
    }
    async fn preflight(
        &self,
        _: &PreparedWrite,
        _: Option<&ForgeOwner>,
    ) -> std::result::Result<Box<dyn PreparedMutation>, ForgeError> {
        if self.changed.load(Ordering::SeqCst) {
            return Err(ForgeError::changed());
        }
        Ok(Box::new(Ready {
            calls: self.calls.clone(),
            unknown: self.unknown,
            pause: self.pause.lock().unwrap().clone(),
        }))
    }
}
#[async_trait::async_trait]
impl RepoLister for Spy {
    fn forge_writer(&self) -> Option<&dyn ForgeWriter> {
        Some(self)
    }
    async fn list(&self) -> std::result::Result<Vec<RepoEntry>, RepoListError> {
        Ok(vec![])
    }
}
fn request() -> ForgeRequest {
    ForgeRequest {
        action: ForgeAction::IssueComment,
        target: ForgeTarget {
            provider: rupu_scm::IssueTracker::Github,
            account: "work".into(),
            api_origin: "https://api.github.com".into(),
            project_ref: "org/repo".into(),
            number: Some("7".into()),
            ws_id: None,
        },
        expected: BTreeMap::from([("version".into(), "v1".into())]),
        input: ForgeInput {
            body: Some("Reviewed comment".into()),
            ..Default::default()
        },
    }
}
async fn fixture(unknown: bool) -> (tempfile::TempDir, AppState, Arc<Spy>, journal::Intent) {
    let home = tempfile::tempdir().unwrap();
    let spy = Arc::new(Spy {
        calls: Arc::new(AtomicUsize::new(0)),
        changed: AtomicBool::new(false),
        unknown,
        pause: std::sync::Mutex::new(None),
    });
    let state = AppState::new(home.path().into(), rupu_config::PricingConfig::default())
        .with_repos(Some(spy.clone()));
    let mut prepared = spy.prepare(&request(), None).await.unwrap();
    prepared.authority["instance_id"] = json!(instance_id(&state).unwrap());
    let key = dedupe_key(&prepared).unwrap();
    let payload = serde_json::to_value(prepared).unwrap();
    let digest = hash(&payload).unwrap();
    let store = journal::Store::open(home.path()).unwrap();
    let intent = store
        .publish_intent(
            "intent1",
            &digest,
            &key,
            chrono::Utc::now().timestamp() as u64 + 600,
            payload,
        )
        .unwrap();
    drop(store);
    (home, state, spy, intent)
}
fn execute_body(intent: &journal::Intent, id: &str) -> ExecuteBody {
    ExecuteBody {
        operation_id: id.into(),
        expected_digest: intent.digest.clone(),
        confirm: true,
    }
}

#[tokio::test]
async fn one_review_dispatches_once_and_exact_replay_only_reads_receipt() {
    let (_home, state, spy, intent) = fixture(false).await;
    let first = execute_job(
        state.clone(),
        intent.intent_id.clone(),
        execute_body(&intent, "operation1"),
    )
    .await
    .unwrap();
    assert_eq!(first["outcome"], "accepted");
    assert_eq!(first["durability"], "durable");
    let replay = execute_job(
        state.clone(),
        intent.intent_id.clone(),
        execute_body(&intent, "operation1"),
    )
    .await
    .unwrap();
    assert_eq!(replay["receipt_version"], first["receipt_version"]);
    assert!(execute_job(
        state,
        intent.intent_id.clone(),
        execute_body(&intent, "other-operation")
    )
    .await
    .is_err());
    assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn changed_preflight_is_durable_not_submitted() {
    let (_home, state, spy, intent) = fixture(false).await;
    spy.changed.store(true, Ordering::SeqCst);
    let value = execute_job(
        state,
        intent.intent_id.clone(),
        execute_body(&intent, "operation1"),
    )
    .await
    .unwrap();
    assert_eq!(value["outcome"], "not_submitted");
    assert_eq!(value["durability"], "durable");
    assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn uncertain_dispatch_and_new_id_cannot_silently_repeat() {
    let (home, state, spy, intent) = fixture(true).await;
    let value = execute_job(
        state.clone(),
        intent.intent_id.clone(),
        execute_body(&intent, "operation1"),
    )
    .await
    .unwrap();
    assert_eq!(value["outcome"], "unknown");
    let replay = execute_job(
        state,
        intent.intent_id.clone(),
        execute_body(&intent, "operation1"),
    )
    .await
    .unwrap();
    assert_eq!(replay["outcome"], "unknown");
    assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
    let store = journal::Store::open(home.path()).unwrap();
    assert!(store
        .publish_intent(
            "new-intent",
            &intent.digest,
            &intent.key,
            intent.expires_at,
            intent.payload
        )
        .is_err());
}
#[tokio::test]
async fn dedupe_uses_immutable_provider_identity_not_workspace_alias_or_generation() {
    let (_home, _state, spy, _) = fixture(false).await;
    let first = spy.prepare(&request(), None).await.unwrap();
    let mut alias = first.clone();
    alias.request.target.project_ref = "display-alias".into();
    alias.generation = "new-epoch".into();
    alias.authority["project_ref"] = json!("different-display");
    alias.authority["ws_id"] = json!("another-root");
    assert_eq!(dedupe_key(&first).unwrap(), dedupe_key(&alias).unwrap());
    alias.authority["provider_object_id"] = json!("8");
    assert_ne!(dedupe_key(&first).unwrap(), dedupe_key(&alias).unwrap());
}
#[test]
fn nonlocal_host_and_implicit_issue_workspace_are_refused() {
    assert!(local(Some("remote")).is_err());
    let mut r = request();
    r.target.ws_id = Some("selected-project".into());
    assert!(request_owner_id(&r).is_err());
    r.action = ForgeAction::ReviewComment;
    r.target.ws_id = None;
    assert!(request_owner_id(&r).is_err());
}

#[tokio::test]
async fn cancelled_waiting_http_task_keeps_single_dispatch_and_receipt_writer_alive() {
    let (home, state, spy, intent) = fixture(false).await;
    let pause = Arc::new(Pause {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    *spy.pause.lock().unwrap() = Some(pause.clone());
    let body = execute_body(&intent, "operation1");
    let waiting = tokio::spawn(execute(
        State(state),
        Path(intent.intent_id),
        Query(LocalQuery::default()),
        Json(body),
    ));
    tokio::time::timeout(Duration::from_secs(2), pause.entered.notified())
        .await
        .unwrap();
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    pause.release.notify_one();
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let reader = journal::Reader::open(home.path()).unwrap();
            if let Some(status) = reader.operation("operation1").unwrap() {
                if !status.writer_active && status.operation.outcome == Some(Outcome::Accepted) {
                    break status.operation;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(observed.phase, Phase::Terminal);
    assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
    let store = journal::Store::open(home.path()).unwrap();
    assert_eq!(
        store.operation("operation1").unwrap().unwrap().outcome,
        Some(Outcome::Accepted)
    );
}

#[test]
fn provider_input_and_availability_failures_have_explicit_http_classes() {
    for (code, status) in [
        ("unsupported_quick_action", StatusCode::BAD_REQUEST),
        ("account_unavailable", StatusCode::SERVICE_UNAVAILABLE),
        ("choices_incomplete", StatusCode::SERVICE_UNAVAILABLE),
        ("labels_unavailable", StatusCode::CONFLICT),
    ] {
        let result = provider_error(ForgeError::fixed(code, "fixture reason"));
        assert_eq!(result.0, status);
        assert_eq!(result.1["code"], code);
    }
}

#[test]
fn branch_creation_requires_exact_registered_owner_and_dedupe_retains_name() {
    let mut req = request();
    req.action = ForgeAction::BranchCreate;
    req.target.number = None;
    assert!(request_owner_id(&req).is_err());
    req.target.ws_id = Some("workspace".into());
    assert_eq!(request_owner_id(&req).unwrap(), Some("workspace"));
    req.input = rupu_scm::forge_write::ForgeInput {
        head: Some("new".into()),
        base: Some("main".into()),
        ..Default::default()
    };
    let mut prepared = PreparedWrite {
        request: req,
        authority: json!({"provider":"github","account":"work","api_origin":"https://api.github.com","provider_project_id":"12"}),
        observed: json!({}),
        preconditions: json!({}),
        limitations: vec![],
        generation: "1".into(),
    };
    let key = dedupe_key(&prepared).unwrap();
    prepared.request.input.head = Some("other".into());
    assert_ne!(key, dedupe_key(&prepared).unwrap());
    prepared.request.input.head = Some("new".into());
    prepared.request.target.ws_id = Some("alias".into());
    assert_eq!(key, dedupe_key(&prepared).unwrap());
    prepared.request.input.base = Some("different-base".into());
    assert_eq!(key, dedupe_key(&prepared).unwrap());
}

#[test]
fn workflow_dispatch_requires_owner_and_deduplicates_filename_and_numeric_aliases() {
    let mut req = request();
    req.action = ForgeAction::WorkflowDispatch; req.target.number = None;
    assert!(request_owner_id(&req).is_err());
    req.target.ws_id = Some("workspace".into());
    assert_eq!(request_owner_id(&req).unwrap(), Some("workspace"));
    req.input = rupu_scm::forge_write::ForgeInput {
        workflow: Some("build.yml".into()), ref_name: Some("main".into()), ref_kind: Some("branch".into()),
        workflow_inputs: std::collections::BTreeMap::from([("key".into(), "exact value".into())]), ..Default::default()
    };
    let mut prepared = PreparedWrite {
        request:req,authority:json!({"provider":"github","account":"work","api_origin":"https://api.github.com","provider_project_id":"12","provider_object_id":"42"}),
        observed:json!({"workflow_id":"42"}),preconditions:json!({}),limitations:vec![],generation:"1".into(),
    };
    let key=dedupe_key(&prepared).unwrap();
    prepared.request.input.workflow=Some("42".into());assert_eq!(dedupe_key(&prepared).unwrap(),key);
    prepared.request.target.ws_id=Some("alias".into());prepared.generation="new".into();assert_eq!(dedupe_key(&prepared).unwrap(),key);
    prepared.request.input.ref_name=Some("other".into());assert_ne!(dedupe_key(&prepared).unwrap(),key);
    prepared.request.input.ref_name=Some("main".into());prepared.request.input.workflow_inputs.insert("key".into(),"other value".into());assert_ne!(dedupe_key(&prepared).unwrap(),key);
    prepared.authority["provider_object_id"]=serde_json::Value::Null;assert!(dedupe_key(&prepared).is_err());
}
