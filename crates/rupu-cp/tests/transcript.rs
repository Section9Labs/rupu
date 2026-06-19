//! Tests for `GET /api/transcript` and its security-critical path validator.

use chrono::TimeZone as _;
use rupu_cp::api::transcript::validate_transcript_path as v;
use rupu_transcript::{Event, RunMode};

/// Build two real transcript events and write them as JSONL to `path`.
fn write_two_event_jsonl(path: &std::path::Path) {
    let e0 = Event::RunStart {
        run_id: "run_x".into(),
        workspace_id: "ws_1".into(),
        agent: "rupu-agent".into(),
        provider: "anthropic".into(),
        model: "claude-opus-4-8".into(),
        started_at: chrono::Utc.with_ymd_and_hms(2026, 6, 16, 12, 0, 0).unwrap(),
        mode: RunMode::Ask,
    };
    let e1 = Event::AssistantMessage {
        content: "hello".into(),
        thinking: None,
    };
    let body = format!(
        "{}\n{}",
        serde_json::to_string(&e0).unwrap(),
        serde_json::to_string(&e1).unwrap()
    );
    std::fs::write(path, body).unwrap();
}

#[test]
fn validator_accepts_in_root_jsonl_rejects_escape() {
    let root = tempfile::tempdir().unwrap();
    let global = root.path().to_path_buf();
    let good = global.join("transcripts").join("run_x.jsonl");
    std::fs::create_dir_all(good.parent().unwrap()).unwrap();
    std::fs::write(&good, "").unwrap();
    let roots = vec![global.clone()];

    // in-root .jsonl → accepted
    assert!(v(good.to_str().unwrap(), &roots).is_ok());
    // out-of-root absolute → rejected
    assert!(v("/etc/passwd", &roots).is_err());
    // traversal escaping the root → rejected
    let esc = format!("{}/transcripts/../../etc/passwd", global.display());
    assert!(v(&esc, &roots).is_err());
    // non-.jsonl → rejected
    let txt = global.join("transcripts/run_x.txt");
    std::fs::write(&txt, "").unwrap();
    assert!(v(txt.to_str().unwrap(), &roots).is_err());
}

#[tokio::test]
async fn get_transcript_returns_events() {
    use axum::http::StatusCode;

    let root = tempfile::tempdir().unwrap();
    let global = root.path().to_path_buf();
    let tpath = global.join("transcripts").join("run_x.jsonl");
    std::fs::create_dir_all(tpath.parent().unwrap()).unwrap();
    write_two_event_jsonl(&tpath);

    let state = rupu_cp::state::AppState::new(global.clone(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);

    // Serve on an ephemeral port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Happy path: valid in-root transcript → 200, 2 events, first is run_start.
    let canon = std::fs::canonicalize(&tpath).unwrap();
    let url = format!("http://{addr}/api/transcript");
    let resp = client
        .get(&url)
        .query(&[("path", canon.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "run_start");

    // Escape attempt: absolute path outside the allowed roots → 400.
    let resp = client
        .get(&url)
        .query(&[("path", "/etc/passwd")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
