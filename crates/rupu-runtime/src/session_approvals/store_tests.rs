use super::*;
use serde_json::json;
use std::os::unix::fs::{symlink, PermissionsExt};

fn intent(id: &str) -> Intent {
    Intent {
        approval_id: id.into(),
        cp_instance_id: "cp_fixture".into(),
        broker_epoch: "epoch_fixture".into(),
        session_id: "ses_fixture".into(),
        root_turn_run_id: "run_fixture".into(),
        run_id: "run_fixture".into(),
        turn_index: 1,
        tool_call_id: format!("call_{id}"),
        call_sequence: 1,
        workspace_id: "ws_fixture".into(),
        workspace_path: "/private/tmp/approval-fixture".into(),
        worker_registration_id: "worker_fixture".into(),
        worker_generation: 1,
        parent_run_id: None,
        root_run_id: "run_fixture".into(),
        lineage_fingerprint: fingerprint(&json!(["run_fixture"])).unwrap(),
        tool: "write_file".into(),
        called_as: "write_file".into(),
        effect: Effect::Write,
        descriptor_fingerprint: fingerprint(&json!("descriptor")).unwrap(),
        grant_fingerprint: fingerprint(&json!(["write_file"])).unwrap(),
        settings_revision: 1,
        orchestration_revision: 1,
        approval_authority_revision: 1,
        host_mcp_registry_fingerprint: None,
        remote: None,
        input: json!({"path":"note.txt","content":"bounded fixture"}),
        review: json!({"action":"Write note.txt"}),
        created_at_ms: 1_000,
        expires_at_ms: 601_000,
    }
}
fn allow(row: &Record) -> DecisionRequest {
    DecisionRequest {
        decision_id: format!("decision_{}", row.intent.approval_id),
        expected_revision: row.revision,
        intent_fingerprint: row.intent_fingerprint.clone(),
        decision: Decision::AllowOnce,
    }
}
fn pre(row: &Record) -> PreDispatch {
    PreDispatch {
        intent_fingerprint: row.intent_fingerprint.clone(),
        worker_registration_id: row.intent.worker_registration_id.clone(),
        worker_generation: row.intent.worker_generation,
        broker_epoch: row.intent.broker_epoch.clone(),
        approval_authority_revision: row.intent.approval_authority_revision,
    }
}
fn denied(store: &mut Store, id: &str) -> Record {
    let row = store.create(intent(id), 1_000).unwrap();
    let mut req = allow(&row);
    req.decision = Decision::Deny;
    store.decide("ses_fixture", id, req, 1_100).unwrap()
}
fn path(root: &Path, id: &str) -> PathBuf {
    root.join(STORE_DIRECTORY)
        .join("active")
        .join(record_name(id))
}

#[test]
fn one_durable_allow_has_one_consumption_and_exact_receipt_replay() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let initial = store.create(intent("approval_one"), 1_000).unwrap();
    let req = allow(&initial);
    let allowed = store
        .decide("ses_fixture", "approval_one", req.clone(), 1_100)
        .unwrap();
    assert_eq!(
        store
            .decide("ses_fixture", "approval_one", req.clone(), 1_200)
            .unwrap(),
        allowed
    );
    let mut other = req.clone();
    other.decision = Decision::Deny;
    assert!(matches!(
        store.decide("ses_fixture", "approval_one", other, 1_200),
        Err(Error::Conflict)
    ));
    let ticket = store
        .reserve("ses_fixture", "approval_one", &pre(&allowed), 1_300)
        .unwrap();
    assert!(matches!(
        store.reserve("ses_fixture", "approval_one", &pre(&allowed), 1_300),
        Err(Error::Conflict)
    ));
    let completed = store
        .complete(
            ticket,
            Completion {
                success: false,
                error: Some("Tool returned an error after dispatch".into()),
                result_fingerprint: None,
            },
            1_400,
        )
        .unwrap();
    assert_eq!(completed.state, State::Completed);
    assert!(completed.dispatched_at_ms.is_some());
    assert!(!completed.completion.unwrap().success);
    assert_eq!(
        store
            .decide("ses_fixture", "approval_one", req, 1_500)
            .unwrap()
            .state,
        State::Completed
    );
}

