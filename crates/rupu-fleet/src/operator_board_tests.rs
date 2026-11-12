use crate::{
    address::OPERATOR, Board, Bus, ClaimOutcome, FleetError, OperatorBoardWrite,
    OperatorClaimOutcome, OperatorVia, PostKind,
};
use std::{
    sync::{Arc, Barrier},
    time::Duration,
};

fn post() -> OperatorBoardWrite {
    OperatorBoardWrite::Post {
        kind: PostKind::Note,
        to: None,
        body: "operator observation".into(),
        expected_run_id: None,
    }
}
#[test]
fn operator_posts_are_attributed_without_joining_or_weakening_worker_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = Bus::open(tmp.path(), OPERATOR);
    assert!(operator.post(PostKind::Note, None, "forged").is_err());
    assert!(Bus::open(tmp.path(), "worker")
        .operator_board(post(), OperatorVia::Cp, None, || Ok(()))
        .is_err());
    let receipt = operator
        .operator_board(post(), OperatorVia::Cp, Some("mcp".into()), || Ok(()))
        .unwrap();
    assert_eq!(receipt.message.from, OPERATOR);
    assert_eq!(receipt.message.via, Some(OperatorVia::Cp));
    assert_eq!(receipt.message.client.as_deref(), Some("mcp"));
    assert!(receipt.message.from_codename.is_none());
    assert_eq!(operator.board.read_posts().unwrap()[0].author, OPERATOR);
    assert!(operator.roster().unwrap().participants().next().is_none());
}
#[test]
fn guarded_retraction_is_atomic_and_never_impersonates_the_original_author() {
    let tmp = tempfile::tempdir().unwrap();
    let worker = Bus::open(tmp.path(), "lead");
    worker.directive(None, "keep the target scope").unwrap();
    let expected = worker.board.read_directives().unwrap().remove(0);
    let bus = Arc::new(Bus::open(tmp.path(), OPERATOR));
    let mut stale = expected.clone();
    stale.body = "changed".into();
    let before = std::fs::read(bus.log_path()).unwrap();
    assert!(bus
        .operator_board(
            OperatorBoardWrite::Retract { expected: stale },
            OperatorVia::Cp,
            None,
            || Ok(())
        )
        .is_err());
    assert_eq!(std::fs::read(bus.log_path()).unwrap(), before);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (bus, barrier, expected) = (bus.clone(), barrier.clone(), expected.clone());
            std::thread::spawn(move || {
                barrier.wait();
                bus.operator_board(
                    OperatorBoardWrite::Retract { expected },
                    OperatorVia::Cp,
                    None,
                    || Ok(()),
                )
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let success = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(success.message.from, OPERATOR);
    assert_eq!(
        success.message.refs.retracts.as_deref(),
        Some(expected.id.as_str())
    );
    assert!(bus.board.read_directives().unwrap().is_empty());
}
#[test]
fn owner_and_recipient_preconditions_refuse_without_logging_or_applying() {
    let tmp = tempfile::tempdir().unwrap();
    let bus = Bus::open(tmp.path(), OPERATOR);
    assert!(bus
        .operator_board(post(), OperatorVia::Cp, None, || Err(FleetError::Refused(
            "owner changed".into()
        )))
        .is_err());
    let stale = OperatorBoardWrite::Post {
        kind: PostKind::Note,
        to: Some(crate::Address::Named("worker".into())),
        body: "hi".into(),
        expected_run_id: Some("run_old".into()),
    };
    assert!(matches!(
        bus.operator_board(stale, OperatorVia::Cp, None, || Ok(())),
        Err(FleetError::RecipientChanged)
    ));
    assert!(!bus.log_path().exists());
    assert!(bus.board.read_posts().unwrap().is_empty());
}
#[test]
fn operator_lease_survives_request_and_stale_release_cannot_remove_successor_or_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let first = {
        let bus = Bus::open(tmp.path(), OPERATOR);
        match bus
            .operator_claim("audit", Duration::from_secs(30), || Ok(()))
            .unwrap()
        {
            OperatorClaimOutcome::Granted { lease } => lease,
            other => panic!("{other:?}"),
        }
    };
    let board = Board::new(tmp.path());
    assert_eq!(
        board.operator_leases().unwrap().as_slice(),
        std::slice::from_ref(&first)
    );
    assert!(!board.operator_release("audit", "wrong").unwrap());
    assert!(board.operator_release("audit", &first.token).unwrap());
    let second = match board
        .operator_claim("audit", Duration::from_secs(30))
        .unwrap()
    {
        OperatorClaimOutcome::Granted { lease } => lease,
        other => panic!("{other:?}"),
    };
    assert_ne!(first.token, second.token);
    assert!(!board.operator_release("audit", &first.token).unwrap());
    assert_eq!(
        board.claim_holder("audit").unwrap().as_deref(),
        Some(OPERATOR)
    );
    let ClaimOutcome::Granted(worker_guard) = board
        .claim("worker-job", "worker", Duration::from_secs(30))
        .unwrap()
    else {
        panic!()
    };
    assert!(
        matches!(board.operator_claim("worker-job", Duration::from_secs(30)).unwrap(),
        OperatorClaimOutcome::Denied { holder } if holder == "worker")
    );
    assert!(!board.operator_release("worker-job", &second.token).unwrap());
    assert_eq!(
        board.claim_holder("worker-job").unwrap().as_deref(),
        Some("worker")
    );
    assert!(board
        .operator_leases()
        .unwrap()
        .iter()
        .all(|lease| lease.owner == OPERATOR));
    drop(worker_guard);
}
#[test]
fn operator_claim_ttl_and_count_are_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    assert!(board
        .operator_claim("bad", Duration::from_secs(29))
        .is_err());
    assert!(board
        .operator_claim("bad", Duration::from_secs(3601))
        .is_err());
    assert!(!tmp.path().join("board").exists());
    for index in 0..128 {
        assert!(matches!(
            board
                .operator_claim(&format!("unit-{index}"), Duration::from_secs(3600))
                .unwrap(),
            OperatorClaimOutcome::Granted { .. }
        ));
    }
    assert!(board
        .operator_claim("overflow", Duration::from_secs(30))
        .is_err());
    assert!(board.claim_holder("overflow").unwrap().is_none());
}

