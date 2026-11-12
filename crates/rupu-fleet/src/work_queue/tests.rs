use super::{
    disk::{self, Store},
    mutation::FAULT,
    types::*,
};
use crate::{Board, Bus, Member, OperatorVia};
use std::{fs, path::Path};

fn fixture() -> tempfile::TempDir {
    tempfile::tempdir_in("/private/tmp").unwrap()
}
fn operator(root: &Path) -> Bus {
    Bus::open(root, "operator")
}
fn agent(root: &Path, address: &str, run: &str, execution: &str, round: u32) -> (Bus, WorkContext) {
    let bus = Bus::open(root, address);
    bus.join(Member {
        agent: "reviewer".into(),
        run_id: run.into(),
        work_execution_id: Some(execution.into()),
        kind: "session_turn".into(),
        ..Default::default()
    })
    .unwrap();
    let context = WorkContext {
        actor: WorkActor::Agent {
            address: address.into(),
            run_id: run.into(),
            execution_id: execution.into(),
            agent: "reviewer".into(),
            recording: WorkRecording {
                anchor: WorkRecordingAnchor::Owner,
                relative_path: format!("round-{round}/transcript.jsonl"),
                workspace_id: None,
                round: Some(round),
            },
        },
        source: Some(WorkSource {
            turn_index: 0,
            call_sequence: 1,
            tool_call_id: "call-original".into(),
        }),
    };
    (bus, context)
}
fn post(bus: &Bus, title: &str, instructions: &str) -> WorkReply {
    let snapshot = bus.board.work_snapshot().unwrap();
    bus.operator_work_apply(
        WorkCommand {
            request_id: format!("request_{}", ulid::Ulid::new()),
            expected_queue_id: snapshot.queue_id,
            expected_revision: snapshot.revision,
            operation: WorkOperation::Post {
                title: title.into(),
                instructions: instructions.into(),
            },
        },
        OperatorVia::Cp,
        || Ok(()),
        || Ok(()),
    )
    .unwrap()
}
fn command(reply: &WorkReply, operation: WorkOperation) -> WorkCommand {
    WorkCommand {
        request_id: format!("request_{}", ulid::Ulid::new()),
        expected_queue_id: Some(reply.receipt.queue_id.clone()),
        expected_revision: reply.current_item.revision,
        operation,
    }
}
fn apply(bus: &Bus, context: &WorkContext, command: WorkCommand) -> WorkReply {
    bus.work_apply(command, context.clone(), || Ok(()), || Ok(()))
        .unwrap()
}
fn timeline(board: &Board, id: &str) -> WorkTimelinePage {
    board
        .work_timeline(WorkTimelineQuery {
            work_id: id.into(),
            limit: 200,
            cursor: None,
        })
        .unwrap()
}

#[test]
fn missing_reads_do_not_create_or_reinterpret_legacy_work() {
    let dir = fixture();
    let root = dir.path().join("messages");
    let board = Board::new(&root);
    assert_eq!(
        board.work_snapshot().unwrap().availability,
        WorkAvailability::NotInitialized
    );
    assert!(!root.exists());
    let bus = Bus::open(&root, "legacy");
    let _claim = bus
        .board
        .claim("legacy-work", "legacy", std::time::Duration::from_secs(60))
        .unwrap();
    assert_eq!(board.work_snapshot().unwrap().capacity.item_count, 0);
    assert!(!root.join("board").join(disk::DATABASE_NAME).exists());
}