#[test]
fn wrong_session_worker_authority_and_fingerprint_never_reserve() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let row = store.create(intent("approval_owned"), 1_000).unwrap();
    assert!(matches!(
        store.get("ses_other", "approval_owned"),
        Err(Error::NotFound)
    ));
    let row = store
        .decide("ses_fixture", "approval_owned", allow(&row), 1_100)
        .unwrap();
    for which in 0..4 {
        let mut p = pre(&row);
        match which {
            0 => p.worker_generation += 1,
            1 => p.broker_epoch = "different".into(),
            2 => p.approval_authority_revision += 1,
            _ => p.intent_fingerprint = fingerprint(&json!("other")).unwrap(),
        };
        assert!(matches!(
            store.reserve("ses_fixture", "approval_owned", &p, 1_200),
            Err(Error::Conflict)
        ));
    }
    assert_eq!(
        store.get("ses_fixture", "approval_owned").unwrap().state,
        State::Allowed
    );
}

#[test]
fn call_identity_survives_new_broker_worker_tool_input_and_archival() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let original = intent("original_call");
    denied(&mut store, "original_call");
    store.archive_terminal().unwrap();
    let mut replacement = original.clone();
    replacement.approval_id = "new_approval".into();
    replacement.cp_instance_id = "new_cp".into();
    replacement.broker_epoch = "new_epoch".into();
    replacement.worker_generation += 1;
    replacement.tool = "different_tool".into();
    replacement.input = json!({"different":"input"});
    assert!(matches!(
        store.create(replacement, 1_000),
        Err(Error::Conflict)
    ));
    let mut new_turn = original;
    new_turn.approval_id = "new_turn".into();
    new_turn.run_id = "run_new_turn".into();
    new_turn.root_turn_run_id = "run_new_turn".into();
    assert!(store.create(new_turn, 1_000).is_ok());
}

#[test]
fn orphaned_identity_claim_blocks_reissue_without_an_actionable_record() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let row = store.create(intent("orphan_claim"), 1_000).unwrap();
    std::fs::remove_file(path(root.path(), "orphan_claim")).unwrap();
    assert!(matches!(
        store.get("ses_fixture", "orphan_claim"),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.create(row.intent.clone(), 1_000),
        Err(Error::Conflict)
    ));
    let mut other = row.intent;
    other.approval_id = "replacement_id".into();
    assert!(matches!(store.create(other, 1_000), Err(Error::Conflict)));
}

#[test]
fn expiry_and_revoke_race_cannot_cross_reservation_boundary() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let row = store.create(intent("approval_expire"), 1_000).unwrap();
    let row = store
        .decide("ses_fixture", "approval_expire", allow(&row), 1_100)
        .unwrap();
    assert!(matches!(
        store.reserve("ses_fixture", "approval_expire", &pre(&row), 601_000),
        Err(Error::Expired)
    ));
    assert_eq!(
        store.get("ses_fixture", "approval_expire").unwrap().state,
        State::Expired
    );
    let row = store.create(intent("approval_revoke"), 1_000).unwrap();
    let req = RevokeRequest {
        operation_id: "revoke_once".into(),
        expected_revision: row.revision,
        intent_fingerprint: row.intent_fingerprint.clone(),
    };
    let revoked = store
        .revoke("ses_fixture", "approval_revoke", req.clone(), 1_100)
        .unwrap();
    assert_eq!(
        store
            .revoke("ses_fixture", "approval_revoke", req, 1_200)
            .unwrap(),
        revoked
    );
    assert!(store
        .reserve("ses_fixture", "approval_revoke", &pre(&revoked), 1_300)
        .is_err());
    let row = store.create(intent("approval_dispatch"), 1_000).unwrap();
    let row = store
        .decide("ses_fixture", "approval_dispatch", allow(&row), 1_100)
        .unwrap();
    let ticket = store
        .reserve("ses_fixture", "approval_dispatch", &pre(&row), 1_200)
        .unwrap();
    let req = RevokeRequest {
        operation_id: "too_late".into(),
        expected_revision: ticket.record().revision,
        intent_fingerprint: row.intent_fingerprint,
    };
    assert!(matches!(
        store.revoke("ses_fixture", "approval_dispatch", req, 1_300),
        Err(Error::Conflict)
    ));
}