#[test]
fn partial_operator_apply_retains_one_canonical_log_receipt_without_replay() {
    let tmp = tempfile::tempdir().unwrap();
    let bus = Bus::open(tmp.path(), OPERATOR);
    bus.fail_after_log
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = bus
        .operator_board(post(), OperatorVia::Cp, None, || Ok(()))
        .unwrap_err();
    let FleetError::PartlyDelivered { id, .. } = error else {
        panic!("expected partial receipt")
    };
    let page = bus.read_log(None, 10).unwrap();
    assert_eq!(page.lines.len(), 1);
    let crate::LogLine::Msg(recorded) = &page.lines[0] else {
        panic!("expected message")
    };
    assert_eq!(recorded.id, id);
    assert_eq!(recorded.from, OPERATOR);
    assert!(bus.board.read_posts().unwrap().is_empty());
}

#[test]
fn expired_operator_lease_can_be_replaced_but_never_released_by_its_old_token() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let OperatorClaimOutcome::Granted { lease: first } = board
        .operator_claim("audit", Duration::from_secs(30))
        .unwrap()
    else {
        panic!()
    };
    let path = std::fs::read_dir(tmp.path().join("board/claims"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    raw["lease_expires_at"] = serde_json::json!("2000-01-01T00:00:00Z");
    std::fs::write(&path, raw.to_string()).unwrap();
    assert!(board.operator_leases().unwrap().is_empty());
    assert!(!board.operator_release("audit", &first.token).unwrap());
    let OperatorClaimOutcome::Granted { lease: second } = board
        .operator_claim("audit", Duration::from_secs(30))
        .unwrap()
    else {
        panic!()
    };
    assert_ne!(first.token, second.token);
    assert!(!board.operator_release("audit", &first.token).unwrap());
    assert_eq!(board.operator_leases().unwrap(), [second]);
}

#[test]
fn operator_claim_inspection_refuses_oversize_records() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let claims = tmp.path().join("board/claims");
    std::fs::create_dir_all(&claims).unwrap();
    std::fs::write(claims.join("oversize.json"), vec![b' '; 65_537]).unwrap();
    assert!(board.operator_leases().is_err());
}

