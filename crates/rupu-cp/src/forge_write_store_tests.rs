use super::*;
use serde_json::json;
use std::os::unix::fs::{symlink, PermissionsExt};

const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn intent(store: &Store, id: &str, key: &str) -> Intent {
    let value = json!({"authority":{"account":"work","project":"org/repo"},"action":"issue.comment","input":{"body":"reviewed text"}});
    store
        .publish_intent(
            id,
            &digest(&value).unwrap(),
            key,
            now().unwrap() + 300,
            value,
        )
        .unwrap()
}
fn reserve(store: &Store, intent: &Intent, operation: &str) -> ReservationTicket {
    match store
        .reserve(operation, &intent.intent_id, &intent.digest)
        .unwrap()
    {
        Reservation::New { ticket, .. } => ticket,
        Reservation::Replay(_) => panic!("expected fresh reservation"),
    }
}
fn path(tmp: &tempfile::TempDir, sub: &str, id: &str) -> PathBuf {
    tmp.path().join(STORE_NAME).join(sub).join(file_name(id))
}

#[test]
fn immutable_review_digest_and_private_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "intent-1", KEY);
    assert_eq!(store.intent("intent-1").unwrap().unwrap(), i);
    assert!(store
        .publish_intent("intent-1", &i.digest, KEY, i.expires_at, i.payload.clone())
        .is_err());
    assert!(store
        .publish_intent("bad-digest", OTHER, KEY, i.expires_at, i.payload.clone())
        .is_err());
    assert_eq!(
        std::fs::metadata(path(&tmp, "intents", "intent-1"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(tmp.path().join(STORE_NAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn fresh_ticket_dispatch_and_durable_terminal_replay() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "intent-1", KEY);
    let ticket = reserve(&store, &i, "operation-1");
    assert_eq!(ticket.operation().phase, Phase::Reserved);
    let dispatched = store.mark_dispatching(ticket).unwrap();
    assert!(dispatched.operation().dispatch_marked_at.is_some());
    let completed = store
        .finish_dispatch(
            dispatched,
            Outcome::Accepted,
            "provider_acknowledged",
            json!({"id":"comment-1"}),
        )
        .unwrap();
    assert_eq!(completed.receipt_version, 3);
    match store.reserve("operation-1", "intent-1", &i.digest).unwrap() {
        Reservation::Replay(actual) => assert_eq!(actual, completed),
        Reservation::New { .. } => panic!("replay authorized dispatch"),
    }
    assert!(store
        .reserve("operation-1", "different", &i.digest)
        .is_err());
    assert!(store.reserve("operation-1", "intent-1", OTHER).is_err());
    assert!(store.reserve("operation-2", "intent-1", &i.digest).is_err());
}

#[test]
fn lease_contention_and_unleased_status_are_distinct() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "i", KEY);
    let ticket = reserve(&store, &i, "op");
    assert!(matches!(Store::open(tmp.path()),Err(e) if e.kind()==io::ErrorKind::WouldBlock));
    let reader = Reader::open(tmp.path()).unwrap();
    let status = reader.operation("op").unwrap().unwrap();
    assert!(status.writer_active);
    assert_eq!(status.observed_outcome, ObservedOutcome::InProgress);
    let final_row = store
        .finish_not_submitted(ticket, "preflight_changed", Value::Null)
        .unwrap();
    assert_eq!(final_row.outcome, Some(Outcome::NotSubmitted));
    let status = reader.operation("op").unwrap().unwrap();
    assert_eq!(status.observed_outcome, ObservedOutcome::InProgress);
    assert_eq!(status.durability, "unconfirmed");
    drop(store);
    let status = reader.operation("op").unwrap().unwrap();
    assert_eq!(status.observed_outcome, ObservedOutcome::NotSubmitted);
    assert_eq!(status.durability, "observed");
}

#[test]
fn restart_never_produces_dispatch_tickets_and_preserves_phase_evidence() {
    for dispatch in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let i = intent(&store, "i", KEY);
        let ticket = reserve(&store, &i, "op");
        if dispatch {
            drop(store.mark_dispatching(ticket).unwrap());
        } else {
            drop(ticket);
        }
        drop(store);
        let read = Reader::open(tmp.path())
            .unwrap()
            .operation("op")
            .unwrap()
            .unwrap();
        assert_eq!(
            read.observed_outcome,
            if dispatch {
                ObservedOutcome::Unknown
            } else {
                ObservedOutcome::NotSubmitted
            }
        );
        let reopened = Store::open(tmp.path()).unwrap();
        let row = reopened.operation("op").unwrap().unwrap();
        assert_eq!(
            row.outcome,
            Some(if dispatch {
                Outcome::Unknown
            } else {
                Outcome::NotSubmitted
            })
        );
        let recovery = row.recovery.as_ref().unwrap();
        assert_eq!(
            recovery.from_phase,
            if dispatch {
                Phase::Dispatching
            } else {
                Phase::Reserved
            }
        );
        assert!(recovery.recovered_at >= recovery.observed_updated_at);
        assert!(matches!(
            reopened.reserve("op", "i", &i.digest).unwrap(),
            Reservation::Replay(_)
        ));
    }
}