#[test]
fn restart_and_worker_loss_preserve_unknown_and_never_reissue_ticket() {
    let root = tempfile::tempdir().unwrap();
    {
        let mut store = Store::open(root.path()).unwrap();
        let row = store.create(intent("approval_unknown"), 1_000).unwrap();
        let row = store
            .decide("ses_fixture", "approval_unknown", allow(&row), 1_100)
            .unwrap();
        let _ticket = store
            .reserve("ses_fixture", "approval_unknown", &pre(&row), 1_200)
            .unwrap();
        store.create(intent("approval_pending"), 1_000).unwrap();
    }
    let mut store = Store::open(root.path()).unwrap();
    assert_eq!(store.recover_epoch("new_epoch", 1_300).unwrap(), 2);
    assert_eq!(
        store.get("ses_fixture", "approval_unknown").unwrap().state,
        State::Unknown
    );
    assert_eq!(
        store.get("ses_fixture", "approval_pending").unwrap().state,
        State::Revoked
    );
    assert_eq!(store.archive_terminal().unwrap(), 1);
    assert!(path(root.path(), "approval_unknown").exists());
    assert!(matches!(
        store.create(intent("approval_unknown"), 1_000),
        Err(Error::Conflict)
    ));
}

#[test]
fn terminal_archive_retains_all_ids_and_scoped_bounded_history() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    for n in 0..65 {
        denied(&mut store, &format!("approval_{n:04}"));
    }
    store.archive_terminal().unwrap();
    assert!(names(&store.active, MAX_ACTIVE_RECORDS).unwrap().is_empty());
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = store.list("ses_fixture", cursor.as_deref(), 7).unwrap();
        assert!(page.records.len() <= 7);
        for row in page.records {
            assert!(seen.insert(row.intent.approval_id));
        }
        pages += 1;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(pages < 30);
    }
    assert_eq!(seen.len(), 65);
    for n in [0, 32, 64] {
        let id = format!("approval_{n:04}");
        assert_eq!(store.get("ses_fixture", &id).unwrap().state, State::Denied);
        assert!(matches!(
            store.create(intent(&id), 1_000),
            Err(Error::Conflict)
        ));
    }
    let page = store.list("ses_fixture", None, 7).unwrap();
    assert!(matches!(
        store.list("ses_other", page.next_cursor.as_deref(), 7),
        Err(Error::Conflict)
    ));
}

#[test]
fn cursor_authentication_snapshot_change_and_new_archive_head_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    for n in 0..4 {
        denied(&mut store, &format!("approval_page_{n}"));
    }
    let page = store.list("ses_fixture", None, 1).unwrap();
    let cursor = page.next_cursor.unwrap();
    let mut tampered = cursor.clone().into_bytes();
    tampered[0] = if tampered[0] == b'a' { b'b' } else { b'a' };
    assert!(store
        .list(
            "ses_fixture",
            Some(std::str::from_utf8(&tampered).unwrap()),
            1
        )
        .is_err());
    store.create(intent("approval_change"), 1_000).unwrap();
    assert!(matches!(
        store.list("ses_fixture", Some(&cursor), 1),
        Err(Error::Conflict)
    ));
    store
        .cancel(
            "ses_fixture",
            "approval_change",
            "cancelled_by_fixture",
            1_100,
        )
        .unwrap();
    store.archive_terminal().unwrap();
    let page = store.list("ses_fixture", None, 1).unwrap();
    let cursor = page.next_cursor.unwrap();
    denied(&mut store, "approval_later");
    store.archive_terminal().unwrap();
    // Captured immutable head is retained when active snapshot is still empty.
    assert!(store.list("ses_fixture", Some(&cursor), 1).is_ok());
}

