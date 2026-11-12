//! Disposable canonical-owner fixtures only; never touch live stores or launch providers.
#![allow(clippy::disallowed_methods)]
use rupu_fleet::{Board, Bus, Member};
use serde_json::{json, Value};
use std::{net::SocketAddr, path::Path, sync::Arc};

struct NoopLauncher;
#[async_trait::async_trait]
impl rupu_cp::launcher::RunLauncher for NoopLauncher {
    async fn launch(
        &self,
        _: rupu_cp::launcher::LaunchRequest,
    ) -> Result<String, rupu_cp::launcher::LaunchError> {
        panic!("Board controls must never launch a run")
    }
}
async fn serve(global: &Path, writable: bool, token: Option<&str>) -> SocketAddr {
    let mut state = rupu_cp::state::AppState::new(global.into(), Default::default());
    if writable {
        state = state.with_launcher(Some(Arc::new(NoopLauncher)));
    }
    let app = rupu_cp::server::router(state, token.map(str::to_owned));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}
fn seed(global: &Path, kind: &str, id: &str, status: &str) -> std::path::PathBuf {
    let collection = match kind {
        "workflow" => "runs",
        "session" => "sessions",
        "agentiflow" => "agentiflows",
        _ => panic!(),
    };
    let dir = global.join(collection).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let (file, raw) = match kind {
        "workflow" => (
            "run.json",
            json!({"id":id,"workflow_name":"audit","status":status,"started_at":"2026-11-08T00:00:00Z","runner_pid":std::process::id()}),
        ),
        "session" => (
            "session.json",
            json!({"session_id":id,"status":status,"created_at":"2026-11-08T00:00:00Z"}),
        ),
        _ => (
            "agentiflow.json",
            json!({"id":id,"status":status,"started_at":"2026-11-08T00:00:00Z","runner_pid":std::process::id()}),
        ),
    };
    std::fs::write(dir.join(file), raw.to_string()).unwrap();
    if kind == "agentiflow" {
        dir
    } else {
        dir.join("messages")
    }
}
async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let response = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    (
        response.status().as_u16(),
        response.json().await.unwrap_or(Value::Null),
    )
}
async fn post(addr: SocketAddr, path: &str, token: &str, operation: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(&json!({"owner_token":token,"operation":operation}))
        .send()
        .await
        .unwrap();
    (
        response.status().as_u16(),
        response.json().await.unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn all_local_owner_families_post_as_operator_without_roster_impersonation() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let addr = serve(&global, true, None).await;
    for (kind, route, id) in [
        ("workflow", "runs", "run_board"),
        ("session", "sessions", "ses_board"),
        ("agentiflow", "agentiflows", "af_board"),
    ] {
        let space = seed(
            &global,
            kind,
            id,
            if kind == "session" { "idle" } else { "running" },
        );
        let path = format!("/api/{route}/{id}/board/operator");
        let (status, before) = get(addr, &path).await;
        assert_eq!(status, 200, "{before}");
        assert_eq!(before["owner"]["id"], id);
        assert_eq!(before["live"], true);
        let token = before["owner"]["token"].as_str().unwrap();
        let (status, receipt) = post(
            addr,
            &path,
            token,
            json!({"action":"post","kind":"note","body":"review scope"}),
        )
        .await;
        assert_eq!(status, 200, "{receipt}");
        assert_eq!(receipt["owner"], before["owner"]);
        assert_eq!(receipt["actor"], "operator");
        assert_eq!(receipt["receipt"]["from"], "operator");
        assert_eq!(receipt["receipt"]["via"], "cp");
        let bus = Bus::open(&space, "operator");
        assert!(bus.roster().unwrap().participants().next().is_none());
        assert_eq!(bus.board.read_posts().unwrap()[0].author, "operator");
    }
}
#[tokio::test]
async fn claim_receipts_recover_and_only_release_the_exact_operator_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = seed(&global, "session", "ses_board", "idle");
    let addr = serve(&global, true, None).await;
    let path = "/api/sessions/ses_board/board/operator";
    let (_, view) = get(addr, path).await;
    let token = view["owner"]["token"].as_str().unwrap();
    let (status, claim) = post(
        addr,
        path,
        token,
        json!({"action":"claim","work_unit":"audit","ttl_seconds":30}),
    )
    .await;
    assert_eq!(status, 200, "{claim}");
    assert_eq!(claim["receipt"]["status"], "granted");
    let lease = &claim["receipt"]["lease"];
    let (_, read) = get(addr, path).await;
    assert_eq!(read["operator_leases"][0], *lease);
    let (status, _) = post(
        addr,
        path,
        token,
        json!({"action":"release","work_unit":"audit","claim_token":"stale"}),
    )
    .await;
    assert_eq!(status, 409);
    let (status, receipt) = post(
        addr,
        path,
        token,
        json!({"action":"release","work_unit":"audit","claim_token":lease["token"]}),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    assert!(Board::new(space).claim_holder("audit").unwrap().is_none());
    assert_eq!(
        post(
            addr,
            path,
            token,
            json!({"action":"release","work_unit":"audit","claim_token":lease["token"]})
        )
        .await
        .0,
        409
    );
}
#[tokio::test]
async fn directive_retraction_requires_the_exact_live_directive() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = seed(&global, "workflow", "run_board", "running");
    let addr = serve(&global, true, None).await;
    let path = "/api/runs/run_board/board/operator";
    let (_, view) = get(addr, path).await;
    let token = view["owner"]["token"].as_str().unwrap();
    let (status, receipt) = post(
        addr,
        path,
        token,
        json!({"action":"directive","body":"stay inside approved targets"}),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    let (_, snapshot) = get(addr, path).await;
    let expected: rupu_fleet::Directive =
        serde_json::from_value(snapshot["live_directives"][0].clone()).unwrap();
    assert_eq!(expected.author, "operator");
    let mut stale = expected.clone();
    stale.body = "stale".into();
    assert_eq!(
        post(
            addr,
            path,
            token,
            json!({"action":"retract","expected":stale})
        )
        .await
        .0,
        409
    );
    let (status, receipt) = post(
        addr,
        path,
        token,
        json!({"action":"retract","expected":expected}),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["receipt"]["from"], "operator");
    assert_eq!(receipt["receipt"]["refs"]["retracts"], expected.id);
    assert!(Board::new(space).read_directives().unwrap().is_empty());
}
#[tokio::test]
async fn remote_wrong_route_readonly_terminal_and_replaced_owners_never_write() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = seed(&global, "workflow", "run_board", "running");
    let addr = serve(&global, true, None).await;
    let path = "/api/runs/run_board/board/operator";
    let (_, view) = get(addr, path).await;
    let token = view["owner"]["token"].as_str().unwrap();
    let op = json!({"action":"post","kind":"note","body":"hi"});
    assert_eq!(
        post(addr, &format!("{path}?host=remote"), token, op.clone())
            .await
            .0,
        501
    );
    assert_eq!(
        post(
            addr,
            "/api/sessions/run_board/board/operator",
            token,
            op.clone()
        )
        .await
        .0,
        404
    );
    let readonly = serve(&global, false, None).await;
    assert_eq!(post(readonly, path, token, op.clone()).await.0, 501);
    seed(&global, "workflow", "run_board", "completed");
    assert_eq!(post(addr, path, token, op.clone()).await.0, 409);
    seed(&global, "workflow", "run_board", "running");
    std::fs::rename(
        global.join("runs/run_board"),
        global.join("runs/old-record"),
    )
    .unwrap();
    seed(&global, "workflow", "run_board", "running");
    assert_eq!(post(addr, path, token, op).await.0, 409);
    assert!(!space.join("log.jsonl").exists());
}
#[tokio::test]
async fn operator_routes_keep_existing_auth_and_reject_actor_fields_and_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = seed(&global, "session", "ses_board", "idle");
    let addr = serve(&global, true, None).await;
    let path = "/api/sessions/ses_board/board/operator";
    let (_, view) = get(addr, path).await;
    let token = view["owner"]["token"].as_str().unwrap();
    assert_eq!(
        post(
            addr,
            path,
            token,
            json!({"action":"post","kind":"note","body":"hi","author":"lead"})
        )
        .await
        .0,
        422
    );
    let protected = serve(&global, true, Some("fixture-only-token")).await;
    assert_eq!(get(protected, path).await.0, 401);
    assert!(!space.join("log.jsonl").exists());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), &space).unwrap();
        assert_eq!(
            post(
                addr,
                path,
                token,
                json!({"action":"post","kind":"note","body":"hi"})
            )
            .await
            .0,
            409
        );
        assert!(!outside.path().join("log.jsonl").exists());
    }
}

