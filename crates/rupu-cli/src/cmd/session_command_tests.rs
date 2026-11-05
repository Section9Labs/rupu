//! Cooperative cancellation exercises the real worker turn path with mock providers.
use super::tests::{idle_dense_session, with_mock_home};
use super::*;

#[tokio::test]
async fn cooperative_cancel_keeps_worker_and_next_turn_usable() {
    cancel_then_continue(false).await;
}

#[tokio::test]
async fn cooperative_cancel_orchestration_keeps_worker_and_next_turn_usable() {
    cancel_then_continue(true).await;
}

async fn cancel_then_continue(orchestration: bool) {
    let _env = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let (global, mut record) = idle_dense_session(&tmp, "ses_cancel");
    record.message_history.clear();
    record.worker_pid = Some(std::process::id());
    record.no_stream = true;
    write_session(&global, SessionScope::Active, &record).unwrap();
    let make_request = |run: &str| SessionTurnRequest {
        version: SessionTurnRequest::VERSION,
        request_id: format!("req_{run}"),
        run_id: run.into(),
        prompt: "continue".into(),
        transcript_path: record.transcripts_dir.join(format!("{run}.jsonl")),
        enqueued_at: Utc::now(),
        compact: false,
        operator_skills: Vec::new(),
    };
    let first = make_request("run_cancel");
    let dir = session_dir(&global, SessionScope::Active, &record.session_id);
    if orchestration {
        rupu_runtime::session_orchestration::SessionOrchestration {
            revision: 0,
            enabled: true,
        }
        .save(&dir)
        .unwrap();
    }
    let cancel = async {
        loop {
            if rupu_runtime::session_cancel::available(
                &dir,
                "run_cancel",
                Some(std::process::id()),
                rupu_launch::proc::process_alive,
            ) && (!orchestration || dir.join("orchestration-turns.json").exists())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        rupu_runtime::session_cancel::request(&dir, "run_cancel", rupu_launch::proc::process_alive)
            .unwrap();
    };
    let slow =
        r#"[{"Slow":{"ms":30000,"turn":{"AssistantText":{"text":"late","stop":"end_turn"}}}}]"#;
    let processing = dir.join("first.processing");
    let first_work = work_claimed_turn(
        &global,
        SessionScope::Active,
        record.clone(),
        std::process::id(),
        ClaimedTurn::Request(first),
        &processing,
    );
    with_mock_home(&global, slow, async {
        let (result, ()) = tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(first_work, cancel)
        })
        .await
        .unwrap();
        result.unwrap();
    })
    .await;
    let after = read_session(&global, &record.session_id).unwrap().0;
    assert_eq!(after.status, SessionStatus::Idle);
    assert_eq!(after.worker_pid, Some(std::process::id()));
    assert!(after.active_run_id.is_none());
    assert!(
        JsonlReader::iter(record.transcripts_dir.join("run_cancel.jsonl"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| matches!(
                e,
                TranscriptEvent::RunComplete {
                    status: RunStatus::Aborted,
                    ..
                }
            ))
    );
    assert!(rupu_runtime::session_cancel::request(
        &dir,
        "run_cancel",
        rupu_launch::proc::process_alive
    )
    .is_err());
    let next = make_request("run_after");
    with_mock_home(
        &global,
        r#"[{"AssistantText":{"text":"next turn works","stop":"end_turn"}}]"#,
        work_claimed_turn(
            &global,
            SessionScope::Active,
            after,
            std::process::id(),
            ClaimedTurn::Request(next),
            &dir.join("next.processing"),
        ),
    )
    .await
    .unwrap();
    let final_record = read_session(&global, &record.session_id).unwrap().0;
    assert_eq!(final_record.status, SessionStatus::Idle);
    assert_eq!(final_record.worker_pid, Some(std::process::id()));
    assert_eq!(final_record.last_run_id.as_deref(), Some("run_after"));
    assert!(serde_json::to_string(&final_record.message_history)
        .unwrap()
        .contains("next turn works"));
    if orchestration {
        let journal: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("orchestration-turns.json")).unwrap())
                .unwrap();
        assert_eq!(
            journal["turns"],
            serde_json::json!(["run_cancel", "run_after"])
        );
        assert!(!dir.join("cancel-run_cancel.owner.json").exists());
        assert!(!dir.join("cancel-run_after.owner.json").exists());
    }
}

#[tokio::test]
async fn cooperative_cancel_compaction_preserves_history_and_worker() {
    let _env = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let (global, mut record) = idle_dense_session(&tmp, "ses_cancel_compact");
    record.context_window_tokens = Some(1000);
    record.worker_pid = Some(std::process::id());
    let history = serde_json::to_value(&record.message_history).unwrap();
    write_session(&global, SessionScope::Active, &record).unwrap();
    let dir = session_dir(&global, SessionScope::Active, &record.session_id);
    let request = SessionTurnRequest {
        version: SessionTurnRequest::VERSION,
        request_id: "req_cancel_compact".into(),
        run_id: "run_compact".into(),
        prompt: "[compact]".into(),
        transcript_path: record.transcripts_dir.join("run_compact.jsonl"),
        enqueued_at: Utc::now(),
        compact: true,
        operator_skills: Vec::new(),
    };
    let cancel = async {
        loop {
            if rupu_runtime::session_cancel::available(
                &dir,
                "run_compact",
                Some(std::process::id()),
                rupu_launch::proc::process_alive,
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        rupu_runtime::session_cancel::request(
            &dir,
            "run_compact",
            rupu_launch::proc::process_alive,
        )
        .unwrap();
    };
    let processing = dir.join("compact.processing");
    let work = work_claimed_turn(
        &global,
        SessionScope::Active,
        record.clone(),
        std::process::id(),
        ClaimedTurn::Request(request),
        &processing,
    );
    with_mock_home(
        &global,
        r#"[{"Slow":{"ms":30000,"turn":{"AssistantText":{"text":"summary","stop":"end_turn"}}}}]"#,
        async {
            let (result, ()) = tokio::time::timeout(Duration::from_secs(15), async {
                tokio::join!(work, cancel)
            })
            .await
            .unwrap();
            result.unwrap();
        },
    )
    .await;
    let after = read_session(&global, &record.session_id).unwrap().0;
    assert_eq!(after.status, SessionStatus::Idle);
    assert_eq!(after.worker_pid, Some(std::process::id()));
    assert!(after.active_run_id.is_none());
    assert_eq!(
        serde_json::to_value(&after.message_history).unwrap(),
        history
    );
    assert!(
        JsonlReader::iter(record.transcripts_dir.join("run_compact.jsonl"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| matches!(
                e,
                TranscriptEvent::RunComplete {
                    status: RunStatus::Aborted,
                    ..
                }
            ))
    );
}