#[test]
fn input_review_fingerprints_preserve_unicode_and_object_order() {
    assert_eq!(
        fingerprint(&json!({"b":2,"a":1})).unwrap(),
        fingerprint(&json!({"a":1,"b":2})).unwrap()
    );
    assert_ne!(
        fingerprint(&json!("é")).unwrap(),
        fingerprint(&json!("e\u{301}")).unwrap()
    );
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let mut oversized = intent("oversized");
    oversized.input = json!({"text":"x".repeat(MAX_INPUT_BYTES)});
    assert!(matches!(
        store.create(oversized, 1_000),
        Err(Error::Invalid)
    ));
    let mut changed = intent("changed");
    let row = store.create(changed.clone(), 1_000).unwrap();
    changed.input = json!({"path":"elsewhere"});
    assert_ne!(row.intent_fingerprint, fingerprint(&changed).unwrap());
    assert!(matches!(store.create(changed, 1_000), Err(Error::Conflict)));
}

#[test]
fn symlink_fifo_hardlink_and_root_replacement_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    drop(store);
    let lease = root.path().join(STORE_DIRECTORY).join("lease");
    std::fs::remove_file(&lease).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(&lease)
        .status()
        .unwrap()
        .success());
    assert!(Store::open(root.path()).is_err());
    std::fs::remove_file(&lease).unwrap();
    symlink("missing", &lease).unwrap();
    assert!(Store::open(root.path()).is_err());
    std::fs::remove_file(&lease).unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let row = store.create(intent("hardlink"), 1_000).unwrap();
    std::fs::hard_link(
        path(root.path(), "hardlink"),
        root.path().join("linked-record"),
    )
    .unwrap();
    assert!(store.get("ses_fixture", &row.intent.approval_id).is_err());
    std::fs::rename(root.path().join(STORE_DIRECTORY), root.path().join("moved")).unwrap();
    std::fs::create_dir(root.path().join(STORE_DIRECTORY)).unwrap();
    assert!(store.list("ses_fixture", None, 50).is_err());
}

#[test]
fn lease_is_exclusive_and_private_permissions_are_enforced() {
    let root = tempfile::tempdir().unwrap();
    let first = Store::open(root.path()).unwrap();
    assert!(Store::open(root.path()).is_err());
    drop(first);
    let path = root.path().join(STORE_DIRECTORY).join("active");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Store::open(root.path()).is_err());
}

#[test]
fn pending_admission_is_bounded_without_discarding_existing_records() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    for n in 0..MAX_PENDING_PER_SESSION {
        store
            .create(intent(&format!("pending_{n}")), 1_000)
            .unwrap();
    }
    assert!(matches!(
        store.create(intent("one_too_many"), 1_000),
        Err(Error::Capacity)
    ));
    assert_eq!(
        store.list("ses_fixture", None, 50).unwrap().records.len(),
        MAX_PENDING_PER_SESSION
    );
}

#[test]
fn epoch_recovery_and_archive_each_scan_active_records_once() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    for n in 0..32 {
        let mut value = intent(&format!("bulk_{n:02}"));
        value.session_id = format!("ses_batch_{}", n / 16);
        store.create(value, 1_000).unwrap();
    }
    store.active_scans.set(0);
    assert_eq!(store.recover_epoch("new_epoch", 1_100).unwrap(), 32);
    assert_eq!(
        store.active_scans.get(),
        1,
        "updates must not rescan the entire store"
    );
    store.active_scans.set(0);
    assert_eq!(store.archive_terminal().unwrap(), 32);
    assert_eq!(
        store.active_scans.get(),
        1,
        "archival groups one validated snapshot"
    );
}

