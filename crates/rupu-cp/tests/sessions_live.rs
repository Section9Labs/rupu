//! e2e: a failed session surfaces last_error / per-run error, and its turn
//! transcript reads + streams. Read-path only (no live LLM / worker).
//!
//! NOTE: rupu_transcript::Event is adjacently tagged (#[serde(tag = "type",
//! content = "data", rename_all = "snake_case")]), so each JSONL line must
//! wrap the payload in a "data" key.
use futures_util::StreamExt as _;
use std::time::Duration;
use tokio::io::AsyncReadExt as _;
use tokio_util::io::StreamReader;

#[tokio::test]
async fn failed_session_surfaces_errors_and_transcript_streams() {
    use axum::http::StatusCode;

    let root = tempfile::tempdir().unwrap();
    let global = root.path().to_path_buf();

    // Turn transcript: assistant_message + run_complete carrying an error.
    // Both events use the adjacently-tagged format: {"type":"..","data":{..}}.
    let tdir = global.join("transcripts");
    std::fs::create_dir_all(&tdir).unwrap();
    let tpath = tdir.join("run_a.jsonl");
    let e0 = r#"{"type":"assistant_message","data":{"content":"working on it"}}"#;
    let e1 = r#"{"type":"run_complete","data":{"run_id":"run_a","status":"error","total_tokens":12,"duration_ms":5,"error":"provider: API error 401"}}"#;
    std::fs::write(&tpath, format!("{e0}\n{e1}\n")).unwrap();

    // session.json: failed session, one run with an error + this transcript.
    let sid = "ses_TEST";
    let sdir = global.join("sessions").join(sid);
    std::fs::create_dir_all(&sdir).unwrap();
    let session_json = serde_json::json!({
        "session_id": sid,
        "agent_name": "triage",
        "status": "failed",
        "last_error": "provider: API error 401",
        "active_run_id": null,
        "runs": [{
            "run_id": "run_a",
            "prompt": "do it",
            "transcript_path": tpath.to_str().unwrap(),
            "status": "error",
            "error": "provider: API error 401"
        }]
    });
    std::fs::write(
        sdir.join("session.json"),
        serde_json::to_vec_pretty(&session_json).unwrap(),
    )
    .unwrap();

    let state = rupu_cp::state::AppState::new(global.clone(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();

    // session DTO surfaces last_error.
    let resp = client
        .get(format!("http://{addr}/api/sessions/{sid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["last_error"], "provider: API error 401");

    // runs row surfaces per-run error.
    let resp = client
        .get(format!("http://{addr}/api/sessions/{sid}/runs"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rows: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(rows[0]["error"], "provider: API error 401");

    // transcript reads the turn (run_complete present).
    let canon = std::fs::canonicalize(&tpath).unwrap();
    let resp = client
        .get(format!("http://{addr}/api/transcript"))
        .query(&[("path", canon.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let t: serde_json::Value = resp.json().await.unwrap();
    let types: Vec<&str> = t["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["type"].as_str())
        .collect();
    assert!(types.contains(&"run_complete"), "events: {types:?}");

    // SSE stream emits a data: line.
    let resp = client
        .get(format!("http://{addr}/api/transcript/stream"))
        .query(&[("path", canon.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let byte_stream = resp
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let mut reader = StreamReader::new(byte_stream);
    let line = tokio::time::timeout(Duration::from_secs(5), async {
        let mut acc = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let n = reader.read(&mut buf).await.unwrap();
            if n == 0 {
                panic!("stream closed before data:");
            }
            acc.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&acc);
            if let Some(l) = text.lines().find(|l| l.starts_with("data:")) {
                return l.to_string();
            }
        }
    })
    .await
    .expect("timed out");
    let json: serde_json::Value =
        serde_json::from_str(line.strip_prefix("data:").unwrap().trim()).unwrap();
    assert!(json["type"].is_string());
}