#[test]
fn unknown_blocks_equivalent_new_intent_but_known_refusal_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "i", KEY);
    let dispatch = store.mark_dispatching(reserve(&store, &i, "op")).unwrap();
    store
        .finish_dispatch(
            dispatch,
            Outcome::Unknown,
            "timeout_after_dispatch",
            Value::Null,
        )
        .unwrap();
    assert!(store
        .publish_intent(
            "other",
            &i.digest,
            KEY,
            now().unwrap() + 300,
            i.payload.clone()
        )
        .is_err());
    let independent = intent(&store, "independent", OTHER);
    store
        .finish_not_submitted(
            reserve(&store, &independent, "op2"),
            "changed_before_dispatch",
            Value::Null,
        )
        .unwrap();
    assert!(store
        .publish_intent(
            "new",
            &independent.digest,
            OTHER,
            now().unwrap() + 300,
            independent.payload.clone()
        )
        .is_ok());
}

#[test]
fn old_ticket_cannot_cross_new_lease_and_recovery_does_not_rebind() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "i", KEY);
    let old = reserve(&store, &i, "op");
    drop(store);
    let replacement = Store::open(tmp.path()).unwrap();
    assert!(replacement.mark_dispatching(old).is_err());
    assert_eq!(
        replacement.operation("op").unwrap().unwrap().outcome,
        Some(Outcome::NotSubmitted)
    );
}