#[test]
fn admission_expires_orphaned_waiters_but_preserves_dispatched_uncertainty() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    for id in ["still_reserved", "still_unknown"] {
        let row = store.create(intent(id), 1_000).unwrap();
        let allowed = store.decide("ses_fixture", id, allow(&row), 1_100).unwrap();
        let _ticket = store
            .reserve("ses_fixture", id, &pre(&allowed), 1_200)
            .unwrap();
        if id == "still_unknown" {
            store
                .mark_unknown("ses_fixture", id, "worker_lost", 1_300)
                .unwrap();
        }
    }
    // Simulate failed best-effort socket cleanup: the durable requests remain
    // pending/allowed after their owner disappears, filling the session limit.
    for n in 0..MAX_PENDING_PER_SESSION {
        let id = format!("orphan_waiter_{n}");
        let row = store.create(intent(&id), 1_000).unwrap();
        if n == 0 {
            store
                .decide("ses_fixture", &id, allow(&row), 1_100)
                .unwrap();
        }
    }
    assert!(matches!(
        store.create(intent("before_expiry"), 2_000),
        Err(Error::Capacity)
    ));
    let mut fresh = intent("after_expiry");
    fresh.created_at_ms = 601_001;
    fresh.expires_at_ms = 1_201_001;
    store.active_scans.set(0);
    assert_eq!(store.create(fresh, 601_001).unwrap().state, State::Pending);
    assert_eq!(
        store.active_scans.get(),
        1,
        "expiry reconciliation must reuse one bounded snapshot"
    );
    for n in 0..MAX_PENDING_PER_SESSION {
        let row = store
            .get("ses_fixture", &format!("orphan_waiter_{n}"))
            .unwrap();
        assert_eq!(row.state, State::Expired);
        assert!(row.dispatch_id.is_none());
    }
    assert_eq!(
        store.get("ses_fixture", "still_reserved").unwrap().state,
        State::DispatchReserved
    );
    assert_eq!(
        store.get("ses_fixture", "still_unknown").unwrap().state,
        State::Unknown
    );
}

#[test]
fn admission_cleans_committed_active_duplicates_before_physical_capacity_check() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    store.record_limit_override.set(4);
    for n in 0..3 {
        let id = format!("protected_{n}");
        let row = store.create(intent(&id), 1_000).unwrap();
        let row = store
            .decide("ses_fixture", &id, allow(&row), 1_100)
            .unwrap();
        let _ticket = store
            .reserve("ses_fixture", &id, &pre(&row), 1_200)
            .unwrap();
        store
            .mark_unknown("ses_fixture", &id, "fixture_unknown", 1_300)
            .unwrap();
    }
    let terminal = denied(&mut store, "committed_not_unlinked");
    store.archive.append(&[terminal]).unwrap();
    // Simulates a crash after commit marker publication, before active unlink.
    assert_eq!(store.active_records_raw().unwrap().len(), 4);
    assert_eq!(store.active_records().unwrap().len(), 3);
    store.create(intent("new_pending"), 1_000).unwrap();
    assert_eq!(store.active_records_raw().unwrap().len(), 4);
    assert!(!path(root.path(), "committed_not_unlinked").exists());
    assert_eq!(
        store
            .get("ses_fixture", "committed_not_unlinked")
            .unwrap()
            .state,
        State::Denied
    );
}

#[test]
fn each_publication_failure_stage_leaves_exact_visible_evidence_without_replay() {
    for stage in [
        PublishStage::FileSynced,
        PublishStage::Published,
        PublishStage::DirectorySynced,
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let value = json!({"fixture":"immutable"});
        let bytes = encode(&value, 1024).unwrap();
        let outcome = publish_hook(&store.active, "fault.json", &bytes, false, &mut |at| {
            if at == stage {
                Err(io::Error::other("fixture failure"))
            } else {
                Ok(())
            }
        });
        assert!(outcome.is_err());
        let observed: Option<Value> = read_optional(&store.active, "fault.json", 1024).unwrap();
        assert_eq!(observed.is_some(), stage != PublishStage::FileSynced);
        if let Some(observed) = observed {
            assert_eq!(observed, value);
            assert_eq!(
                std::fs::metadata(root.path().join(STORE_DIRECTORY).join("active/fault.json"))
                    .unwrap()
                    .nlink(),
                1,
                "atomic no-replace publication has no two-link crash interval"
            );
            assert!(publish(
                &store.active,
                "fault.json",
                &json!("replacement"),
                false,
                1024
            )
            .is_err());
        }
    }
}