#[tokio::test]
async fn standalone_requires_its_own_live_recorded_participant_and_tokens_never_cross_owners() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = global.join("runs/run_bare/messages");
    std::fs::create_dir_all(&space).unwrap();
    let bus = Bus::open(&space, "worker");
    let member = Member {
        run_id: "run_other".into(),
        agent: "review".into(),
        kind: "standalone".into(),
        pid: Some(std::process::id()),
        ..Default::default()
    };
    bus.join(member.clone()).unwrap();
    let addr = serve(&global, true, None).await;
    let path = "/api/runs/run_bare/board/operator";
    let (_, before) = get(addr, path).await;
    assert_eq!(before["live"], false);
    let token = before["owner"]["token"].as_str().unwrap();
    let op = json!({"action":"post","kind":"note","body":"scope"});
    assert_eq!(post(addr, path, token, op.clone()).await.0, 409);
    bus.join(Member {
        run_id: "run_bare".into(),
        ..member
    })
    .unwrap();
    let (status, current) = get(addr, path).await;
    assert_eq!(status, 200, "{current}");
    assert_eq!(current["owner"]["kind"], "standalone");
    assert_eq!(current["live"], true);
    assert_eq!(post(addr, path, token, op.clone()).await.0, 200);
    assert!(!global.join("runs/run_bare/run.json").exists());
    let other_space = seed(&global, "session", "ses_other", "idle");
    assert_eq!(
        post(addr, "/api/sessions/ses_other/board/operator", token, op)
            .await
            .0,
        409
    );
    assert!(!other_space.join("log.jsonl").exists());
}

