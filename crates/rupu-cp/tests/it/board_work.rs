//! In-process production-router fixtures. No listener, worker or provider.
use axum::{
    body::{to_bytes, Body},
    http::Request,
    Router,
};
use rupu_cp::{
    launcher::{LaunchError, LaunchRequest, RunLauncher},
    state::AppState,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tower::ServiceExt;

struct NoLaunch;
#[async_trait::async_trait]
impl RunLauncher for NoLaunch {
    async fn launch(&self, _: LaunchRequest) -> std::result::Result<String, LaunchError> {
        panic!("work bookkeeping cannot launch an execution")
    }
}
fn fixture_app(global: &Path, writable: bool, token: Option<&str>) -> Router {
    let state = AppState::new(global.into(), Default::default())
        .with_launcher(writable.then(|| Arc::new(NoLaunch) as Arc<dyn RunLauncher>));
    rupu_cp::server::router(state, token.map(str::to_owned))
}
fn seed(global: &Path, family: &str, id: &str, status: &str) -> PathBuf {
    let dir = global.join(family).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let (file, record) = match family {
        "sessions" | "sessions-archive" => (
            "session.json",
            json!({"session_id":id,"status":status,"created_at":"2026-11-12T00:00:00Z","runs":[]}),
        ),
        "runs" | "runs-archive" => (
            "run.json",
            json!({"id":id,"status":status,"workflow_name":"audit","started_at":"2026-11-12T00:00:00Z","runner_pid":std::process::id()}),
        ),
        "agentiflows" => (
            "agentiflow.json",
            json!({"id":id,"status":status,"started_at":"2026-11-12T00:00:00Z","runner_pid":std::process::id()}),
        ),
        _ => panic!("unknown fixture family"),
    };
    std::fs::write(dir.join(file), record.to_string()).unwrap();
    dir
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    auth: bool,
) -> (u16, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if auth {
        req = req.header("authorization", "Bearer fixture-token");
    }
    let bytes = body
        .map(|v| serde_json::to_vec(&v).unwrap())
        .unwrap_or_default();
    if !bytes.is_empty() {
        req = req.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn get(app: &Router, path: &str) -> Value {
    let (status, value) = request(app, "GET", path, None, false).await;
    assert_eq!(status, 200, "{value}");
    value
}
fn post_body(page: &Value, id: &str, title: &str) -> Value {
    json!({"owner_token":page["owner"]["token"],"request_id":id,"expected_queue_id":page["snapshot"]["queue_id"],
        "expected_revision":page["snapshot"]["revision"],"operation":{"action":"post","title":title,"instructions":"Inspect the exact request"}})
}
fn transition(page: &Value, reply: &Value, id: &str, action: &str) -> Value {
    json!({"owner_token":page["owner"]["token"],"request_id":id,"expected_queue_id":reply["receipt"]["queue_id"],
        "expected_revision":reply["current_item"]["revision"],"operation":{"action":action,"reason":"Operator review"}})
}

#[tokio::test]
async fn missing_queues_are_read_without_creating_stores_for_all_families() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let app = fixture_app(&root, true, None);
    for (family, id, status) in [
        ("sessions", "ses_work", "idle"),
        ("runs", "run_work", "running"),
        ("agentiflows", "af_work", "running"),
    ] {
        let dir = seed(&root, family, id, status);
        let path = format!("/api/{family}/{id}/board/work");
        let page = get(&app, &path).await;
        assert_eq!(page["snapshot"]["availability"], "not_initialized");
        assert_eq!(page["snapshot"]["queue_id"], Value::Null);
        assert_eq!(page["snapshot"]["revision"], 0);
        assert_eq!(page["owner"]["writable"], true);
        assert_eq!(
            page["counts"]["scope"],
            "matching_filters_at_source_revision"
        );
        assert!(!dir.join("messages").exists());
        assert!(!dir.join("board").exists());
        let (s, reply) = request(
            &app,
            "POST",
            &path,
            Some(post_body(&page, "create", "Review")),
            false,
        )
        .await;
        assert_eq!(s, 200, "{reply}");
        assert_eq!(reply["receipt"]["execution_effect"], "none");
        assert_eq!(
            reply["current_item"]["creator"],
            json!({"kind":"operator","via":"cp"})
        );
        assert_eq!(reply["current_item"]["state"], "queued");
        assert_eq!(
            reply["current_item"]["sources"]["creator"]["availability"],
            "unsupported"
        );
        assert!(!dir.join("messages/participants.jsonl").exists());
    }
}

#[tokio::test]
async fn replay_preserves_receipt_and_returns_current_item_with_history() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    seed(&root, "sessions", "ses_work", "idle");
    let app = fixture_app(&root, true, None);
    let path = "/api/sessions/ses_work/board/work";
    let page = get(&app, path).await;
    let input = post_body(&page, "create", "Review");
    let (s, created) = request(&app, "POST", path, Some(input.clone()), false).await;
    assert_eq!(s, 200, "{created}");
    let id = created["receipt"]["work_id"].as_str().unwrap();
    let changes = format!("{path}/{id}/transitions");
    let (s, cancelled) = request(
        &app,
        "POST",
        &changes,
        Some(transition(&page, &created, "cancel", "cancel")),
        false,
    )
    .await;
    assert_eq!(s, 200, "{cancelled}");
    assert_eq!(cancelled["current_item"]["state"], "cancelled");
    let (s, replayed) = request(&app, "POST", path, Some(input.clone()), false).await;
    assert_eq!(s, 200, "{replayed}");
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["receipt"], created["receipt"]);
    assert_eq!(replayed["current_item"]["state"], "cancelled");
    assert_eq!(replayed["queue_revision"], cancelled["queue_revision"]);
    let mut changed = input;
    changed["operation"]["title"] = json!("different request");
    let (s, error) = request(&app, "POST", path, Some(changed), false).await;
    assert_eq!(s, 409, "{error}");
    assert_eq!(error["error"]["code"], "work_request_conflict");
    let (s, requeued) = request(
        &app,
        "POST",
        &changes,
        Some(transition(&page, &cancelled, "requeue", "requeue")),
        false,
    )
    .await;
    assert_eq!(s, 200, "{requeued}");
    assert_eq!(requeued["current_item"]["state"], "queued");
    let history = get(&app, &format!("{path}/{id}/timeline?limit=2")).await;
    assert_eq!(history["counts"]["matching_total"], 3);
    assert_eq!(history["counts"]["returned"], 2);
    assert_eq!(history["events"][0]["action"], "post");
    assert_eq!(history["events"][1]["action"], "cancel");
    let cursor = history["next_cursor"].as_str().unwrap();
    let last = get(
        &app,
        &format!("{path}/{id}/timeline?limit=2&cursor={cursor}"),
    )
    .await;
    assert_eq!(last["events"][0]["action"], "requeue");
    assert_eq!(last["has_more"], false);
    let detail = get(&app, &format!("{path}/{id}")).await;
    assert_eq!(detail["item"]["event_count"], 3);
    assert_eq!(detail["allowed_actions"], json!(["cancel"]));
    assert!(detail["snapshot"]["capacity"]["database_bytes"].is_number());
}