#[test]
fn reservation_publication_failure_never_returns_a_second_dispatch_permit() {
    for stage in [
        PublishStage::FileSynced,
        PublishStage::Published,
        PublishStage::DirectorySynced,
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let row = store.create(intent("reserve_fault"), 1_000).unwrap();
        let allowed = store
            .decide("ses_fixture", "reserve_fault", allow(&row), 1_100)
            .unwrap();
        store.fail_publication_stage.set(Some(stage));
        assert!(
            store
                .reserve("ses_fixture", "reserve_fault", &pre(&allowed), 1_200)
                .is_err(),
            "no ticket may escape a failed publication"
        );
        store.fail_publication_stage.set(None);
        let observed = store.get("ses_fixture", "reserve_fault").unwrap();
        if stage == PublishStage::FileSynced {
            assert_eq!(observed.state, State::Allowed);
            assert_eq!(
                store
                    .cancel(
                        "ses_fixture",
                        "reserve_fault",
                        "pre_dispatch_refusal",
                        1_300
                    )
                    .unwrap()
                    .state,
                State::Cancelled
            );
        } else {
            assert_eq!(observed.state, State::DispatchReserved);
            assert!(matches!(
                store.reserve("ses_fixture", "reserve_fault", &pre(&allowed), 1_300),
                Err(Error::Conflict)
            ));
            store.recover_epoch("new_epoch", 1_400).unwrap();
            assert_eq!(
                store.get("ses_fixture", "reserve_fault").unwrap().state,
                State::Unknown
            );
            assert_eq!(store.archive_terminal().unwrap(), 0);
        }
    }
}

#[test]
fn interrupted_unpublished_active_temp_is_removed_under_exclusive_lease() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let temp = format!("pending-{}", ulid::Ulid::new());
    let p = root.path().join(STORE_DIRECTORY).join("active").join(temp);
    std::fs::write(&p, b"unpublished fixture").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    drop(store);
    let store = Store::open(root.path()).unwrap();
    assert!(!p.exists());
    assert!(store
        .list("ses_fixture", None, 50)
        .unwrap()
        .records
        .is_empty());
}

#[test]
fn interrupted_archive_publication_reuses_exact_page_and_consumed_ids() {
    for head_published in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(root.path()).unwrap();
        let rows: Vec<_> = (0..5)
            .map(|n| denied(&mut store, &format!("orphan_{n}")))
            .collect();
        store.archive_terminal().unwrap();
        drop(store);
        let archive = root.path().join(STORE_DIRECTORY).join("archive");
        let commits: Vec<_> = std::fs::read_dir(archive.join("commits"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(commits.len(), 1);
        std::fs::remove_file(&commits[0]).unwrap();
        if !head_published {
            std::fs::remove_file(archive.join("heads").join(record_name("ses_fixture"))).unwrap();
        }
        for row in &rows {
            let p = path(root.path(), &row.intent.approval_id);
            std::fs::write(&p, serde_json::to_vec(row).unwrap()).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut store = Store::open(root.path()).unwrap();
        // A plain read before recovery must not duplicate the active rows
        // through a head whose commit marker was never published.
        assert_eq!(
            store.list("ses_fixture", None, 50).unwrap().records.len(),
            5
        );
        assert_eq!(store.archive_terminal().unwrap(), 5);
        assert_eq!(std::fs::read_dir(archive.join("pages")).unwrap().count(), 1);
        assert_eq!(
            store.list("ses_fixture", None, 50).unwrap().records.len(),
            5
        );
        assert!(matches!(
            store.create(intent("orphan_0"), 1_000),
            Err(Error::Conflict)
        ));
    }
}

#[test]
fn missing_or_corrupt_archive_is_not_replaced_with_an_empty_history() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    denied(&mut store, "archived_one");
    store.archive_terminal().unwrap();
    drop(store);
    let pages = root.path().join(STORE_DIRECTORY).join("archive/pages");
    let page = std::fs::read_dir(pages)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&page, b"{}").unwrap();
    let mut store = Store::open(root.path()).unwrap();
    assert!(store.get("ses_fixture", "archived_one").is_err());
    assert!(store.list("ses_fixture", None, 50).is_err());
    assert!(store.create(intent("archived_one"), 1_000).is_err());
}