#[test]
fn lifecycle_preserves_attempts_and_exact_execution_sources() {
    let dir = fixture();
    let op = operator(dir.path());
    let (bus, context) = agent(dir.path(), "worker", "run_one", "execution_one", 1);
    let posted = post(&op, "Review storage", "Inspect the transaction boundary.");
    let id = posted.current_item.work_id.clone();
    let claimed = apply(
        &bus,
        &context,
        command(
            &posted,
            WorkOperation::Claim {
                work_id: id.clone(),
            },
        ),
    );
    let attempt = claimed
        .current_item
        .latest_attempt
        .as_ref()
        .unwrap()
        .work_attempt_id
        .clone();
    assert!(claimed
        .current_item
        .latest_attempt
        .as_ref()
        .unwrap()
        .executor
        .is_none());
    let started = apply(
        &bus,
        &context,
        command(
            &claimed,
            WorkOperation::Start {
                work_id: id.clone(),
                work_attempt_id: attempt.clone(),
            },
        ),
    );
    let blocked = apply(
        &bus,
        &context,
        command(
            &started,
            WorkOperation::Block {
                work_id: id.clone(),
                work_attempt_id: attempt.clone(),
                reason: "Need a decision".into(),
            },
        ),
    );
    let resumed = apply(
        &bus,
        &context,
        command(
            &blocked,
            WorkOperation::Resume {
                work_id: id.clone(),
                work_attempt_id: attempt.clone(),
            },
        ),
    );
    let done = apply(
        &bus,
        &context,
        command(
            &resumed,
            WorkOperation::Complete {
                work_id: id.clone(),
                work_attempt_id: attempt.clone(),
                summary: "Reviewed the storage implementation.".into(),
            },
        ),
    );
    assert_eq!(done.current_item.state, WorkState::Completed);
    assert_eq!(
        done.current_item.latest_attempt.as_ref().unwrap().closure,
        Some(WorkAttemptClosure::Completed)
    );
    let requeued = op
        .operator_work_apply(
            command(
                &done,
                WorkOperation::Requeue {
                    work_id: id.clone(),
                    reason: "Review the next revision".into(),
                },
            ),
            OperatorVia::Cp,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(
        requeued
            .current_item
            .latest_attempt
            .as_ref()
            .unwrap()
            .closure,
        Some(WorkAttemptClosure::Completed)
    );
    assert!(!requeued.receipt.prior_attempt_may_still_run);
    let next = apply(
        &bus,
        &context,
        command(
            &requeued,
            WorkOperation::Claim {
                work_id: id.clone(),
            },
        ),
    );
    assert_ne!(
        next.current_item
            .latest_attempt
            .as_ref()
            .unwrap()
            .work_attempt_id,
        attempt
    );
    let events = timeline(&op.board, &id);
    assert_eq!(events.events.len(), 8);
    assert_eq!(events.counts.matching_total, 8);
    assert_eq!(events.snapshot.revision, next.queue_revision);
    assert_eq!(events.events[5].source, context.source);
    assert_eq!(
        op.board.work_get_snapshot(&id).unwrap().snapshot.revision,
        next.queue_revision
    );
}

#[test]
fn retry_returns_immutable_receipt_and_current_item_before_liveness_checks() {
    let dir = fixture();
    let op = operator(dir.path());
    let (bus, context) = agent(dir.path(), "worker", "run_one", "execution_one", 1);
    let posted = post(&op, "Review", "Inspect implementation");
    let id = posted.current_item.work_id.clone();
    let claim = command(
        &posted,
        WorkOperation::Claim {
            work_id: id.clone(),
        },
    );
    let first = apply(&bus, &context, claim.clone());
    let attempt = first.receipt.work_attempt_id.clone().unwrap();
    let started = apply(
        &bus,
        &context,
        command(
            &first,
            WorkOperation::Start {
                work_id: id.clone(),
                work_attempt_id: attempt,
            },
        ),
    );
    bus.leave("done").unwrap();
    let mut retried = context.clone();
    retried.source.as_mut().unwrap().tool_call_id = "call-new".into();
    let replay = bus
        .work_apply(
            claim.clone(),
            retried,
            || Ok(()),
            || panic!("replay must precede liveness"),
        )
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(replay.current_item.state, WorkState::Working);
    assert_eq!(replay.queue_revision, started.queue_revision);
    let mut rebound = claim;
    rebound.expected_revision += 1;
    assert!(matches!(
        bus.work_apply(rebound, context.clone(), || Ok(()), || Ok(())),
        Err(WorkError::IdempotencyConflict { .. })
    ));
    assert!(bus
        .work_apply(
            command(
                &started,
                WorkOperation::Block {
                    work_id: id,
                    work_attempt_id: started.receipt.work_attempt_id.clone().unwrap(),
                    reason: "wait".into()
                }
            ),
            context,
            || Ok(()),
            || Ok(())
        )
        .is_err());
}

#[test]
fn same_run_different_execution_and_stale_leave_cannot_inherit_attempt() {
    let dir = fixture();
    let op = operator(dir.path());
    let (old, old_context) = agent(dir.path(), "lead", "same_run", "execution_one", 1);
    let posted = post(&op, "Review", "First round");
    let id = posted.current_item.work_id.clone();
    let claimed = apply(
        &old,
        &old_context,
        command(
            &posted,
            WorkOperation::Claim {
                work_id: id.clone(),
            },
        ),
    );
    let (new, new_context) = agent(dir.path(), "lead", "same_run", "execution_two", 2);
    old.leave("finished old round").unwrap();
    assert!(new.roster().unwrap().get("lead").unwrap().live);
    let start = command(
        &claimed,
        WorkOperation::Start {
            work_id: id.clone(),
            work_attempt_id: claimed.receipt.work_attempt_id.clone().unwrap(),
        },
    );
    assert!(matches!(
        new.work_apply(start.clone(), new_context, || Ok(()), || Ok(())),
        Err(WorkError::AttemptConflict)
    ));
    assert!(matches!(
        old.work_apply(start, old_context, || Ok(()), || Ok(())),
        Err(WorkError::Denied(_))
    ));
    let requeued = op
        .operator_work_apply(
            command(
                &claimed,
                WorkOperation::Requeue {
                    work_id: id.clone(),
                    reason: "recover abandoned attempt".into(),
                },
            ),
            OperatorVia::Cp,
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    assert!(requeued.receipt.prior_attempt_may_still_run);
    assert!(requeued.current_item.prior_attempt_may_still_run);
    assert_eq!(
        requeued.current_item.latest_attempt.unwrap().closure,
        Some(WorkAttemptClosure::Requeued)
    );
}

#[test]
fn strict_roster_and_actor_shape_fail_closed() {
    use std::io::Write;
    let dir = fixture();
    let op = operator(dir.path());
    let (bus, context) = agent(dir.path(), "worker", "run_one", "execution_one", 1);
    let posted = post(&op, "Review", "Check strict authority");
    let cmd = command(
        &posted,
        WorkOperation::Claim {
            work_id: posted.current_item.work_id.clone(),
        },
    );
    let mut forged = context.clone();
    if let WorkActor::Agent { execution_id, .. } = &mut forged.actor {
        *execution_id = "another_execution".into();
    }
    assert!(matches!(
        bus.work_apply(cmd.clone(), forged, || Ok(()), || Ok(())),
        Err(WorkError::Denied(_))
    ));
    fs::OpenOptions::new()
        .append(true)
        .open(bus.participants_path())
        .unwrap()
        .write_all(b"{broken}\n")
        .unwrap();
    assert!(matches!(
        bus.work_apply(cmd, context, || Ok(()), || Ok(())),
        Err(WorkError::Corrupt(_))
    ));
    assert_eq!(
        op.board
            .work_get(&posted.current_item.work_id)
            .unwrap()
            .state,
        WorkState::Queued
    );
}

#[test]
fn transaction_faults_never_split_receipt_from_state() {
    let dir = fixture();
    let op = operator(dir.path());
    let posted = post(&op, "Review", "Crash consistency");
    let cmd = command(
        &posted,
        WorkOperation::Cancel {
            work_id: posted.current_item.work_id.clone(),
            reason: "test cancellation".into(),
        },
    );
    FAULT.with(|f| f.set(1));
    let before = op.operator_work_apply(cmd.clone(), OperatorVia::Cp, || Ok(()), || Ok(()));
    FAULT.with(|f| f.set(0));
    assert!(matches!(before, Err(WorkError::Persistence(_))));
    assert_eq!(
        op.board
            .work_get(&posted.current_item.work_id)
            .unwrap()
            .state,
        WorkState::Queued
    );
    FAULT.with(|f| f.set(2));
    let after = op.operator_work_apply(cmd.clone(), OperatorVia::Cp, || Ok(()), || Ok(()));
    FAULT.with(|f| f.set(0));
    assert!(matches!(after, Err(WorkError::Reconciliation { .. })));
    let recovered = operator(dir.path())
        .operator_work_apply(
            cmd,
            OperatorVia::Cp,
            || Ok(()),
            || panic!("committed retry"),
        )
        .unwrap();
    assert!(recovered.replayed);
    assert_eq!(recovered.current_item.state, WorkState::Cancelled);
    assert_eq!(
        timeline(&op.board, &posted.current_item.work_id)
            .events
            .len(),
        2
    );
}

#[test]
fn escaped_legal_text_and_capacity_reserve_allow_terminal_result() {
    let dir = fixture();
    let op = operator(dir.path());
    let (bus, context) = agent(dir.path(), "worker", "run_one", "execution_one", 1);
    let instructions = "\u{1}".repeat(MAX_INSTRUCTIONS_BYTES);
    let posted = post(&op, "Escaped text", &instructions);
    assert_eq!(posted.current_item.instructions, instructions);
    let id = posted.current_item.work_id.clone();
    let claimed = apply(
        &bus,
        &context,
        command(
            &posted,
            WorkOperation::Claim {
                work_id: id.clone(),
            },
        ),
    );
    let attempt = claimed.receipt.work_attempt_id.clone().unwrap();
    let started = apply(
        &bus,
        &context,
        command(
            &claimed,
            WorkOperation::Start {
                work_id: id.clone(),
                work_attempt_id: attempt.clone(),
            },
        ),
    );
    let store = Store::open(dir.path(), true).unwrap().unwrap();
    let snapshot = op.board.work_snapshot().unwrap();
    let cap = snapshot.capacity.database_bytes + snapshot.capacity.reserved_bytes;
    store
        .conn
        .execute("UPDATE meta SET max_bytes=?1", [cap as i64])
        .unwrap();
    drop(store);
    let summary = "\u{1}".repeat(MAX_RESULT_BYTES);
    let done = apply(
        &bus,
        &context,
        command(
            &started,
            WorkOperation::Complete {
                work_id: id,
                work_attempt_id: attempt,
                summary: summary.clone(),
            },
        ),
    );
    assert_eq!(
        done.current_item.latest_event.summary.as_deref(),
        Some(summary.as_str())
    );
    let after = op.board.work_snapshot().unwrap();
    assert_eq!(after.capacity.reserved_bytes, 0);
    assert!(after.capacity.database_bytes <= cap);
}

#[test]
fn indexed_pages_cover_more_than_two_thousand_items_and_reject_stale_cursors() {
    let dir = fixture();
    let mut store = Store::open(dir.path(), true).unwrap().unwrap();
    let context = WorkContext {
        actor: WorkActor::Operator {
            via: OperatorVia::Cp,
        },
        source: None,
    };
    let mut queue_id = None;
    for index in 0..2_005 {
        let reply = store
            .apply(
                &WorkCommand {
                    request_id: format!("post_{index}"),
                    expected_queue_id: queue_id.clone(),
                    expected_revision: index,
                    operation: WorkOperation::Post {
                        title: format!("Item {index}"),
                        instructions: "Recorded through the real Queue store".into(),
                    },
                },
                &context,
            )
            .unwrap();
        queue_id = Some(reply.receipt.queue_id);
    }
    let board = Board::new(dir.path());
    let mut cursor = None;
    let mut found = 0;
    let mut first_cursor = None;
    loop {
        let page = board
            .work_list(WorkPageQuery {
                filters: WorkFilters::default(),
                limit: 200,
                cursor,
            })
            .unwrap();
        assert_eq!(page.counts.matching_total, 2_005);
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_PAGE_BYTES);
        found += page.items.len();
        if first_cursor.is_none() {
            first_cursor = page.next.clone();
        }
        if !page.has_more {
            break;
        }
        cursor = page.next;
    }
    assert_eq!(found, 2_005);
    let plan:String=store.conn.query_row("EXPLAIN QUERY PLAN SELECT summary FROM items WHERE state='queued' AND creation_sequence<100 ORDER BY creation_sequence DESC LIMIT 50",[],|r|r.get(3)).unwrap();
    assert!(plan.contains("INDEX"), "{plan}");
    post(&operator(dir.path()), "Later", "Make cursor stale");
    assert!(matches!(
        board.work_list(WorkPageQuery {
            filters: WorkFilters::default(),
            limit: 200,
            cursor: first_cursor
        }),
        Err(WorkError::CursorConflict)
    ));
}

#[test]
fn unsafe_files_corruption_and_generation_replacement_are_not_empty_queues() {
    use std::os::unix::fs::symlink;
    let dir = fixture();
    let op = operator(dir.path());
    let posted = post(&op, "Review", "Validate storage");
    let database = dir.path().join("board").join(disk::DATABASE_NAME);
    let old = database.with_extension("saved");
    fs::rename(&database, &old).unwrap();
    symlink(&old, &database).unwrap();
    assert!(op.board.work_snapshot().is_err());
    fs::remove_file(&database).unwrap();
    fs::hard_link(&old, &database).unwrap();
    assert!(op.board.work_snapshot().is_err());
    fs::remove_file(&database).unwrap();
    let newer = post(
        &op,
        "New generation",
        "New store after explicit replacement",
    );
    assert_ne!(posted.receipt.queue_id, newer.receipt.queue_id);
    assert!(matches!(
        op.operator_work_apply(
            command(
                &posted,
                WorkOperation::Cancel {
                    work_id: posted.current_item.work_id.clone(),
                    reason: "stale".into()
                }
            ),
            OperatorVia::Cp,
            || Ok(()),
            || Ok(())
        ),
        Err(WorkError::OwnerChanged)
    ));
    fs::write(&database, b"not sqlite").unwrap();
    assert!(op.board.work_snapshot().is_err());
}

// Child mode executes the production Bus/SQLite mutation in another process.
#[test]
fn process_worker() {
    let Ok(root) = std::env::var("RUPU_QUEUE_PROCESS_ROOT") else {
        return;
    };
    if std::env::var("RUPU_QUEUE_PROCESS_MODE").as_deref() == Ok("hot_journal") {
        let database = Path::new(&root).join("board").join(disk::DATABASE_NAME);
        let conn = rusqlite::Connection::open(database).unwrap();
        conn.execute_batch("PRAGMA cache_size=1; BEGIN IMMEDIATE; UPDATE items SET summary=summary || 'uncommitted';").unwrap();
        // Deliberately skip Connection::drop: a real exited writer leaves the
        // rollback journal that the public Bus must recover before replay.
        std::process::exit(0);
    }
    let request: WorkCommand =
        serde_json::from_str(&std::env::var("RUPU_QUEUE_PROCESS_REQUEST").unwrap()).unwrap();
    let actor = std::env::var("RUPU_QUEUE_PROCESS_ACTOR").unwrap();
    let output = std::env::var("RUPU_QUEUE_PROCESS_OUTPUT").unwrap();
    let (bus, context) = agent(
        Path::new(&root),
        &actor,
        &actor,
        &format!("execution_{actor}"),
        1,
    );
    let result = bus.work_apply(request, context, || Ok(()), || Ok(()));
    fs::write(
        output,
        match result {
            Ok(reply) => serde_json::to_string(&reply).unwrap(),
            Err(e) => format!("error:{e}"),
        },
    )
    .unwrap();
}

#[test]
fn separate_processes_have_one_claim_winner() {
    let dir = fixture();
    let op = operator(dir.path());
    let posted = post(&op, "Review", "Concurrent processes");
    let cmd = command(
        &posted,
        WorkOperation::Claim {
            work_id: posted.current_item.work_id.clone(),
        },
    );
    let mut children = Vec::new();
    for index in 0..2 {
        let output = dir.path().join(format!("result_{index}"));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("work_queue::tests::process_worker")
            .arg("--exact")
            .arg("--nocapture")
            .env("RUPU_QUEUE_PROCESS_ROOT", dir.path())
            .env(
                "RUPU_QUEUE_PROCESS_REQUEST",
                serde_json::to_string(&cmd).unwrap(),
            )
            .env("RUPU_QUEUE_PROCESS_ACTOR", format!("worker_{index}"))
            .env("RUPU_QUEUE_PROCESS_OUTPUT", &output)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        children.push((child, output));
    }
    let mut successes = 0;
    for (mut child, output) in children {
        assert!(child.wait().unwrap().success());
        if fs::read_to_string(output).unwrap().starts_with('{') {
            successes += 1;
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(
        op.board
            .work_get(&posted.current_item.work_id)
            .unwrap()
            .attempt_count,
        1
    );
}

#[test]
fn one_shared_bus_replays_old_execution_after_join_changes() {
    let dir = fixture();
    let op = operator(dir.path());
    let (bus, old_context) = agent(dir.path(), "lead", "same_run", "old_execution", 1);
    let posted = post(&op, "Shared Bus", "Replay across lead round replacement");
    let cmd = command(
        &posted,
        WorkOperation::Claim {
            work_id: posted.current_item.work_id.clone(),
        },
    );
    let claimed = apply(&bus, &old_context, cmd.clone());
    bus.join(Member {
        agent: "reviewer".into(),
        run_id: "same_run".into(),
        work_execution_id: Some("new_execution".into()),
        kind: "flow_lead".into(),
        ..Default::default()
    })
    .unwrap();
    let retried = bus
        .work_apply(
            cmd,
            old_context.clone(),
            || Ok(()),
            || panic!("historical receipt does not need current execution"),
        )
        .unwrap();
    assert_eq!(retried.receipt, claimed.receipt);
    assert!(retried.replayed);
    let next = command(
        &claimed,
        WorkOperation::Start {
            work_id: claimed.current_item.work_id.clone(),
            work_attempt_id: claimed.receipt.work_attempt_id.clone().unwrap(),
        },
    );
    assert!(matches!(
        bus.work_apply(next, old_context, || Ok(()), || Ok(())),
        Err(WorkError::Denied(_))
    ));
}

#[test]
fn public_bus_recovers_hot_journal_before_historical_receipt_replay() {
    let dir = fixture();
    let op = operator(dir.path());
    let cmd = WorkCommand {
        request_id: "original_post".into(),
        expected_queue_id: None,
        expected_revision: 0,
        operation: WorkOperation::Post {
            title: "Crash recovery".into(),
            instructions: "x".repeat(MAX_INSTRUCTIONS_BYTES),
        },
    };
    let original = op
        .operator_work_apply(cmd.clone(), OperatorVia::Cp, || Ok(()), || Ok(()))
        .unwrap();
    for _ in 0..4 {
        post(&op, "Fill pages", &"x".repeat(MAX_INSTRUCTIONS_BYTES));
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("work_queue::tests::process_worker")
        .arg("--exact")
        .env("RUPU_QUEUE_PROCESS_ROOT", dir.path())
        .env("RUPU_QUEUE_PROCESS_MODE", "hot_journal")
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(dir
        .path()
        .join("board")
        .join(format!("{}-journal", disk::DATABASE_NAME))
        .exists());
    assert!(matches!(
        op.board.work_snapshot(),
        Err(WorkError::ReadOnly(_))
    ));
    let recovered = op
        .operator_work_apply(
            cmd,
            OperatorVia::Cp,
            || Ok(()),
            || panic!("existing receipt replay"),
        )
        .unwrap();
    assert!(recovered.replayed);
    assert_eq!(recovered.receipt, original.receipt);
    assert_eq!(op.board.work_snapshot().unwrap().capacity.item_count, 5);
    let page = op
        .board
        .work_list(WorkPageQuery {
            filters: WorkFilters::default(),
            limit: 50,
            cursor: None,
        })
        .unwrap();
    assert_eq!(page.items.len(), 5);
}

#[test]
fn separate_process_retry_has_one_attempt_and_identical_receipts() {
    let dir = fixture();
    let op = operator(dir.path());
    let posted = post(&op, "Review", "Concurrent identical request");
    let cmd = command(
        &posted,
        WorkOperation::Claim {
            work_id: posted.current_item.work_id.clone(),
        },
    );
    let mut children = Vec::new();
    for index in 0..2 {
        let output = dir.path().join(format!("retry_{index}"));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("work_queue::tests::process_worker")
            .arg("--exact")
            .env("RUPU_QUEUE_PROCESS_ROOT", dir.path())
            .env(
                "RUPU_QUEUE_PROCESS_REQUEST",
                serde_json::to_string(&cmd).unwrap(),
            )
            .env("RUPU_QUEUE_PROCESS_ACTOR", "same_worker")
            .env("RUPU_QUEUE_PROCESS_OUTPUT", &output)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        children.push((child, output));
    }
    let mut replies = Vec::new();
    for (mut child, path) in children {
        assert!(child.wait().unwrap().success());
        let raw = fs::read_to_string(path).unwrap();
        replies.push(serde_json::from_str::<WorkReply>(&raw).unwrap());
    }
    assert_eq!(replies[0].receipt, replies[1].receipt);
    assert_eq!(replies.iter().filter(|r| r.replayed).count(), 1);
    assert_eq!(
        op.board
            .work_get(&posted.current_item.work_id)
            .unwrap()
            .attempt_count,
        1
    );
}

#[test]
fn page_metadata_and_rows_share_one_read_snapshot_during_writes() {
    let dir = fixture();
    let op = operator(dir.path());
    post(&op, "First", "Concurrent snapshot test");
    let root = dir.path().to_owned();
    let writer = std::thread::spawn(move || {
        let bus = operator(&root);
        for index in 0..24 {
            post(
                &bus,
                &format!("Additional {index}"),
                "Append while another connection pages",
            );
        }
    });
    while !writer.is_finished() {
        match op.board.work_list(WorkPageQuery {
            filters: WorkFilters::default(),
            limit: 200,
            cursor: None,
        }) {
            Ok(page) => {
                assert_eq!(page.snapshot.revision, page.counts.matching_total);
                assert_eq!(page.items.len() as u64, page.counts.matching_total);
                assert_eq!(
                    page.items.first().unwrap().creation_sequence,
                    page.snapshot.capacity.item_count
                );
                assert!(page
                    .items
                    .windows(2)
                    .all(|w| w[0].creation_sequence > w[1].creation_sequence));
            }
            Err(WorkError::Busy) => (),
            Err(error) => panic!("unexpected read error: {error}"),
        }
    }
    writer.join().unwrap();
    assert_eq!(op.board.work_snapshot().unwrap().capacity.item_count, 25);
}