#[tokio::test]
async fn queue_cursors_bind_owner_filter_endpoint_and_revision() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    seed(&root, "sessions", "ses_work", "idle");
    seed(&root, "sessions", "ses_other", "idle");
    let app = fixture_app(&root, true, None);
    let path = "/api/sessions/ses_work/board/work";
    for i in 0..3 {
        let page = get(&app, path).await;
        let (s, v) = request(
            &app,
            "POST",
            path,
            Some(post_body(&page, &format!("p{i}"), &format!("Item {i}"))),
            false,
        )
        .await;
        assert_eq!(s, 200, "{v}");
    }
    let first = get(&app, &format!("{path}?state=queued&limit=1")).await;
    assert_eq!(first["counts"]["matching_total"], 3);
    assert_eq!(first["counts"]["returned"], 1);
    let cursor = first["next_cursor"].as_str().unwrap();
    let next = get(
        &app,
        &format!("{path}?state=queued&limit=1&cursor={cursor}"),
    )
    .await;
    assert_ne!(next["items"][0]["work_id"], first["items"][0]["work_id"]);
    let item = first["items"][0]["work_id"].as_str().unwrap();
    for bad in [
        format!("{path}?cursor={cursor}"),
        format!("/api/sessions/ses_other/board/work?state=queued&cursor={cursor}"),
        format!("{path}/{item}/timeline?cursor={cursor}"),
    ] {
        let (s, e) = request(&app, "GET", &bad, None, false).await;
        assert_eq!(s, 409, "{e}");
        assert_eq!(e["error"]["retry"], "refresh");
    }
    let page = get(&app, path).await;
    let (s, v) = request(
        &app,
        "POST",
        path,
        Some(post_body(&page, "later", "Later")),
        false,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, e) = request(
        &app,
        "GET",
        &format!("{path}?state=queued&cursor={cursor}"),
        None,
        false,
    )
    .await;
    assert_eq!(s, 409, "{e}");
}