#[test]
fn bounded_operator_directive_reader_preserves_existing_fold_semantics() {
    let tmp = tempfile::tempdir().unwrap();
    let bus = Bus::open(tmp.path(), "lead");
    assert!(bus.board.operator_directives().unwrap().is_empty());
    let first = bus.directive(None, "first").unwrap();
    bus.directive(None, "second").unwrap();
    bus.retract(&first.message.id).unwrap();
    assert_eq!(
        bus.board.operator_directives().unwrap(),
        bus.board.read_directives().unwrap()
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(tmp.path().join("board/directives.jsonl"))
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    assert!(matches!(
        bus.board.operator_directives(),
        Err(FleetError::Refused(_))
    ));
}

#[test]
fn guarded_operator_board_lead_requires_exact_live_registration() {
    let tmp = tempfile::tempdir().unwrap();
    let lead = Bus::open(tmp.path(), crate::address::LEAD);
    let operator = Bus::open(tmp.path(), OPERATOR);
    let action = |directive: bool, run: &str| {
        if directive {
            OperatorBoardWrite::Directive {
                to: Some(crate::Address::Lead),
                body: "review current scope".into(),
                expected_run_id: Some(run.into()),
            }
        } else {
            OperatorBoardWrite::Post {
                kind: PostKind::Note,
                to: Some(crate::Address::Lead),
                body: "review current scope".into(),
                expected_run_id: Some(run.into()),
            }
        }
    };
    // A legacy mailbox is not proof that a current lead owns this address.
    std::fs::create_dir_all(tmp.path().join("mailboxes/lead")).unwrap();
    for directive in [false, true] {
        assert!(matches!(
            operator.operator_board(action(directive, "af_lead"), OperatorVia::Cp, None, || Ok(
                ()
            )),
            Err(FleetError::RecipientChanged)
        ));
    }
    assert!(!operator.log_path().exists());
    lead.join(crate::Member {
        agent: "lead-agent".into(),
        run_id: "af_lead".into(),
        kind: "agentiflow-lead".into(),
        ..Default::default()
    })
    .unwrap();
    for directive in [false, true] {
        let receipt = operator
            .operator_board(action(directive, "af_lead"), OperatorVia::Cp, None, || {
                Ok(())
            })
            .unwrap();
        assert_eq!(receipt.message.from, OPERATOR);
        assert_eq!(receipt.message.to.as_deref(), Some(crate::address::LEAD));
        assert_eq!(receipt.message.via, Some(OperatorVia::Cp));
        assert!(receipt.message.from_codename.is_none());
    }
    let before = std::fs::read(operator.log_path()).unwrap();
    for directive in [false, true] {
        assert!(matches!(
            operator.operator_board(
                action(directive, "af_previous"),
                OperatorVia::Cp,
                None,
                || Ok(())
            ),
            Err(FleetError::RecipientChanged)
        ));
    }
    lead.leave("completed").unwrap();
    for directive in [false, true] {
        assert!(matches!(
            operator.operator_board(action(directive, "af_lead"), OperatorVia::Cp, None, || Ok(
                ()
            )),
            Err(FleetError::RecipientChanged)
        ));
    }
    assert_eq!(std::fs::read(operator.log_path()).unwrap(), before);
}
