// Included inside session::tests so this uses the existing disposable home
// and mock-provider seam without exposing production helpers.
#[tokio::test]
async fn orchestration_is_assembled_in_real_session_turns_and_survives_disable() {
    let _guard = crate::test_support::ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (global, mut record) = idle_dense_session(&tmp, "ses_orchestration");
    record.message_history.clear();
    record.model_limits = Some(rupu_providers::model_limits::ModelLimits::fixed(
        100_000, 8_192,
    ));
    record.agent_tools = Some(Vec::new());
    record.permission_mode = "readonly".into();
    write_session(&global, SessionScope::Active, &record).unwrap();
    let dir = session_dir(&global, SessionScope::Active, &record.session_id);
    for (index, enabled) in [true, false, true].into_iter().enumerate() {
        rupu_runtime::session_orchestration::SessionOrchestration {
            revision: index as u64,
            enabled,
        }
        .save(&dir)
        .unwrap();
        let run_id = format!("run_orchestration_{index}");
        enqueue_session_turn_request(
            &global,
            SessionScope::Active,
            &record.session_id,
            SessionTurnRequest {
                version: 1,
                request_id: format!("req_{index}"),
                run_id: run_id.clone(),
                prompt: "go".into(),
                transcript_path: record.transcripts_dir.join(format!("{run_id}.jsonl")),
                enqueued_at: Utc::now(),
                compact: false,
                operator_skills: Vec::new(),
            },
        )
        .unwrap();
        let queued: serde_json::Value = serde_json::from_slice(
            &std::fs::read(session_turn_request_path(
                &global,
                SessionScope::Active,
                &record.session_id,
                &format!("req_{index}"),
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            queued["version"], 4,
            "old workers must refuse configured sessions, including disabled revisions"
        );
        with_mock_home(
            &global,
            r#"[{"AssistantText":{"text":"session turn done","stop":"end_turn"}}]"#,
            run_turn(RunTurnArgs {
                session_id: record.session_id.clone(),
                run_id: run_id.clone(),
                prompt: "go".into(),
                operator_skills: Vec::new(),
            }),
        )
        .await
        .unwrap();
        let events: Vec<_> =
            JsonlReader::iter(record.transcripts_dir.join(format!("{run_id}.jsonl")))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
        let grant = events
            .iter()
            .find_map(|e| match e {
                TranscriptEvent::ToolGrant { entries, .. } => Some(entries),
                _ => None,
            })
            .unwrap();
        assert_eq!(grant.iter().any(|g| g.tool == "dispatch"), enabled);
        assert_eq!(grant.iter().any(|g| g.tool == "board.directive"), enabled);
        assert!(!grant.iter().any(|g| g.tool == "goal.status"));
        assert!(!grant.iter().any(|g| g.tool == "budget.status"));
        let after = read_session(&global, &record.session_id).unwrap().0;
        assert_eq!(after.status, SessionStatus::Idle, "{:?}", after.last_error);
        assert!(after.total_turns > index as u32);
        assert_eq!(after.permission_mode, "readonly");
    }
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("orchestration-turns.json")).unwrap())
            .unwrap();
    assert_eq!(journal["session_id"], record.session_id);
    assert_eq!(
        journal["turns"],
        serde_json::json!(["run_orchestration_0", "run_orchestration_2"])
    );
}
