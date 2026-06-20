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

/// Write a single transcript event as JSONL to `path` and return its
/// `type` tag for the assertion.
fn write_one_event_jsonl(path: &std::path::Path) -> &'static str {
    let e = Event::RunStart {
        run_id: "run_y".into(),
        workspace_id: "ws_1".into(),
        agent: "rupu-agent".into(),
        provider: "anthropic".into(),
        model: "claude-opus-4-8".into(),
        started_at: chrono::Utc.with_ymd_and_hms(2026, 6, 16, 12, 0, 0).unwrap(),
        mode: RunMode::Ask,
    };
    std::fs::write(path, format!("{}\n", serde_json::to_string(&e).unwrap())).unwrap();
    "run_start"
}

#[tokio::test]
async fn stream_transcript_emits_first_event() {
    use axum::http::StatusCode;
    use futures_util::StreamExt as _;
    use std::time::Duration;
    use tokio::io::AsyncReadExt as _;
    use tokio_util::io::StreamReader;

    let root = tempfile::tempdir().unwrap();
    let global = root.path().to_path_buf();
    let tpath = global.join("transcripts").join("run_y.jsonl");
    std::fs::create_dir_all(tpath.parent().unwrap()).unwrap();
    let want_type = write_one_event_jsonl(&tpath);

    let state = rupu_cp::state::AppState::new(global.clone(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let canon = std::fs::canonicalize(&tpath).unwrap();
    let url = format!("http://{addr}/api/transcript/stream");

    // Streaming read: open the SSE connection and read raw bytes until the
    // first `data:` line appears, then parse the JSON after it.
    let resp = client
        .get(&url)
        .query(&[("path", canon.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let byte_stream = resp
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let mut reader = StreamReader::new(byte_stream);

    // Read incrementally until we have a `data:` line, with a hard timeout so a
    // hang fails rather than blocks forever.
    let data_line = tokio::time::timeout(Duration::from_secs(5), async {
        let mut acc = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let n = reader.read(&mut buf).await.unwrap();
            if n == 0 {
                panic!("stream closed before any data: line");
            }
            acc.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&acc);
            if let Some(line) = text.lines().find(|l| l.starts_with("data:")) {
                return line.to_string();
            }
        }
    })
    .await
    .expect("timed out waiting for first SSE data: line");

    // Drop the connection by dropping the reader/response.
    drop(reader);

    let json_str = data_line.strip_prefix("data:").unwrap().trim();
    let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
    assert_eq!(parsed["type"], want_type);

    // Escape attempt: validation runs first → 400.
    let resp = client
        .get(&url)
        .query(&[("path", "/etc/passwd")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