#[tokio::test]
async fn archives_remain_readable_and_old_owner_tokens_cannot_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let active = seed(&root, "sessions", "ses_work", "idle");
    let app = fixture_app(&root, true, None);
    let path = "/api/sessions/ses_work/board/work";
    let page = get(&app, path).await;
    let input = post_body(&page, "create", "Before archive");
    let (s, created) = request(&app, "POST", path, Some(input.clone()), false).await;
    assert_eq!(s, 200, "{created}");
    std::fs::create_dir(root.join("sessions-archive")).unwrap();
    std::fs::rename(&active, root.join("sessions-archive/ses_work")).unwrap();
    let archived = get(&app, path).await;
    assert_eq!(archived["owner"]["archived"], true);
    assert_eq!(archived["owner"]["writable"], false);
    assert_eq!(archived["owner"]["allowed_actions"], json!([]));
    assert_eq!(archived["counts"]["matching_total"], 1);
    assert_ne!(archived["owner"]["token"], page["owner"]["token"]);
    let (s, e) = request(&app, "POST", path, Some(input), false).await;
    assert_eq!(s, 409, "{e}");
    let (s, e) = request(
        &app,
        "POST",
        path,
        Some(post_body(&archived, "new", "Archived")),
        false,
    )
    .await;
    assert_eq!(s, 409, "{e}");
}

#[tokio::test]
async fn strict_inputs_local_host_service_and_auth_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    seed(&root, "sessions", "ses_work", "idle");
    let app = fixture_app(&root, true, None);
    let path = "/api/sessions/ses_work/board/work";
    let page = get(&app, path).await;
    for suffix in ["?host=remote", "?host="] {
        let (s, e) = request(&app, "GET", &format!("{path}{suffix}"), None, false).await;
        assert_eq!(s, 501, "{e}");
    }
    for suffix in [
        "?limit=0",
        "?limit=201",
        "?state=posted",
        "?actor=operator",
        "?limit=nope",
    ] {
        let (s, e) = request(&app, "GET", &format!("{path}{suffix}"), None, false).await;
        assert_eq!(s, 400, "{e}");
        assert_eq!(e["error"]["code"], "work_invalid_input");
    }
    let (s, _) = request(&app, "GET", "/api/runs/ses_work/board/work", None, false).await;
    assert_eq!(s, 404);
    let mut forged = post_body(&page, "forged", "X");
    forged["actor"] = json!({"kind":"agent","run_id":"run_fake"});
    let (s, e) = request(&app, "POST", path, Some(forged), false).await;
    assert_eq!(s, 400, "{e}");
    let mut missing = post_body(&page, "missing", "X");
    missing.as_object_mut().unwrap().remove("expected_queue_id");
    let (s, e) = request(&app, "POST", path, Some(missing), false).await;
    assert_eq!(s, 400, "{e}");
    let readonly = fixture_app(&root, false, None);
    let (s, e) = request(
        &readonly,
        "POST",
        path,
        Some(post_body(&page, "readonly", "X")),
        false,
    )
    .await;
    assert_eq!(s, 501, "{e}");
    let auth = fixture_app(&root, true, Some("fixture-token"));
    let (s, _) = request(&auth, "GET", path, None, false).await;
    assert_eq!(s, 401);
    let (s, e) = request(&auth, "GET", path, None, true).await;
    assert_eq!(s, 200, "{e}");
}