#[tokio::test]
async fn corrupt_record_identity_and_nested_store_symlinks_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = seed(&global, "workflow", "run_board", "running");
    let addr = serve(&global, true, None).await;
    let path = "/api/runs/run_board/board/operator";
    let (_, view) = get(addr, path).await;
    let token = view["owner"]["token"].as_str().unwrap();
    let op = json!({"action":"post","kind":"note","body":"scope"});
    let record = global.join("runs/run_board/run.json");
    let raw = std::fs::read(&record).unwrap();
    let mut bad: Value = serde_json::from_slice(&raw).unwrap();
    bad["id"] = json!("run_other");
    std::fs::write(&record, bad.to_string()).unwrap();
    assert_eq!(post(addr, path, token, op.clone()).await.0, 409);
    std::fs::write(&record, raw).unwrap();
    #[cfg(unix)]
    {
        std::fs::create_dir_all(&space).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), space.join("board")).unwrap();
        assert_eq!(post(addr, path, token, op).await.0, 409);
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }
    assert!(!space.join("log.jsonl").exists());
}

#[tokio::test]
async fn partially_applied_post_returns_recorded_id_for_read_reconciliation() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = seed(&global, "session", "ses_board", "idle");
    let addr = serve(&global, true, None).await;
    let path = "/api/sessions/ses_board/board/operator";
    let (_, view) = get(addr, path).await;
    let token = view["owner"]["token"].as_str().unwrap();
    // A local fixture apply failure after the canonical message is appended.
    std::fs::create_dir_all(space.join("board/posts.jsonl")).unwrap();
    let (status, receipt) = post(
        addr,
        path,
        token,
        json!({"action":"post","kind":"note","body":"exactly one attempt"}),
    )
    .await;
    assert_eq!(status, 500, "{receipt}");
    assert_eq!(receipt["code"], "operator_board_write_uncertain");
    assert_eq!(receipt["mutation_outcome"], "reconcile");
    let page = Bus::open(&space, "operator").read_log(None, 10).unwrap();
    assert_eq!(page.lines.len(), 1);
    let rupu_fleet::LogLine::Msg(recorded) = &page.lines[0] else {
        panic!()
    };
    assert_eq!(receipt["logged_message_id"], recorded.id);
    assert_eq!(recorded.from, "operator");
}