#[test]
fn expired_review_is_not_executable_but_existing_receipt_is_replayable() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "i", KEY);
    let ticket = reserve(&store, &i, "op");
    store
        .finish_not_submitted(ticket, "preflight_refused", Value::Null)
        .unwrap();
    // Test immutable stored timestamps, not wall-clock sleeps.
    let mut expired = i.clone();
    expired.created_at = 1;
    expired.expires_at = 2;
    publish_bytes(
        &store.reader.intents,
        "i.json",
        &encode(&expired).unwrap(),
        true,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert!(matches!(
        store.reserve("op", "i", &i.digest).unwrap(),
        Reservation::Replay(_)
    ));
    assert!(store.reserve("new-op", "i", &i.digest).is_err());
    let mut unused = expired;
    unused.intent_id = "unused".into();
    publish_bytes(
        &store.reader.intents,
        "unused.json",
        &encode(&unused).unwrap(),
        false,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert!(store
        .reserve("unused-op", "unused", &unused.digest)
        .is_err());
}

#[test]
fn publication_failure_boundaries_never_issue_ticket_after_uncertain_reservation() {
    for stage in [
        PublishStage::FileSynced,
        PublishStage::Published,
        PublishStage::DirectorySynced,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let i = intent(&store, "i", KEY);
        assert!(store
            .reserve_with("op", "i", &i.digest, &mut |at| if at == stage {
                Err(io::Error::other("injected"))
            } else {
                Ok(())
            })
            .is_err());
        let row = store.operation("op").unwrap();
        if stage == PublishStage::FileSynced {
            assert!(row.is_none());
        } else {
            assert_eq!(row.unwrap().phase, Phase::Reserved);
            assert!(matches!(
                store.reserve("op", "i", &i.digest).unwrap(),
                Reservation::Replay(_)
            ));
        }
    }
}

#[test]
fn dispatch_marker_faults_consume_ticket_and_preserve_uncertainty() {
    for stage in [
        PublishStage::FileSynced,
        PublishStage::Published,
        PublishStage::DirectorySynced,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let i = intent(&store, "i", KEY);
        let ticket = reserve(&store, &i, "op");
        assert!(store
            .mark_dispatching_with(ticket, &mut |at| if at == stage {
                Err(io::Error::other("injected"))
            } else {
                Ok(())
            })
            .is_err());
        assert!(matches!(
            store.reserve("op", "i", &i.digest).unwrap(),
            Reservation::Replay(_)
        ));
        drop(store);
        let reopened = Store::open(tmp.path()).unwrap();
        assert_eq!(
            reopened.operation("op").unwrap().unwrap().outcome,
            Some(if stage == PublishStage::FileSynced {
                Outcome::NotSubmitted
            } else {
                Outcome::Unknown
            })
        );
    }
}

#[test]
fn lost_terminal_receipt_preserves_marker_or_known_published_evidence() {
    for stage in [
        PublishStage::FileSynced,
        PublishStage::Published,
        PublishStage::DirectorySynced,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let i = intent(&store, "i", KEY);
        let ticket = store.mark_dispatching(reserve(&store, &i, "op")).unwrap();
        assert!(store
            .finish_dispatch_with(
                ticket,
                Outcome::Accepted,
                "provider_acknowledged",
                json!({"id":"created-42"}),
                &mut |at| if at == stage {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            )
            .is_err());
        drop(store);
        let recovered = Store::open(tmp.path()).unwrap();
        let row = recovered.operation("op").unwrap().unwrap();
        if stage == PublishStage::FileSynced {
            assert_eq!(row.outcome, Some(Outcome::Unknown));
        } else {
            assert_eq!(row.outcome, Some(Outcome::Accepted));
            assert_eq!(row.result["id"], "created-42");
        }
    }
}

#[test]
fn compaction_preserves_full_terminal_review_replay_and_unknown_snapshots() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let known = intent(&store, "known", KEY);
    let unknown = intent(&store, "unknown", OTHER);
    store
        .finish_not_submitted(
            reserve(&store, &known, "known-op"),
            "preflight_refused",
            Value::Null,
        )
        .unwrap();
    store
        .finish_dispatch(
            store
                .mark_dispatching(reserve(&store, &unknown, "unknown-op"))
                .unwrap(),
            Outcome::Unknown,
            "timeout",
            Value::Null,
        )
        .unwrap();
    assert_eq!(store.compact_intents(now().unwrap() + 1000).unwrap(), 1);
    assert!(!path(&tmp, "intents", "known").exists());
    assert!(path(&tmp, "intents", "unknown").exists());
    assert_eq!(store.intent("known").unwrap().unwrap(), known);
    assert!(matches!(
        store.reserve("known-op", "known", &known.digest).unwrap(),
        Reservation::Replay(_)
    ));
    assert!(store
        .publish_intent(
            "known",
            &known.digest,
            KEY,
            now().unwrap() + 300,
            known.payload.clone()
        )
        .is_err());
}

#[test]
fn capacity_is_refusal_not_deletion_or_uncertainty_override() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    for n in 0..MAX_UNUSED_INTENTS {
        intent(&store, &format!("intent-{n}"), KEY);
    }
    let value = json!({"text":"same"});
    assert!(store
        .publish_intent(
            "one-too-many",
            &digest(&value).unwrap(),
            KEY,
            now().unwrap() + 300,
            value
        )
        .is_err());
    assert_eq!(
        names(&store.reader.intents, MAX_INTENTS).unwrap().len(),
        MAX_UNUSED_INTENTS
    );
    assert!(digest(&json!({"text":"x".repeat(MAX_RECORD_BYTES as usize)})).is_err());
}