#[cfg(unix)]
#[tokio::test]
async fn lifecycle_lease_refuses_writes_before_any_store_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dir = seed(&root, "sessions", "ses_work", "idle");
    let app = fixture_app(&root, true, None);
    let path = "/api/sessions/ses_work/board/work";
    let page = get(&app, path).await;
    let lease = rupu_runtime::retention_lease::RetentionLease::try_acquire(&root).unwrap();
    let input = post_body(&page, "after-lease", "Review");
    let (s, e) = request(&app, "POST", path, Some(input.clone()), false).await;
    assert_eq!(s, 503, "{e}");
    assert_eq!(e["error"]["retry"], "same_request");
    assert!(!dir.join("messages").exists());
    drop(lease);
    let (s, e) = request(&app, "POST", path, Some(input), false).await;
    assert_eq!(s, 200, "{e}");
}

#[test]
fn mcp_schema_exposes_operator_actions_only() {
    use rupu_mcp::ToolSource;
    let tools = rupu_cp::mcp::operator_source::OperatorSource::new(Router::new()).tools();
    let work: Vec<_> = tools
        .iter()
        .filter(|t| t.name.starts_with("board.work."))
        .collect();
    assert_eq!(work.len(), 5);
    let transitions = work
        .iter()
        .find(|t| t.name == "board.work.transitions")
        .unwrap();
    let schema = transitions.input_schema.to_string();
    assert!(schema.contains("cancel") && schema.contains("requeue"));
    for forbidden in [
        "claim",
        "executor",
        "complete",
        "execution_id",
        "tool_call_id",
    ] {
        assert!(!schema.contains(forbidden), "{schema}");
    }
    let post = work.iter().find(|t| t.name == "board.work.post").unwrap();
    assert!(post.hints.idempotent && !post.hints.read_only);
    assert!(transitions.hints.idempotent && !transitions.hints.read_only);
    for required in [
        "id",
        "owner_token",
        "request_id",
        "expected_queue_id",
        "expected_revision",
        "operation",
    ] {
        assert!(
            post.input_schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(required)),
            "{}",
            post.input_schema
        );
    }
}

#[tokio::test]
async fn mcp_first_post_preserves_required_null_without_changing_optional_query_nulls() {
    use rupu_mcp::{CallCx, ToolResult, ToolSource};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    seed(&root, "sessions", "ses_work", "idle");
    let router = fixture_app(&root, true, None);
    let page = get(&router, "/api/sessions/ses_work/board/work").await;
    let source = rupu_cp::mcp::operator_source::OperatorSource::new(router);
    let mut args = post_body(&page, "mcp-first", "Review through MCP");
    args["id"] = json!("ses_work");
    args["host"] = Value::Null;
    let result = source
        .call("board.work.post", args.clone(), &CallCx::default())
        .await
        .unwrap();
    let ToolResult::Json(reply) = result else {
        panic!("{result:?}")
    };
    assert_eq!(reply["receipt"]["execution_effect"], "none");
    assert_eq!(
        reply["current_item"]["creator"],
        json!({"kind":"operator","via":"cp"})
    );
    args.as_object_mut().unwrap().remove("request_id");
    let result = source
        .call("board.work.post", args, &CallCx::default())
        .await
        .unwrap();
    assert!(
        matches!(&result,ToolResult::Error(s) if s.contains("missing required argument `request_id`")),
        "{result:?}"
    );
    // An established read operation keeps optional query nulls out of the URL.
    let result = source
        .call(
            "messages.list",
            json!({"id":"ses_work","host":null,"after":null,"limit":null}),
            &CallCx::default(),
        )
        .await
        .unwrap();
    assert!(matches!(result, ToolResult::Json(_)), "{result:?}");
    let result = source
        .call(
            "messages.list",
            json!({"id":null,"host":null}),
            &CallCx::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(&result,ToolResult::Error(s) if s.contains("missing required argument `id`")),
        "{result:?}"
    );
}

