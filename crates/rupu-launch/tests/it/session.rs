//! Session orchestration exercises the real scheduler/ledger with disposable
//! filesystem state and scripted process children. Never invokes a provider.
use crate::support::{config, request, Home};
use rupu_launch::{ledger, session::SessionLauncher, testing::ScriptedProcesses};
use rupu_tools::{
    ChildKind, ChildState, JoinSet, Launcher, ParentEnd, PermissionMode, Surface, Until,
};
use std::{sync::Arc, time::Duration};

fn context(home: &Home, turn: &str, mode: PermissionMode) -> rupu_tools::ToolContext {
    let mut ctx = home.parent(Some("crew/lead"), &["probe"], mode);
    let id = ctx.identity_mut();
    id.run_id = turn.into();
    id.surface = Surface::Session;
    id.scope_name = Some("ses_test".into());
    id.launch.children_dir = Some(home.runs().join(turn));
    ctx
}
fn home() -> Home {
    let home = Home::new();
    home.agent("probe", "provider: anthropic\nmodel: claude-sonnet-4-6");
    std::fs::create_dir_all(home.global().join("sessions/ses_test")).unwrap();
    home
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_child_result_survives_turn_and_launcher_restart() {
    let h = home();
    let dir = h.global().join("sessions/ses_test");
    let ctx1 = context(&h, "turn_one", PermissionMode::Readonly);
    let backend = Arc::new(ScriptedProcesses::done("durable result"));
    let l1 = SessionLauncher::open(
        &dir,
        "ses_test",
        &ctx1,
        h.launcher_with(h.assembler(config()), backend.clone(), None),
    )
    .await
    .unwrap();
    let child = l1
        .launch(request(ChildKind::Agent, "probe", Some("task")), &ctx1)
        .await
        .unwrap();
    // Observe completion without collecting it; the next turn joins it.
    tokio::time::timeout(Duration::from_secs(5), async {
        while l1
            .children(&ctx1.identity)
            .iter()
            .all(|c| c.state != ChildState::Done)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Completed).await;
    drop(l1);
    let ctx2 = context(&h, "turn_two", PermissionMode::Readonly);
    let next = Arc::new(ScriptedProcesses::done("must not relaunch"));
    let l2 = SessionLauncher::open(
        &dir,
        "ses_test",
        &ctx2,
        h.launcher_with(h.assembler(config()), next.clone(), None),
    )
    .await
    .unwrap();
    let result = l2
        .join(
            &ctx2.identity,
            JoinSet::Handles(vec![child.handle.clone()]),
            Duration::ZERO,
            Until::All,
        )
        .await;
    assert_eq!(result[0].state, ChildState::Done);
    assert_eq!(result[0].output.as_deref(), Some("durable result"));
    assert!(next.spawned().is_empty());
    assert_eq!(backend.spawned().len(), 1);
    assert_eq!(
        ledger::read(&h.runs().join("turn_one"))[0]
            .continued_by
            .as_deref(),
        Some("turn_two")
    );
    l2.finish(&ctx2.identity, ParentEnd::Completed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_checkpoint_reschedules_pending_work_with_current_permission_ceiling() {
    checkpoint_then_resume(ParentEnd::Completed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_cancel_checkpoints_children_and_resumes_with_current_permission_ceiling() {
    checkpoint_then_resume(ParentEnd::Paused).await;
}

async fn checkpoint_then_resume(end: ParentEnd) {
    let h = home();
    let dir = h.global().join("sessions/ses_test");
    let ctx1 = context(&h, "turn_one", PermissionMode::Bypass);
    let backend = Arc::new(ScriptedProcesses::scripted(vec![]));
    let l1 = SessionLauncher::open(
        &dir,
        "ses_test",
        &ctx1,
        h.launcher_with(h.assembler(config()), backend.clone(), None),
    )
    .await
    .unwrap();
    let child = l1
        .launch(
            request(ChildKind::Agent, "probe", Some("durable task")),
            &ctx1,
        )
        .await
        .unwrap();
    l1.finish(&ctx1.identity, end).await;
    assert_eq!(
        ledger::read(&h.runs().join("turn_one"))[0].state(),
        ChildState::Interrupted
    );
    drop(l1);
    let ctx2 = context(&h, "turn_two", PermissionMode::Readonly);
    let next = Arc::new(ScriptedProcesses::done("rescheduled"));
    let l2 = SessionLauncher::open(
        &dir,
        "ses_test",
        &ctx2,
        h.launcher_with(h.assembler(config()), next.clone(), None),
    )
    .await
    .unwrap();
    let result = l2
        .join(
            &ctx2.identity,
            JoinSet::Handles(vec![child.handle]),
            Duration::from_secs(5),
            Until::All,
        )
        .await;
    assert_eq!(result[0].state, ChildState::Done);
    assert_eq!(result[0].output.as_deref(), Some("rescheduled"));
    let launches = next.spawned();
    assert_eq!(launches.len(), 1);
    match &launches[0].argv {
        rupu_runtime::argv::RunArgv::Agent(a) => assert_eq!(a.mode, Some(PermissionMode::Readonly)),
        _ => panic!("agent child expected"),
    }
    l2.finish(&ctx2.identity, ParentEnd::Completed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_refuses_foreign_ancestry_and_concurrent_owner() {
    let h = home();
    let dir = h.global().join("sessions/ses_test");
    let ctx1 = context(&h, "turn_one", PermissionMode::Readonly);
    let launcher = h.launcher(config(), None);
    assert!(
        SessionLauncher::open(&dir, "ses_other", &ctx1, launcher.clone())
            .await
            .is_err()
    );
    let l1 = SessionLauncher::open(&dir, "ses_test", &ctx1, launcher.clone())
        .await
        .unwrap();
    let ctx2 = context(&h, "turn_two", PermissionMode::Readonly);
    assert!(
        SessionLauncher::open(&dir, "ses_test", &ctx2, launcher.clone())
            .await
            .is_err()
    );
    drop(l1);
    std::fs::write(
        dir.join("orchestration-turns.json"),
        r#"{"session_id":"ses_other","turns":["flow_other"]}"#,
    )
    .unwrap();
    assert!(
        SessionLauncher::open(&dir, "ses_test", &ctx2, launcher.clone())
            .await
            .is_err()
    );
    std::fs::write(
        dir.join("orchestration-turns.json"),
        r#"{"session_id":"ses_test","turns":["flow_other"]}"#,
    )
    .unwrap();
    assert!(
        SessionLauncher::open(&dir, "ses_test", &ctx2, launcher)
            .await
            .is_err(),
        "matching journal identity alone must not adopt an unrelated flow"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_does_not_resume_a_child_removed_from_its_allowlist() {
    let h = home();
    let dir = h.global().join("sessions/ses_test");
    let ctx1 = context(&h, "turn_one", PermissionMode::Readonly);
    let l1 = SessionLauncher::open(
        &dir,
        "ses_test",
        &ctx1,
        h.launcher_with(
            h.assembler(config()),
            Arc::new(ScriptedProcesses::scripted(vec![])),
            None,
        ),
    )
    .await
    .unwrap();
    let child = l1
        .launch(request(ChildKind::Agent, "probe", Some("task")), &ctx1)
        .await
        .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Completed).await;
    drop(l1);
    let mut ctx2 = context(&h, "turn_two", PermissionMode::Readonly);
    ctx2.identity_mut().launch.agents.clear();
    let next = Arc::new(ScriptedProcesses::done("not allowed"));
    let l2 = SessionLauncher::open(
        &dir,
        "ses_test",
        &ctx2,
        h.launcher_with(h.assembler(config()), next.clone(), None),
    )
    .await
    .unwrap();
    let result = l2
        .join(
            &ctx2.identity,
            JoinSet::Handles(vec![child.handle]),
            Duration::ZERO,
            Until::All,
        )
        .await;
    assert_eq!(result[0].state, ChildState::Failed);
    assert!(result[0].error.as_deref().unwrap().contains("allowlist"));
    assert!(next.spawned().is_empty());
    l2.finish(&ctx2.identity, ParentEnd::Completed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_board_messages_directives_and_claims_survive_reopen() {
    use rupu_tools::board::{BoardClaimTool, BoardDirectiveTool, BoardPostTool, BoardReleaseTool};
    use rupu_tools::Tool;
    let h = home();
    let dir = h.global().join("sessions/ses_test");
    let mut ctx1 = context(&h, "turn_one", PermissionMode::Readonly);
    let bus = Arc::new(rupu_fleet::Bus::open(&dir.join("messages"), "lead").staying());
    bus.join(rupu_fleet::Member {
        agent: "session".into(),
        run_id: "turn_one".into(),
        kind: "session_turn".into(),
        ..Default::default()
    })
    .unwrap();
    ctx1.services.message_bus = Some(bus.clone());
    let l1 = SessionLauncher::open(&dir, "ses_test", &ctx1, h.launcher(config(), None))
        .await
        .unwrap();
    assert!(BoardClaimTool
        .invoke(serde_json::json!({"work_unit":"task:one"}), &ctx1)
        .await
        .unwrap()
        .error
        .is_none());
    assert!(BoardPostTool
        .invoke(
            serde_json::json!({"kind":"note","body":"task state: awaiting review"}),
            &ctx1
        )
        .await
        .unwrap()
        .error
        .is_none());
    assert!(BoardDirectiveTool
        .invoke(
            serde_json::json!({"body":"Inspect only the disposable fixture"}),
            &ctx1
        )
        .await
        .unwrap()
        .error
        .is_none());
    bus.send(&rupu_fleet::Address::Broadcast, "child result ready")
        .unwrap();
    bus.send(
        &rupu_fleet::Address::Named("lead".into()),
        "pending direct message",
    )
    .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Completed).await;
    drop(l1);
    drop(ctx1);
    drop(bus);
    let reopened = Arc::new(rupu_fleet::Bus::open(&dir.join("messages"), "lead").staying());
    assert_eq!(
        reopened.board.claim_holder("task:one").unwrap().as_deref(),
        Some("lead")
    );
    assert_eq!(
        reopened.board.read_posts().unwrap()[0].body,
        "task state: awaiting review"
    );
    assert_eq!(
        reopened.board.read_directives().unwrap()[0].body,
        "Inspect only the disposable fixture"
    );
    assert!(!reopened.read_log(None, 100).unwrap().lines.is_empty());
    let mut ctx2 = context(&h, "turn_two", PermissionMode::Readonly);
    ctx2.services.message_bus = Some(reopened.clone());
    let l2 = SessionLauncher::open(&dir, "ses_test", &ctx2, h.launcher(config(), None))
        .await
        .unwrap();
    assert!(reopened.claims.lock().contains_key("task:one"));
    let inbox = reopened.mailbox.drain("lead").unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].body, "pending direct message");
    assert!(reopened.mailbox.drain("lead").unwrap().is_empty());
    BoardReleaseTool
        .invoke(serde_json::json!({"work_unit":"task:one"}), &ctx2)
        .await
        .unwrap();
    assert_eq!(reopened.board.claim_holder("task:one").unwrap(), None);
    l2.finish(&ctx2.identity, ParentEnd::Completed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_workflow_belongs_to_the_session_across_turns() {
    use rupu_tools::{ChildRequest, ChildTarget};
    let h = home();
    let dir = h.global().join("sessions/ses_test");
    let ctx1 = context(&h, "turn_one", PermissionMode::Readonly);
    let l1 = SessionLauncher::open(&dir, "ses_test", &ctx1, h.launcher(config(), None))
        .await
        .unwrap();
    let generated = h.runs().join("turn_one/generated");
    std::fs::create_dir_all(&generated).unwrap();
    let file = generated.join("review.yaml");
    std::fs::write(
        &file,
        "name: review\nsteps:\n  - id: inspect\n    agent: probe\n    prompt: inspect fixture\n",
    )
    .unwrap();
    l1.finish(&ctx1.identity, ParentEnd::Completed).await;
    drop(l1);
    let ctx2 = context(&h, "turn_two", PermissionMode::Readonly);
    let backend = Arc::new(ScriptedProcesses::done("workflow completed"));
    let l2 = SessionLauncher::open(
        &dir,
        "ses_test",
        &ctx2,
        h.launcher_with(h.assembler(config()), backend.clone(), None),
    )
    .await
    .unwrap();
    let req = ChildRequest {
        kind: ChildKind::Workflow,
        target: ChildTarget::File(file.clone()),
        prompt: None,
        inputs: Default::default(),
    };
    l2.check(&req, &ctx2).await.unwrap();
    let child = l2.launch(req.clone(), &ctx2).await.unwrap();
    assert_eq!(
        l2.join(
            &ctx2.identity,
            JoinSet::Handles(vec![child.handle]),
            Duration::from_secs(5),
            Until::All
        )
        .await[0]
            .state,
        ChildState::Done
    );
    let mut unrelated = context(&h, "turn_foreign", PermissionMode::Readonly);
    unrelated.identity_mut().scope_name = Some("ses_other".into());
    assert!(l2.check(&req, &unrelated).await.is_err());
    l2.finish(&ctx2.identity, ParentEnd::Completed).await;
}