#[test]
fn store_refuses_symlink_hardlink_special_file_and_permissive_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "i", KEY);
    drop(store);
    let original = path(&tmp, "intents", "i");
    std::fs::hard_link(&original, tmp.path().join("outside-link")).unwrap();
    assert!(Reader::open(tmp.path()).unwrap().intent("i").is_err());
    std::fs::remove_file(tmp.path().join("outside-link")).unwrap();
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Reader::open(tmp.path()).unwrap().intent("i").is_err());
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
    let outside = tmp.path().join("outside");
    std::fs::rename(&original, &outside).unwrap();
    symlink(&outside, &original).unwrap();
    assert!(Reader::open(tmp.path()).unwrap().intent("i").is_err());
    std::fs::remove_file(&original).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(&original)
        .status()
        .unwrap()
        .success());
    assert!(Reader::open(tmp.path()).unwrap().intent("i").is_err());
    assert!(!i.digest.is_empty());
}

#[test]
fn changed_root_and_changed_lease_do_not_preserve_write_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "i", KEY);
    let old = tmp.path().join(STORE_NAME);
    std::fs::rename(&old, tmp.path().join("moved")).unwrap();
    assert!(store.reserve("op", "i", &i.digest).is_err());
    std::fs::rename(tmp.path().join("moved"), &old).unwrap();
    std::fs::rename(old.join("lease"), old.join("old-lease")).unwrap();
    std::fs::write(old.join("lease"), b"").unwrap();
    std::fs::set_permissions(old.join("lease"), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.reserve("op", "i", &i.digest).is_err());
}

#[test]
fn crash_leftover_publication_link_is_cleaned_without_losing_record() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let i = intent(&store, "i", KEY);
    let pending = tmp
        .path()
        .join(STORE_NAME)
        .join("intents")
        .join(format!("pending-{}", ulid::Ulid::new()));
    std::fs::hard_link(path(&tmp, "intents", "i"), &pending).unwrap();
    drop(store);
    let reopened = Store::open(tmp.path()).unwrap();
    assert!(!pending.exists());
    assert_eq!(reopened.intent("i").unwrap().unwrap(), i);
}

#[test]
fn independent_process_cannot_acquire_parent_lease() {
    const PROBE: &str = "RUPU_FORGE_STORE_LEASE_PROBE";
    if let Some(path) = std::env::var_os(PROBE) {
        assert!(
            matches!(Store::open(Path::new(&path)),Err(e) if e.kind()==io::ErrorKind::WouldBlock)
        );
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let _lease = Store::open(tmp.path()).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("forge_write_store::tests::independent_process_cannot_acquire_parent_lease")
        .arg("--exact")
        .env(PROBE, tmp.path())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn store_and_phase_tickets_can_transfer_ownership_between_tasks() {
    fn assert_send<T: Send>() {}
    assert_send::<Store>();
    assert_send::<ReservationTicket>();
    assert_send::<DispatchTicket>();
    // Store contains PhantomData<Cell<()>>. Cell's negative Sync property
    // prevents sharing &Store across threads; no unsafe Sync impl exists.
}

#[test]
fn preflight_crossing_review_expiry_cannot_receive_dispatch_ticket() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let review = intent(&store, "expiring", KEY);
    let ticket = reserve(&store, &review, "expiring-op");
    assert!(store
        .mark_dispatching_at(ticket, review.expires_at, &mut |_| Ok(()))
        .is_err());
    let operation = store.operation("expiring-op").unwrap().unwrap();
    assert_eq!(operation.outcome, Some(Outcome::NotSubmitted));
    assert_eq!(
        operation.reason.as_deref(),
        Some("review_expired_before_dispatch")
    );
    assert!(operation.dispatch_marked_at.is_none());
    assert!(matches!(
        store
            .reserve("expiring-op", "expiring", &review.digest)
            .unwrap(),
        Reservation::Replay(_)
    ));
    // Explicit refusal after expiry remains permitted; expiry guards dispatch,
    // not persistence of a known pre-dispatch outcome.
    let other = intent(&store, "refused", OTHER);
    let mut ticket = reserve(&store, &other, "refused-op");
    ticket.operation.intent.created_at = 1;
    ticket.operation.intent.expires_at = 2;
    publish_bytes(
        &store.reader.operations,
        "refused-op.json",
        &encode(&ticket.operation).unwrap(),
        true,
        &mut |_| Ok(()),
    )
    .unwrap();
    let operation = store
        .finish_not_submitted(ticket, "preflight_refused", Value::Null)
        .unwrap();
    assert_eq!(operation.outcome, Some(Outcome::NotSubmitted));
}
