use super::*;
use rupu_tools::approval::ApprovalEffect;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

fn owner() -> ApprovalOwner {
    ApprovalOwner {
        session_id: "ses_fixture".into(),
        root_turn_run_id: "run_fixture".into(),
        run_id: "run_fixture".into(),
        parent_run_id: None,
        root_run_id: "run_fixture".into(),
        workspace_id: "ws_fixture".into(),
        workspace_path: "/fixture/project".into(),
        settings_revision: 3,
        orchestration_revision: 2,
        host_mcp_registry_fingerprint: None,
    }
}
fn request() -> CallApprovalRequest {
    CallApprovalRequest {
        owner: owner(),
        turn_index: 7,
        tool_call_id: "call_fixture".into(),
        call_sequence: 2,
        tool: "core.write_file".into(),
        called_as: "write_file".into(),
        effect: ApprovalEffect::Write,
        descriptor_fingerprint: "descriptor_fixture".into(),
        grant_fingerprint: "grant_fixture".into(),
        remote: None,
        input: json!({"path":"fixture.txt","content":"fixture"}),
    }
}
fn pending() -> Value {
    json!({"session_id":"ses_fixture","run_id":"run_fixture","root_turn_run_id":"run_fixture",
        "workspace_id":"ws_fixture","tool_call_id":"call_fixture","turn_index":7,"call_sequence":2,
        "cp_instance_id":"instance_fixture","broker_epoch":"epoch_fixture","worker_generation":4,
        "root_run_id":"run_fixture","parent_run_id":null,"tool":"core.write_file","effect":"write",
        "approval_id":"approval_fixture","intent_fingerprint":"fingerprint_fixture"})
}
async fn pair() -> (Arc<Connection>, tokio::net::UnixStream) {
    let (worker, broker) = UnixStream::pair().unwrap();
    broker.set_nonblocking(true).unwrap();
    let mut broker = tokio::net::UnixStream::from_std(broker).unwrap();
    let bind = tokio::spawn(Connection::bind(worker, "ses_fixture".into()));
    assert!(
        matches!(read_frame::<_, ClientFrame>(&mut broker).await.unwrap(),
        ClientFrame::Bind { session_id, .. } if session_id == "ses_fixture")
    );
    write_frame(
        &mut broker,
        &ServerFrame::Bound {
            instance_id: "instance_fixture".into(),
            broker_epoch: "epoch_fixture".into(),
            worker_generation: 4,
        },
    )
    .await
    .unwrap();
    (bind.await.unwrap().unwrap(), broker)
}
async fn receive_request(broker: &mut tokio::net::UnixStream) {
    assert!(
        matches!(read_frame::<_, ClientFrame>(broker).await.unwrap(), ClientFrame::Request { call } if call == request())
    );
}
async fn send_reserved(broker: &mut tokio::net::UnixStream) {
    write_frame(broker, &ServerFrame::Pending { event: pending() })
        .await
        .unwrap();
    write_frame(
        broker,
        &ServerFrame::Reserved {
            approval_id: "approval_fixture".into(),
            intent_fingerprint: "fingerprint_fixture".into(),
        },
    )
    .await
    .unwrap();
}
async fn assert_closed(broker: &mut tokio::net::UnixStream) {
    let mut byte = [0];
    let read = tokio::time::timeout(Duration::from_secs(2), broker.read(&mut byte))
        .await
        .expect("peer must close");
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "no further protocol request is sent"
    );
}

#[tokio::test]
async fn abort_after_pending_closes_actual_socket_with_surviving_handles() {
    let (connection, mut broker) = pair().await;
    let authorizer = connection.authorizer(owner()).unwrap();
    let surviving = authorizer.clone();
    let (seen, observed) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut seen = Some(seen);
        authorizer
            .authorize(request(), &mut move |_| {
                let _ = seen.take().unwrap().send(());
                Ok(())
            })
            .await
    });
    receive_request(&mut broker).await;
    write_frame(&mut broker, &ServerFrame::Pending { event: pending() })
        .await
        .unwrap();
    observed.await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(e) if e.is_cancelled()));
    assert_closed(&mut broker).await;
    assert!(surviving.check_owner(&owner()).await.is_err());
    assert!(connection.authorizer(owner()).is_err());
}

#[tokio::test]
async fn mismatched_pending_owner_never_reaches_observer_or_dispatch() {
    for key in [
        "worker_generation",
        "broker_epoch",
        "run_id",
        "root_run_id",
        "parent_run_id",
        "tool",
        "effect",
    ] {
        let (connection, mut broker) = pair().await;
        let authorizer = connection.authorizer(owner()).unwrap();
        let task = tokio::spawn(async move {
            authorizer
                .authorize(request(), &mut |_| {
                    panic!("unverified event must not be published")
                })
                .await
                .is_err()
        });
        receive_request(&mut broker).await;
        let mut event = pending();
        event[key] = json!("wrong");
        write_frame(&mut broker, &ServerFrame::Pending { event })
            .await
            .unwrap();
        assert!(task.await.unwrap(), "{key}");
        assert_closed(&mut broker).await;
    }
}

#[tokio::test]
async fn dropped_reserved_permit_closes_before_waiting_sibling_can_send() {
    let (connection, mut broker) = pair().await;
    let authorizer = connection.authorizer(owner()).unwrap();
    let first = authorizer.clone();
    let task = tokio::spawn(async move { first.authorize(request(), &mut |_| Ok(())).await });
    receive_request(&mut broker).await;
    send_reserved(&mut broker).await;
    let permit = match task.await.unwrap().unwrap() {
        ApprovalOutcome::Permitted(p) => p,
        _ => panic!("permit"),
    };
    let sibling = tokio::spawn(async move { authorizer.check_owner(&owner()).await });
    tokio::task::yield_now().await;
    drop(permit);
    assert!(sibling.await.unwrap().is_err());
    assert_closed(&mut broker).await;
}

