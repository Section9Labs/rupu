//! Unit fixtures for the operator's enqueue precondition, never provider work.
use super::*;
use std::sync::{mpsc, Arc};

fn member(run_id: &str) -> Member {
    Member {
        agent: "review".into(),
        run_id: run_id.into(),
        kind: "subagent".into(),
        codename: Some("crew/lynx#1.2".into()),
        ..Default::default()
    }
}

fn guarded(to: &str, run_id: &str) -> OperatorMessage {
    OperatorMessage {
        to: Address::parse(to).unwrap(),
        expected_run_id: Some(run_id.into()),
        body: "Review the selected change".into(),
        urgent: true,
        via: OperatorVia::Cp,
        viewer: None,
        client: None,
    }
}

#[test]
fn expected_run_supports_the_registered_lead_without_a_prefix_assumption() {
    let tmp = tempfile::tempdir().unwrap();
    let lead = Bus::open(tmp.path(), address::LEAD);
    let operator = Bus::open(tmp.path(), address::OPERATOR);
    for run_id in ["run_123", "sub_123", "af_123", "turn_123", "legacy-123"] {
        lead.join(member(run_id)).unwrap();
        let receipt = operator
            .operator_send(guarded(address::LEAD, run_id))
            .unwrap();
        assert_eq!(receipt.delivery().resolved, [address::LEAD]);
        let inbox = lead.mailbox.drain(address::LEAD).unwrap();
        assert_eq!(inbox.len(), 1);
        assert!(inbox[0].urgent);
    }
    lead.join(member("run_next")).unwrap();
    let log_before = std::fs::read(operator.log_path()).unwrap();
    assert!(matches!(
        operator.operator_send(guarded(address::LEAD, "legacy-123")),
        Err(FleetError::RecipientChanged)
    ));
    assert_eq!(std::fs::read(operator.log_path()).unwrap(), log_before);
    assert_eq!(lead.mailbox.inbox_len(address::LEAD).unwrap(), 0);
}

#[test]
fn expected_run_invalid_requests_leave_an_absent_space_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let space = tmp.path().join("absent");
    let operator = Bus::open(&space, address::OPERATOR);
    for run_id in ["", "run x", "../run", "run/1", "run.1", "run_é", "run_\n"] {
        assert!(matches!(
            operator.operator_send(guarded("lynx#1", run_id)),
            Err(FleetError::Refused(_))
        ));
        assert!(!space.exists(), "invalid ID {run_id:?} created state");
    }
    assert!(matches!(
        operator.operator_send(guarded("lynx#1", &"x".repeat(129))),
        Err(FleetError::Refused(_))
    ));
    for selector in [
        "broadcast",
        "role:review",
        "step:scan",
        "parent",
        "operator",
    ] {
        assert!(matches!(
            operator.operator_send(guarded(selector, "run_one")),
            Err(FleetError::Refused(_))
        ));
        assert!(!space.exists(), "selector {selector} created state");
    }
}

#[test]
fn expected_run_never_falls_back_to_an_alias_role_or_legacy_mailbox() {
    let tmp = tempfile::tempdir().unwrap();
    let recipient = Bus::open(tmp.path(), "lynx#1");
    recipient.join(member("run_one")).unwrap();
    let operator = Bus::open(tmp.path(), address::OPERATOR);
    std::fs::create_dir_all(tmp.path().join("mailboxes/legacy_1")).unwrap();
    std::fs::create_dir_all(tmp.path().join("mailboxes/lead")).unwrap();
    for target in ["crew/lynx#1.2", "review", "legacy#1", "missing", "lead"] {
        assert!(matches!(
            operator.operator_send(guarded(target, "run_one")),
            Err(FleetError::RecipientChanged)
        ));
        assert!(!operator.log_path().exists());
        assert_eq!(operator.mailbox.inbox_len(target).unwrap(), 0);
    }
    assert_eq!(recipient.mailbox.inbox_len("lynx#1").unwrap(), 0);
    recipient.leave("completed").unwrap();
    assert!(matches!(
        operator.operator_send(guarded("lynx#1", "run_one")),
        Err(FleetError::RecipientChanged)
    ));
    assert!(!operator.log_path().exists());

    // Existing unguarded callers keep their legacy address resolution.
    let mut legacy = guarded("legacy#1", "unused");
    legacy.expected_run_id = None;
    assert_eq!(
        operator.operator_send(legacy).unwrap().delivery().resolved,
        ["legacy#1"]
    );
}

#[test]
fn expected_run_enqueue_serializes_join_and_leave_through_mailbox_delivery() {
    for rebind in [true, false] {
        enqueue_against_transition(rebind);
    }
}

fn enqueue_against_transition(rebind: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let recipient = Arc::new(Bus::open(tmp.path(), "lynx#1"));
    recipient.join(member("run_old")).unwrap();
    let operator = Bus::open(tmp.path(), address::OPERATOR);
    let (begin_tx, begin_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let contender = Arc::clone(&recipient);
    std::thread::scope(|scope| {
        let join = scope.spawn(move || {
            begin_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            started_tx.send(()).unwrap();
            if rebind {
                contender.join(member("run_new"))
            } else {
                contender.leave("completed")
            }
        });
        let to = Address::Named("lynx#1".into());
        let receipt = operator
            .write_as(
                address::OPERATOR,
                Draft {
                    kind: MessageKind::Operator,
                    to: Some(&to),
                    expected_run_id: Some("run_old"),
                    body: "Accepted for the old occupant",
                    urgent: true,
                    ..Draft::plain()
                },
                |message| {
                    begin_tx.send(()).unwrap();
                    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    let lock = std::fs::OpenOptions::new()
                        .write(true)
                        .open(tmp.path().join("log.jsonl.lock"))
                        .unwrap();
                    assert_eq!(
                        rustix::fs::flock(
                            &lock,
                            rustix::fs::FlockOperation::NonBlockingLockExclusive
                        ),
                        Err(rustix::io::Errno::WOULDBLOCK),
                        "enqueue must still hold the same file lock used by join and leave"
                    );
                    assert_eq!(
                        operator.roster()?.get("lynx#1").unwrap().member.run_id,
                        "run_old"
                    );
                    operator.deliver(message, false)
                },
            )
            .unwrap();
        join.join().unwrap().unwrap();
        assert_eq!(receipt.delivery().resolved, ["lynx#1"]);
    });
    let roster = operator.roster().unwrap();
    let current = roster.get("lynx#1").unwrap();
    assert_eq!(current.live, rebind);
    assert_eq!(
        current.member.run_id,
        if rebind { "run_new" } else { "run_old" }
    );
    // The contract is atomic enqueue, not a new durable recipient protocol:
    // a successfully queued message remains in the address-owned inbox after
    // a later join or leave. The precondition does not claim which later
    // occupant, if any, consumes that queued message.
    let inbox = recipient.mailbox.drain("lynx#1").unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].body, "Accepted for the old occupant");
}
