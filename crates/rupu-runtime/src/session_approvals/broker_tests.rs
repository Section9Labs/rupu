use super::broker::*;
use super::protocol::{read_frame, write_frame, ClientFrame, ServerFrame};
use super::store::{Decision, DecisionRequest, State};
use rupu_tools::approval::{
    ApprovalCompletion, ApprovalEffect, ApprovalOwner, CallApprovalRequest,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

fn fixture() -> (tempfile::TempDir, Arc<Broker>, ApprovalOwner) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let directory = root.path().join("sessions/ses_fixture");
    std::fs::create_dir_all(&directory).unwrap();
    let owner = ApprovalOwner {
        session_id: "ses_fixture".into(),
        root_turn_run_id: "run_fixture".into(),
        run_id: "run_fixture".into(),
        parent_run_id: None,
        root_run_id: "run_fixture".into(),
        workspace_id: "ws_fixture".into(),
        workspace_path: project.to_string_lossy().into_owned(),
        settings_revision: 0,
        orchestration_revision: 0,
        host_mcp_registry_fingerprint: None,
    };
    std::fs::write(directory.join("session.json"),serde_json::to_vec(&json!({"session_id":owner.session_id,"builtin_harness":true,"workspace_id":owner.workspace_id,"workspace_path":owner.workspace_path,"permission_mode":"ask","status":"running","active_run_id":owner.run_id,"worker_pid":std::process::id()})).unwrap()).unwrap();
    let broker = Broker::open(root.path().into()).unwrap();
    (root, broker, owner)
}
async fn channel(b: &Arc<Broker>, o: &ApprovalOwner) -> UnixStream {
    let socket = b
        .spawn_channel(
            Some(o.session_id.clone()),
            o.workspace_id.clone(),
            PathBuf::from(&o.workspace_path),
        )
        .unwrap();
    socket.set_nonblocking(true).unwrap();
    let mut stream = UnixStream::from_std(socket).unwrap();
    write_frame(
        &mut stream,
        &ClientFrame::Bind {
            session_id: o.session_id.clone(),
            worker_pid: std::process::id(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame::<_, ServerFrame>(&mut stream).await.unwrap(),
        ServerFrame::Bound { .. }
    ));
    write_frame(&mut stream, &ClientFrame::CheckOwner { owner: o.clone() })
        .await
        .unwrap();
    assert!(matches!(
        read_frame::<_, ServerFrame>(&mut stream).await.unwrap(),
        ServerFrame::Ready {}
    ));
    stream
}
fn call(owner: ApprovalOwner) -> CallApprovalRequest {
    CallApprovalRequest {
        owner,
        turn_index: 0,
        tool_call_id: "call_fixture".into(),
        call_sequence: 0,
        tool: "write_file".into(),
        called_as: "write_file".into(),
        effect: ApprovalEffect::Write,
        descriptor_fingerprint: format!("sha256:{}", "1".repeat(64)),
        grant_fingerprint: format!("sha256:{}", "2".repeat(64)),
        remote: None,
        input: json!({"path":"note.txt","content":"fixture"}),
    }
}
async fn pending(stream: &mut UnixStream, c: CallApprovalRequest) -> Value {
    write_frame(stream, &ClientFrame::Request { call: c })
        .await
        .unwrap();
    match read_frame::<_, ServerFrame>(stream).await.unwrap() {
        ServerFrame::Pending { event } => event,
        _ => panic!("expected actual pending receipt"),
    }
}
async fn decide(b: &Arc<Broker>, e: &Value, decision: Decision) {
    let before = b
        .record(
            e["session_id"].as_str().unwrap().into(),
            e["approval_id"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(
        before.state,
        State::Pending,
        "fixture decision must still be pending"
    );
    assert!(
        b.live_record(&before),
        "fixture decision must retain exact live ownership"
    );
    assert_eq!(before.revision, e["revision"].as_u64().unwrap());
    assert_eq!(
        before.intent_fingerprint,
        e["intent_fingerprint"].as_str().unwrap()
    );
    b.decide(
        e["session_id"].as_str().unwrap().into(),
        e["approval_id"].as_str().unwrap().into(),
        DecisionRequest {
            decision_id: "decision_fixture".into(),
            expected_revision: e["revision"].as_u64().unwrap(),
            intent_fingerprint: e["intent_fingerprint"].as_str().unwrap().into(),
            decision,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn one_exact_decision_reserves_then_records_one_outcome() {
    let (_root, b, o) = fixture();
    let mut stream = channel(&b, &o).await;
    let event = pending(&mut stream, call(o.clone())).await;
    assert_eq!(event["state"], "pending");
    assert!(event.get("review").is_none());
    decide(&b, &event, Decision::AllowOnce).await;
    let (id, hash) = match read_frame::<_, ServerFrame>(&mut stream).await.unwrap() {
        ServerFrame::Reserved {
            approval_id,
            intent_fingerprint,
        } => (approval_id, intent_fingerprint),
        _ => panic!("expected one reservation"),
    };
    write_frame(
        &mut stream,
        &ClientFrame::Complete {
            approval_id: id.clone(),
            intent_fingerprint: hash,
            outcome: ApprovalCompletion {
                success: true,
                error: None,
                result_fingerprint: None,
            },
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame::<_, ServerFrame>(&mut stream).await.unwrap(),
        ServerFrame::Recorded {}
    ));
    let record = b.record(o.session_id.clone(), id).await.unwrap();
    assert_eq!(record.state, State::Completed);
    assert!(record.completion.unwrap().success);
    // Same exact runtime call cannot mint another approval after completion.
    write_frame(&mut stream, &ClientFrame::Request { call: call(o) })
        .await
        .unwrap();
    assert!(read_frame::<_, ServerFrame>(&mut stream).await.is_err());
}

#[tokio::test]
async fn denial_never_issues_a_dispatch_permit() {
    let (_root, b, o) = fixture();
    let mut stream = channel(&b, &o).await;
    let event = pending(&mut stream, call(o.clone())).await;
    decide(&b, &event, Decision::Deny).await;
    assert!(matches!(
        read_frame::<_, ServerFrame>(&mut stream).await.unwrap(),
        ServerFrame::Denied { .. }
    ));
    let record = b
        .record(o.session_id, event["approval_id"].as_str().unwrap().into())
        .await
        .unwrap();
    assert_eq!(record.state, State::Denied);
    assert!(record.dispatch_id.is_none());
}

#[tokio::test]
async fn lost_worker_after_reserved_stays_unknown_without_replay() {
    let (_root, b, o) = fixture();
    let mut stream = channel(&b, &o).await;
    let event = pending(&mut stream, call(o.clone())).await;
    decide(&b, &event, Decision::AllowOnce).await;
    assert!(matches!(
        read_frame::<_, ServerFrame>(&mut stream).await.unwrap(),
        ServerFrame::Reserved { .. }
    ));
    stream.shutdown().await.unwrap();
    drop(stream);
    for _ in 0..100 {
        if let Ok(record) = b
            .record(
                o.session_id.clone(),
                event["approval_id"].as_str().unwrap().into(),
            )
            .await
        {
            if record.state == State::Unknown {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("uncertain reservation was not retained")
}

#[tokio::test]
async fn changed_project_is_refused_before_request_publication() {
    let (_root, b, o) = fixture();
    let mut stream = channel(&b, &o).await;
    let mut forged = o.clone();
    forged.workspace_id = "ws_other".into();
    write_frame(&mut stream, &ClientFrame::CheckOwner { owner: forged })
        .await
        .unwrap();
    assert!(read_frame::<_, ServerFrame>(&mut stream).await.is_err());
    assert!(b
        .list(o.session_id, None, 50)
        .await
        .unwrap()
        .records
        .is_empty());
}

#[test]
fn credentials_are_not_published_and_hashing_preserves_unicode_values() {
    assert!(!reviewable(
        &json!({"nested":{"authorization":"fixture"}}),
        0
    ));
    assert_ne!(
        fingerprint(&json!({"x":"é"})).unwrap(),
        fingerprint(&json!({"x":"e\u{301}"})).unwrap()
    );
    assert_eq!(
        fingerprint(&json!({"a":1,"b":2})).unwrap(),
        fingerprint(&json!({"b":2,"a":1})).unwrap()
    );
}

#[tokio::test]
async fn credential_revision_change_revokes_pending_even_with_same_registry() {
    use rupu_auth::mcp_credentials::{Binding, Store};
    for remove in [false, true] {
        let (root, b, mut o) = fixture();
        let registry = rupu_config::mcp::Registry::parse(
            r#"
version = 1
[servers.fixture]
label = "Fixture"
url = "https://example.invalid/mcp"
enabled = true
bearer_store = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("mcp.toml"),
            toml::to_string(&registry).unwrap(),
        )
        .unwrap();
        let credentials = Store::for_global(root.path());
        let binding = Binding {
            server_id: "fixture".into(),
            url: "https://example.invalid/mcp".into(),
            generation: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
        };
        let enrolled = credentials
            .mutate(
                &binding,
                "absent",
                "01ARZ3NDEKTSV4RRFFQ69G5FAW",
                Some("synthetic-test-only"),
            )
            .unwrap();
        o.host_mcp_registry_fingerprint =
            Some(super::authority::fingerprint(root.path(), &registry).unwrap());
        let mut stream = channel(&b, &o).await;
        let mut request = call(o.clone());
        request.remote = Some(rupu_tools::approval::ApprovalRemote {
            server_id: "fixture".into(),
            tool: "write".into(),
        });
        let event = pending(&mut stream, request).await;
        credentials
            .mutate(
                &binding,
                &enrolled.credential_revision,
                "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                if remove {
                    None
                } else {
                    Some("synthetic-replacement-only")
                },
            )
            .unwrap();
        let result = b
            .decide(
                o.session_id.clone(),
                event["approval_id"].as_str().unwrap().into(),
                DecisionRequest {
                    decision_id: "decision_fixture".into(),
                    expected_revision: 0,
                    intent_fingerprint: event["intent_fingerprint"].as_str().unwrap().into(),
                    decision: Decision::AllowOnce,
                },
            )
            .await;
        assert!(result.is_err());
        assert!(tokio::time::timeout(
            Duration::from_secs(3),
            read_frame::<_, ServerFrame>(&mut stream)
        )
        .await
        .unwrap()
        .is_err());
    }
}

#[tokio::test]
async fn equivalent_workspace_alias_is_owned_but_retargeted_alias_is_not() {
    let (root, b, mut o) = fixture();
    let alias = root.path().join("workspace-alias");
    std::os::unix::fs::symlink(&o.workspace_path, &alias).unwrap();
    o.workspace_path = alias.to_string_lossy().into_owned();
    let mut stream = channel(&b, &o).await;
    let event = pending(&mut stream, call(o.clone())).await;
    let record = b
        .record(
            o.session_id.clone(),
            event["approval_id"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert!(b.live_record(&record));
    let other = root.path().join("other-project");
    std::fs::create_dir(&other).unwrap();
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(other, &alias).unwrap();
    assert!(!b.live_record(&record));
    assert!(b
        .decide(
            o.session_id,
            event["approval_id"].as_str().unwrap().into(),
            DecisionRequest {
                decision_id: "decision_fixture".into(),
                expected_revision: 0,
                intent_fingerprint: event["intent_fingerprint"].as_str().unwrap().into(),
                decision: Decision::AllowOnce,
            }
        )
        .await
        .is_err());
    assert!(tokio::time::timeout(
        Duration::from_secs(3),
        read_frame::<_, ServerFrame>(&mut stream)
    )
    .await
    .unwrap()
    .is_err());
}

#[tokio::test]
async fn pending_observation_does_not_contend_with_exact_decision() {
    let (_root, b, o) = fixture();
    let mut stream = channel(&b, &o).await;
    let event = pending(&mut stream, call(o.clone())).await;
    let id = event["approval_id"].as_str().unwrap().to_owned();
    let mut readers = Vec::new();
    for _ in 0..16 {
        let broker = b.clone();
        let session = o.session_id.clone();
        let id = id.clone();
        readers.push(tokio::spawn(
            async move { broker.record(session, id).await },
        ));
    }
    decide(&b, &event, Decision::Deny).await;
    for reader in readers {
        reader
            .await
            .unwrap()
            .expect("concurrent observation must retain store access");
    }
    assert!(matches!(
        read_frame::<_, ServerFrame>(&mut stream).await.unwrap(),
        ServerFrame::Denied { .. }
    ));
    let record = b.record(o.session_id, id).await.unwrap();
    assert_eq!(record.state, State::Denied);
    assert!(record.dispatch_id.is_none());
}
