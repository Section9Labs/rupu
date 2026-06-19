use chrono::Utc;
use rupu_runtime::{
    AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
    AutoflowHistoryStore,
};

/// Construct an AppState rooted at `dir` and spin up an axum test server.
async fn spawn_server(dir: &std::path::Path) -> std::net::SocketAddr {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Seed a minimal cycle with known counts into a store rooted at
/// `<global_dir>/autoflows/history` (the path the handler uses).
fn seed_cycle(global_dir: &std::path::Path) -> AutoflowCycleRecord {
    let store_root = global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);

    let now = Utc::now();
    let mut cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, now);
    cycle.workflow_count = 3;
    cycle.ran_cycles = 2;
    cycle.skipped_cycles = 1;
    cycle.failed_cycles = 0;
    cycle.finished_at = now.to_rfc3339();
    // Attach an event that carries a run_id so we can verify harvesting.
    cycle.events.push(AutoflowCycleEvent {
        kind: AutoflowCycleEventKind::RunLaunched,
        run_id: Some("run_autoflow_seeded_01".into()),
        ..Default::default()
    });

    store.save(&cycle).unwrap();
    cycle
}

#[tokio::test]
async fn list_autoflows_returns_seeded_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let seeded = seed_cycle(tmp.path());

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert_eq!(arr.len(), 1, "expected exactly one cycle; got {}", arr.len());

    let row = &arr[0];
    assert_eq!(
        row["cycle_id"].as_str(),
        Some(seeded.cycle_id.as_str()),
        "cycle_id mismatch"
    );
    assert_eq!(row["mode"].as_str(), Some("tick"), "mode should be 'tick'");
    assert_eq!(
        row["workflow_count"].as_u64(),
        Some(3),
        "workflow_count mismatch"
    );
    assert_eq!(
        row["ran_cycles"].as_u64(),
        Some(2),
        "ran_cycles mismatch"
    );
    assert_eq!(
        row["skipped_cycles"].as_u64(),
        Some(1),
        "skipped_cycles mismatch"
    );
    assert_eq!(
        row["failed_cycles"].as_u64(),
        Some(0),
        "failed_cycles mismatch"
    );

    // The run_id from the embedded event should be surfaced.
    let run_ids = row["run_ids"].as_array().expect("run_ids should be an array");
    assert_eq!(run_ids.len(), 1, "expected one run_id; got {run_ids:?}");
    assert_eq!(
        run_ids[0].as_str(),
        Some("run_autoflow_seeded_01"),
        "run_id value mismatch"
    );
}

#[tokio::test]
async fn list_autoflows_empty_when_no_store_dir() {
    // Spin up a server with a global_dir that has NO autoflows/history subdir.
    let tmp = tempfile::tempdir().unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "missing store dir should return 200, not 500"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(
        arr.is_empty(),
        "no cycles seeded, response should be []; got {arr:?}"
    );
}
