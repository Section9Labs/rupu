use super::*;
use rupu_orchestrator::execution_observation::{
    ExecutionIteration, ExecutionOwner, MAX_MEMBERS, MAX_SNAPSHOT_BYTES, SNAPSHOT_FILE,
};
use rupu_orchestrator::runs::RunStatus;

fn executing_fixture() -> (tempfile::TempDir, AppState, PathBuf, ExecutionOwner) {
    let (tmp, s, path) = fixture();
    let mut run = s.run_store.load(ID).unwrap();
    run.status = RunStatus::Running;
    s.run_store.update(&run).unwrap();
    let execution = ExecutionOwner::start(&path, &run).unwrap();
    (tmp, s, path, execution)
}

async fn latest(s: &AppState) -> Value {
    let (status, value) = request(
        s,
        &format!("/api/runs/{ID}/observation?ws_id={WS}&window=latest&refresh=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    value
}

async fn poll(s: &AppState, known: Option<&str>) -> Value {
    let mut uri = format!("/api/runs/{ID}/observation/frontier?ws_id={WS}");
    if let Some(known) = known {
        uri.push_str("&known_frontier=");
        uri.push_str(known);
    }
    let (status, value) = request(s, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["comparison"], "metadata_read_time");
    assert!(value["input_bytes"].as_u64().unwrap() <= 1024 * 1024);
    value
}

fn assert_noncurrent(value: &Value) {
    assert_ne!(value["executing_steps"]["validity"], "current");
    assert_ne!(value["executing_steps"]["reason"], Value::Null);
}

#[tokio::test]
async fn executing_steps_change_frontier_and_source_generation_without_transcript_append() {
    let (_tmp, s, path, owner) = executing_fixture();
    // RunStore::create initializes the result journal. Prove its bytes stay
    // unchanged; absence is not part of the production storage contract.
    let original_results = fs::read(path.join("step_results.jsonl")).unwrap();
    assert!(!path.join("events.jsonl").exists());
    let original = latest(&s).await;
    let first = poll(&s, original["frontier"].as_str()).await;
    assert_eq!(first["changed"], false);
    assert_eq!(first["executing_steps"], original["executing_steps"]);
    let active = owner
        .handle()
        .enter(
            "review",
            vec![ExecutionIteration {
                loop_id: "refine".into(),
                iteration: 2,
            }],
        )
        .unwrap();
    let changed = poll(&s, first["frontier"].as_str()).await;
    let execution = &changed["executing_steps"];
    assert_eq!(changed["changed"], true);
    assert_eq!(execution["schema"], EXECUTING_STEPS_FEATURE);
    assert_eq!(execution["scope"], "scheduler_step_bodies");
    assert_eq!(execution["run_id"], ID);
    assert_eq!(execution["workspace_id"], WS);
    assert_eq!(execution["host_id"], "local");
    assert_eq!(execution["owner_started_at"], original["run"]["started_at"]);
    assert_eq!(execution["validity"], "current");
    assert_eq!(execution["complete"], true);
    assert_eq!(
        execution["execution_generation"],
        original["executing_steps"]["execution_generation"]
    );
    assert!(
        execution["revision"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > original["executing_steps"]["revision"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    );
    assert_eq!(execution["members"][0]["step_id"], "review");
    assert_eq!(execution["members"][0]["iterations"][0]["iteration"], 2);
    assert!(execution["members"][0]["activation_id"].is_string());
    let unchanged = poll(&s, changed["frontier"].as_str()).await;
    assert_eq!(unchanged["changed"], false);
    assert_eq!(unchanged["executing_steps"], *execution);
    let capture = latest(&s).await;
    assert_eq!(capture["frontier"], changed["frontier"]);
    assert_eq!(capture["executing_steps"], *execution);
    assert_ne!(capture["source_generation"], original["source_generation"]);
    assert_eq!(capture["sections"].as_object().unwrap().len(), 6);
    assert!(capture["execution_input_bytes"].as_u64().unwrap() > 0);
    assert!(!path.join("events.jsonl").exists());
    assert_eq!(
        fs::read(path.join("step_results.jsonl")).unwrap(),
        original_results
    );
    active.finish().unwrap();
    owner.finish().unwrap();
}

#[tokio::test]
async fn executing_capture_stays_frozen_when_a_later_activation_leaves() {
    let (_tmp, s, _path, owner) = executing_fixture();
    let first = owner.handle().enter("one", vec![]).unwrap();
    let second = owner.handle().enter("two", vec![]).unwrap();
    let capture = latest(&s).await;
    let snapshot = capture["snapshot"].as_str().unwrap();
    let uri =
        format!("/api/runs/{ID}/observation/page?ws_id={WS}&snapshot={snapshot}&section=steps");
    let (status, page_before) = request(&s, &uri).await;
    assert_eq!(status, StatusCode::OK);
    first.finish().unwrap();
    let changed = poll(&s, capture["frontier"].as_str()).await;
    assert_eq!(changed["changed"], true);
    assert_eq!(
        changed["executing_steps"]["members"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(changed["executing_steps"]["members"][0]["step_id"], "two");
    let (status, page_after) = request(&s, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page_after, page_before);
    let frozen = s.workflow_observations.snapshots.lock().unwrap();
    let retained = frozen.iter().find(|v| v.token == snapshot).unwrap();
    assert_eq!(retained.header, capture);
    assert_eq!(
        retained.header["executing_steps"]["members"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(retained.bytes >= serde_json::to_vec(&retained.header).unwrap().len());
    drop(frozen);
    second.finish().unwrap();
    owner.finish().unwrap();
}

#[tokio::test]
async fn executing_owner_loss_is_qualified_and_changes_frontier() {
    let (_tmp, s, _path, owner) = executing_fixture();
    let active = owner.handle().enter("review", vec![]).unwrap();
    let current = poll(&s, None).await;
    assert_eq!(current["executing_steps"]["validity"], "current");
    drop(owner);
    let lost = poll(&s, current["frontier"].as_str()).await;
    assert_eq!(lost["changed"], true);
    assert_noncurrent(&lost);
    let unchanged = poll(&s, lost["frontier"].as_str()).await;
    assert_eq!(unchanged["changed"], false);
    assert_eq!(unchanged["executing_steps"], lost["executing_steps"]);
    assert_noncurrent(&latest(&s).await);
    drop(active);
}

#[tokio::test]
async fn executing_legacy_owner_has_explicit_unavailable_observation() {
    let (_tmp, s, _path) = fixture();
    let capture = latest(&s).await;
    let execution = &capture["executing_steps"];
    assert_eq!(execution["schema"], EXECUTING_STEPS_FEATURE);
    assert_eq!(execution["validity"], "unavailable");
    assert_eq!(execution["complete"], false);
    assert_eq!(execution["reason"], "observation_missing");
    assert!(execution["execution_generation"].is_null());
    assert!(execution["revision"].is_null());
    assert!(execution["observed_at"].is_null());
    assert_eq!(execution["members"], json!([]));
    assert_eq!(
        poll(&s, capture["frontier"].as_str()).await["changed"],
        false
    );
}

#[tokio::test]
async fn executing_archive_never_reestablishes_current_ownership() {
    let (_tmp, s, path, owner) = executing_fixture();
    owner.finish().unwrap();
    let archive = s.run_store.archive_root();
    fs::create_dir_all(&archive).unwrap();
    fs::rename(&path, archive.join(ID)).unwrap();
    let capture = latest(&s).await;
    assert_eq!(capture["archived"], true);
    assert_eq!(capture["executing_steps"]["validity"], "stale");
    assert_eq!(capture["executing_steps"]["reason"], "archived_record");
    assert_noncurrent(&poll(&s, None).await);
    assert!(!path.exists());
}

#[tokio::test]
async fn executing_wrong_identity_malformed_and_overlimit_records_are_explicit() {
    for defect in ["run", "workspace", "malformed", "bytes", "members"] {
        let (_tmp, s, path, owner) = executing_fixture();
        let active = owner.handle().enter("review", vec![]).unwrap();
        let snapshot = path.join(SNAPSHOT_FILE);
        let mut value: Value = serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
        let bytes = match defect {
            "run" => {
                value["run_id"] = json!("run_other");
                serde_json::to_vec(&value).unwrap()
            }
            "workspace" => {
                value["workspace_id"] = json!("ws_other");
                serde_json::to_vec(&value).unwrap()
            }
            "malformed" => b"{unfinished".to_vec(),
            "bytes" => vec![b' '; MAX_SNAPSHOT_BYTES + 1],
            "members" => {
                let member = value["members"][0].clone();
                value["members"] = Value::Array(
                    (0..=MAX_MEMBERS)
                        .map(|_| {
                            let mut next = member.clone();
                            next["activation_id"] = json!(ulid::Ulid::new().to_string());
                            next
                        })
                        .collect(),
                );
                serde_json::to_vec(&value).unwrap()
            }
            _ => unreachable!(),
        };
        fs::write(&snapshot, bytes).unwrap();
        for observed in [poll(&s, None).await, latest(&s).await] {
            assert_noncurrent(&observed);
            assert_eq!(observed["executing_steps"]["complete"], false, "{defect}");
            assert_eq!(
                observed["executing_steps"]["members"],
                json!([]),
                "{defect}"
            );
            assert!(serde_json::to_vec(&observed).unwrap().len() <= MAX_PAGE);
        }
        drop(active);
        drop(owner);
    }
}

#[tokio::test]
async fn executing_owner_route_rejects_wrong_project_and_remote_host() {
    let (_tmp, s, _path, _owner) = executing_fixture();
    for suffix in ["", "/frontier"] {
        let (status, _) = request(
            &s,
            &format!("/api/runs/{ID}/observation{suffix}?ws_id=ws_other"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = request(
            &s,
            &format!("/api/runs/{ID}/observation{suffix}?ws_id={WS}&host=remote"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    }
}

#[tokio::test]
async fn executing_capability_is_host_discovery_only() {
    let (_tmp, s, _path) = fixture();
    let response = crate::api::host_info::routes()
        .with_state(s)
        .oneshot(
            Request::builder()
                .uri("/api/host/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == EXECUTING_STEPS_FEATURE));
}