#[tokio::test]
async fn completed_owner_refuses_new_actions_but_recovers_an_existing_receipt() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dir = seed(&root, "runs", "run_work", "running");
    let router = fixture_app(&root, true, None);
    let path = "/api/runs/run_work/board/work";
    let page = get(&router, path).await;
    let input = post_body(&page, "before-completion", "Review");
    let (s, original) = request(&router, "POST", path, Some(input.clone()), false).await;
    assert_eq!(s, 200, "{original}");
    let mut record: Value =
        serde_json::from_slice(&std::fs::read(dir.join("run.json")).unwrap()).unwrap();
    record["status"] = json!("completed");
    std::fs::write(dir.join("run.json"), record.to_string()).unwrap();
    let page = get(&router, path).await;
    assert_eq!(page["owner"]["writable"], false);
    let (s, replayed) = request(&router, "POST", path, Some(input), false).await;
    assert_eq!(s, 200, "{replayed}");
    assert_eq!(replayed["receipt"], original["receipt"]);
    assert_eq!(replayed["replayed"], true);
    let (s, e) = request(
        &router,
        "POST",
        path,
        Some(post_body(&page, "after-completion", "No new work")),
        false,
    )
    .await;
    assert_eq!(s, 409, "{e}");
}

#[tokio::test]
async fn capabilities_follow_installed_service_and_record_limits_are_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    seed(&root, "sessions", "ses_work", "idle");
    for writable in [false, true] {
        let router = fixture_app(&root, writable, None);
        let info = get(&router, "/api/host/info").await;
        let features = info["features"].as_array().unwrap();
        assert!(features.contains(&json!("board.work_queue.v1")));
        assert_eq!(
            features.contains(&json!("board.work_queue.mutations.v1")),
            writable
        );
    }
    let router = fixture_app(&root, true, None);
    let path = "/api/sessions/ses_work/board/work";
    let page = get(&router, path).await;
    let (s, e) = request(
        &router,
        "POST",
        path,
        Some(post_body(&page, "too-large", &"x".repeat(257))),
        false,
    )
    .await;
    assert_eq!(s, 413, "{e}");
    assert_eq!(e["error"]["code"], "work_capacity");
    let after = get(&router, path).await;
    assert_eq!(after["snapshot"]["revision"], 0);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_or_replaced_owner_does_not_inherit_write_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dir = seed(&root, "sessions", "ses_work", "idle");
    let router = fixture_app(&root, true, None);
    let path = "/api/sessions/ses_work/board/work";
    let page = get(&router, path).await;
    let old = root.join("saved-owner");
    std::fs::rename(&dir, &old).unwrap();
    std::os::unix::fs::symlink(&old, &dir).unwrap();
    let (s, e) = request(&router, "GET", path, None, false).await;
    assert_eq!(s, 409, "{e}");
    std::fs::remove_file(&dir).unwrap();
    seed(&root, "sessions", "ses_work", "idle");
    let (s, e) = request(
        &router,
        "POST",
        path,
        Some(post_body(&page, "stale-owner", "Review")),
        false,
    )
    .await;
    assert_eq!(s, 409, "{e}");
    assert_eq!(e["error"]["code"], "work_owner_changed");
    assert!(!dir.join("messages").exists());
}