#[test]
fn interrupted_page_resumes_before_newly_terminal_rows_change_batch_order() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    let mut ids: Vec<_> = (0..80).map(|n| format!("shift_{n:03}")).collect();
    ids.sort_by_key(|id| record_name(id));
    let pending = store.create(intent(&ids[0]), 1_000).unwrap();
    let rows: Vec<_> = ids[1..26].iter().map(|id| denied(&mut store, id)).collect();
    store.archive_terminal().unwrap();
    drop(store);
    let archive = root.path().join(STORE_DIRECTORY).join("archive");
    let pages: Vec<Value> = std::fs::read_dir(archive.join("pages"))
        .unwrap()
        .map(|e| serde_json::from_slice(&std::fs::read(e.unwrap().path()).unwrap()).unwrap())
        .collect();
    let first = pages.iter().find(|p| p["previous"].is_null()).unwrap();
    // Stored struct ordering, rather than Value serialization, identifies file.
    let first_path = std::fs::read_dir(archive.join("pages"))
        .unwrap()
        .find_map(|e| {
            let p = e.unwrap().path();
            let v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            (v["previous"].is_null()).then_some(p)
        })
        .unwrap();
    // Recreate the point after first-page tombstones but before its head. The
    // later page is removed because it could not yet have existed then.
    for dir in ["commits", "heads"] {
        for e in std::fs::read_dir(archive.join(dir)).unwrap() {
            std::fs::remove_file(e.unwrap().path()).unwrap();
        }
    }
    let first_ids: std::collections::BTreeSet<String> = first["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["intent"]["approval_id"].as_str().unwrap().into())
        .collect();
    for row in &rows {
        if !first_ids.contains(&row.intent.approval_id) {
            std::fs::remove_file(
                archive
                    .join("ids")
                    .join(record_name(&row.intent.approval_id)),
            )
            .unwrap();
        }
        let p = path(root.path(), &row.intent.approval_id);
        std::fs::write(&p, serde_json::to_vec(row).unwrap()).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    for e in std::fs::read_dir(archive.join("pages")).unwrap() {
        let p = e.unwrap().path();
        if p != first_path {
            std::fs::remove_file(p).unwrap();
        }
    }
    let mut store = Store::open(root.path()).unwrap();
    let mut req = allow(&pending);
    req.decision = Decision::Deny;
    store
        .decide("ses_fixture", &pending.intent.approval_id, req, 1_200)
        .unwrap();
    assert_eq!(store.archive_terminal().unwrap(), 26);
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let page = store.list("ses_fixture", cursor.as_deref(), 50).unwrap();
        for row in page.records {
            assert!(seen.insert(row.intent.approval_id));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 26);
}

#[test]
#[ignore = "opt-in filesystem scalability fixture; no wall-time assertion"]
fn archived_known_outcomes_exceed_active_lifetime_count_limit() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    for n in 0..MAX_ACTIVE_RECORDS + 1 {
        denied(&mut store, &format!("lifetime_{n:05}"));
    }
    assert_eq!(
        store.get("ses_fixture", "lifetime_00000").unwrap().state,
        State::Denied
    );
    assert_eq!(
        store
            .get("ses_fixture", &format!("lifetime_{MAX_ACTIVE_RECORDS:05}"))
            .unwrap()
            .state,
        State::Denied
    );
    assert!(store.active_records().unwrap().len() < 65);
}