#[tokio::test]
async fn completion_requires_recorded_then_releases_serial_channel() {
    let (connection, mut broker) = pair().await;
    let authorizer = connection.authorizer(owner()).unwrap();
    let first = authorizer.clone();
    let task = tokio::spawn(async move { first.authorize(request(), &mut |_| Ok(())).await });
    receive_request(&mut broker).await;
    send_reserved(&mut broker).await;
    let permit = match task.await.unwrap().unwrap() {
        ApprovalOutcome::Permitted(p) => p,
        _ => panic!("permit"),
    };
    let done = tokio::spawn(permit.complete(ApprovalCompletion {
        success: true,
        error: None,
        result_fingerprint: None,
    }));
    assert!(
        matches!(read_frame::<_, ClientFrame>(&mut broker).await.unwrap(), ClientFrame::Complete {
        approval_id, intent_fingerprint, outcome
    } if approval_id == "approval_fixture" && intent_fingerprint == "fingerprint_fixture" && outcome.success)
    );
    write_frame(&mut broker, &ServerFrame::Recorded {})
        .await
        .unwrap();
    done.await.unwrap().unwrap();
    let next = tokio::spawn(async move { authorizer.check_owner(&owner()).await });
    assert!(matches!(
        read_frame::<_, ClientFrame>(&mut broker).await.unwrap(),
        ClientFrame::CheckOwner { .. }
    ));
    write_frame(&mut broker, &ServerFrame::Ready {})
        .await
        .unwrap();
    next.await.unwrap().unwrap();
}

#[tokio::test]
async fn mismatched_reserved_fingerprint_is_not_a_permit() {
    let (connection, mut broker) = pair().await;
    let authorizer = connection.authorizer(owner()).unwrap();
    let task = tokio::spawn(async move {
        authorizer
            .authorize(request(), &mut |_| Ok(()))
            .await
            .is_err()
    });
    receive_request(&mut broker).await;
    write_frame(&mut broker, &ServerFrame::Pending { event: pending() })
        .await
        .unwrap();
    write_frame(
        &mut broker,
        &ServerFrame::Reserved {
            approval_id: "approval_fixture".into(),
            intent_fingerprint: "other".into(),
        },
    )
    .await
    .unwrap();
    assert!(task.await.unwrap());
    assert_closed(&mut broker).await;
}

#[tokio::test]
async fn oversized_frame_rejected_before_payload_read_and_unknown_fields_refused() {
    use tokio::io::AsyncWriteExt;
    let (mut tx, mut rx) = tokio::io::duplex(64);
    tx.write_u32((super::super::protocol::MAX_FRAME_BYTES + 1) as u32)
        .await
        .unwrap();
    assert!(read_frame::<_, ClientFrame>(&mut rx).await.is_err());
    for tag in ["ready", "recorded"] {
        assert!(serde_json::from_value::<ServerFrame>(json!({"type":tag,"grant":"all"})).is_err());
        let clean = json!({"type":tag});
        let frame: ServerFrame = serde_json::from_value(clean.clone()).unwrap();
        assert_eq!(serde_json::to_value(frame).unwrap(), clean);
    }
}

#[tokio::test]
async fn broken_worker_capability_stays_closed_but_explicit_new_worker_can_bind() {
    let (old, mut broker) = pair().await;
    let authorizer = old.authorizer(owner()).unwrap();
    let task = tokio::spawn(async move { authorizer.check_owner(&owner()).await });
    assert!(matches!(
        read_frame::<_, ClientFrame>(&mut broker).await.unwrap(),
        ClientFrame::CheckOwner { .. }
    ));
    drop(broker);
    assert!(task.await.unwrap().is_err());
    assert!(
        old.is_closed(),
        "worker loop must retire this capability, not reuse it"
    );
    assert!(old.authorizer(owner()).is_err());

    // A later explicit send launches a distinct worker/channel. The old
    // request/permit is never transferred or replayed onto the new socket.
    let (new, mut broker) = pair().await;
    assert!(!new.is_closed());
    let authorizer = new.authorizer(owner()).unwrap();
    let task = tokio::spawn(async move { authorizer.check_owner(&owner()).await });
    assert!(matches!(
        read_frame::<_, ClientFrame>(&mut broker).await.unwrap(),
        ClientFrame::CheckOwner { .. }
    ));
    write_frame(&mut broker, &ServerFrame::Ready {})
        .await
        .unwrap();
    task.await.unwrap().unwrap();
    assert!(old.is_closed());
}

#[tokio::test]
async fn authoritative_denial_preserves_safe_reason_without_closing_channel() {
    let (connection, mut broker) = pair().await;
    let authorizer = connection.authorizer(owner()).unwrap();
    let task = tokio::spawn(async move { authorizer.authorize(request(), &mut |_| Ok(())).await });
    receive_request(&mut broker).await;
    write_frame(
        &mut broker,
        &ServerFrame::Denied {
            reason: "versioned credential binding required".into(),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(task.await.unwrap().unwrap(), ApprovalOutcome::Denied { reason } if reason == "versioned credential binding required")
    );
    assert!(!connection.is_closed());
}