#[tokio::test]
async fn earlier_owner_token_cannot_bypass_grown_directive_journal_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = seed(&global, "session", "ses_board", "idle");
    let addr = serve(&global, true, None).await;
    let path = "/api/sessions/ses_board/board/operator";
    let (_, view) = get(addr, path).await;
    let token = view["owner"]["token"].as_str().unwrap();
    assert_eq!(
        post(
            addr,
            path,
            token,
            json!({"action":"directive","body":"captured before growth"})
        )
        .await
        .0,
        200
    );
    let (_, snapshot) = get(addr, path).await;
    let expected = snapshot["live_directives"][0].clone();
    let log_path = space.join("log.jsonl");
    let before = std::fs::read(&log_path).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(space.join("board/directives.jsonl"))
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    let (status, refusal) = post(
        addr,
        path,
        token,
        json!({"action":"retract","expected":expected}),
    )
    .await;
    assert_eq!(status, 409, "{refusal}");
    assert_eq!(refusal["mutation_outcome"], "not_applied");
    assert!(refusal["error"].as_str().unwrap().contains("32 MiB"));
    assert_eq!(std::fs::read(&log_path).unwrap(), before);
    assert_eq!(get(addr, path).await.0, 409);
    assert_eq!(std::fs::read(&log_path).unwrap(), before);
}

#[tokio::test]
async fn exact_registered_lead_board_receipts_match_native_recipient_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    let space = seed(&global, "agentiflow", "af_board_lead", "running");
    let lead = Bus::open(&space, "lead");
    lead.join(Member {
        agent: "lead-agent".into(),
        run_id: "af_board_lead".into(),
        kind: "agentiflow-lead".into(),
        ..Default::default()
    })
    .unwrap();
    let addr = serve(&global, true, None).await;
    let path = "/api/agentiflows/af_board_lead/board/operator";
    let (status, view) = get(addr, path).await;
    assert_eq!(status, 200, "{view}");
    let token = view["owner"]["token"].as_str().unwrap();
    for action in ["post", "directive"] {
        let mut operation = json!({"action":action,"body":"recorded scope","addressed_to":"lead","expected_run_id":"af_board_lead"});
        if action == "post" {
            operation["kind"] = json!("note");
        }
        let (status, receipt) = post(addr, path, token, operation.clone()).await;
        assert_eq!(status, 200, "{receipt}");
        assert_eq!(receipt["owner"], view["owner"]);
        assert_eq!(receipt["actor"], "operator");
        assert_eq!(receipt["via"], "cp");
        assert_eq!(receipt["action"], action);
        assert_eq!(receipt["receipt"]["from"], "operator");
        assert_eq!(receipt["receipt"]["to"], "lead");
        assert_eq!(receipt["receipt"]["kind"], action);
        assert!(receipt["receipt"]["id"]
            .as_str()
            .unwrap()
            .starts_with("msg_"));
        assert!(receipt["receipt"]["from_codename"].is_null());
        let before = std::fs::read(space.join("log.jsonl")).unwrap();
        operation["expected_run_id"] = json!("af_stale");
        let (status, refused) = post(addr, path, token, operation).await;
        assert_eq!(status, 409, "{refused}");
        assert_eq!(refused["code"], "operator_board_refused");
        assert_eq!(refused["mutation_outcome"], "not_applied");
        assert_eq!(std::fs::read(space.join("log.jsonl")).unwrap(), before);
    }
}
