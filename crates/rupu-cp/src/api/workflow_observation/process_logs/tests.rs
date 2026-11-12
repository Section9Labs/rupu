use super::*;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use std::fs;
use tower::ServiceExt;
const ID: &str = "run_logs_fixture";
const WS: &str = "ws_fixture";
fn fixture() -> (tempfile::TempDir, AppState, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let s = AppState::new(
        temp.path().canonicalize().unwrap(),
        rupu_config::PricingConfig::default(),
    );
    let run=serde_json::from_value(json!({"id":ID,"workspace_id":WS,"workflow_name":"review","status":"completed","inputs":{},"workspace_path":"/fixture","transcript_dir":"/fixture/transcripts","started_at":"2026-11-09T10:00:00Z"})).unwrap();
    s.run_store
        .create(run, "name: review\nsteps: []\n")
        .unwrap();
    let path = s.run_store.root.join(ID);
    (temp, s, path)
}
fn query(source: Source) -> QueryParams {
    QueryParams {
        ws_id: WS.into(),
        host: Some("local".into()),
        source,
        snapshot: None,
        cursor: None,
    }
}
fn read(s: &AppState, source: Source) -> Capture {
    capture(s, ID, &query(source), &AtomicBool::new(false)).unwrap()
}
#[test]
fn sources_are_exact_and_missing_does_not_mean_empty_history() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("events.jsonl"), "not process output").unwrap();
    let absent = read(&s, Source::Retry);
    assert_eq!(absent.header["state"], "missing");
    assert_eq!(absent.header["history_complete"], false);
    fs::write(path.join("session-launch.log"), "launch output").unwrap();
    fs::write(path.join("retry.log"), "").unwrap();
    assert_eq!(read(&s, Source::SessionLaunch).text, "launch output");
    let empty = read(&s, Source::Retry);
    assert_eq!(empty.header["state"], "available");
    assert_eq!(empty.text, "");
}
#[test]
fn suffix_and_unicode_pages_reconstruct_only_the_bounded_capture() {
    let (_tmp, s, path) = fixture();
    let text = "🦀".repeat(CAPTURE_BYTES);
    fs::write(path.join("retry.log"), &text).unwrap();
    let c = read(&s, Source::Retry);
    assert_eq!(c.text.len(), CAPTURE_BYTES);
    assert_eq!(c.header["head_truncated"], true);
    assert_eq!(
        c.header["captured_start"],
        (text.len() - CAPTURE_BYTES).to_string()
    );
    let mut cursor = None;
    let mut joined = String::new();
    loop {
        let page = c.page(cursor.as_deref()).unwrap();
        let part = page["text"].as_str().unwrap();
        assert!(part.len() <= PAGE_BYTES);
        assert!(serde_json::to_vec(&page).unwrap().len() < 128 * 1024);
        joined.push_str(part);
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(joined, c.text);
    assert!(!joined.contains('\u{fffd}'));
}
#[test]
fn invalid_bytes_are_explicit_and_projection_is_bounded() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("retry.log"), vec![255; CAPTURE_BYTES + 1]).unwrap();
    let c = read(&s, Source::Retry);
    assert_eq!(c.header["invalid_utf8"], true);
    assert_eq!(c.text.len(), ENCODED_CAPTURE_BYTES);
    assert_eq!(c.header["process_host"], "unknown");
}
#[test]
fn traversal_foreign_project_remote_and_agent_owners_refuse() {
    let (_tmp, s, path) = fixture();
    assert!(owner("../run", WS, Some("local")).is_err());
    assert!(owner(ID, WS, Some("remote")).is_err());
    let mut q = query(Source::Retry);
    q.ws_id = "ws_foreign".into();
    assert!(capture(&s, ID, &q, &AtomicBool::new(false)).is_err());
    let mut record: Value =
        serde_json::from_slice(&fs::read(path.join("run.json")).unwrap()).unwrap();
    record["host_id"] = json!("remote");
    fs::write(path.join("run.json"), record.to_string()).unwrap();
    assert!(capture(&s, ID, &query(Source::Retry), &AtomicBool::new(false)).is_err());
    record.as_object_mut().unwrap().remove("host_id");
    record["workflow_name"] = json!("agent:review");
    fs::write(path.join("run.json"), record.to_string()).unwrap();
    assert!(capture(&s, ID, &query(Source::Retry), &AtomicBool::new(false)).is_err());
}
#[test]
fn symlinks_directories_and_fifo_are_refused_without_blocking() {
    let (_tmp, s, path) = fixture();
    let log = path.join("retry.log");
    std::os::unix::fs::symlink("run.json", &log).unwrap();
    assert!(capture(&s, ID, &query(Source::Retry), &AtomicBool::new(false)).is_err());
    fs::remove_file(&log).unwrap();
    fs::create_dir(&log).unwrap();
    assert!(capture(&s, ID, &query(Source::Retry), &AtomicBool::new(false)).is_err());
    fs::remove_dir(&log).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(&log)
        .status()
        .unwrap()
        .success());
    assert!(capture(&s, ID, &query(Source::Retry), &AtomicBool::new(false)).is_err());
}
#[test]
fn cursors_bind_owner_source_capture_and_utf8_boundaries() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("retry.log"), "🦀".repeat(10000)).unwrap();
    let c = read(&s, Source::Retry);
    let token = c.cursor(PAGE_BYTES);
    assert!(c.page(Some(&token)).is_ok());
    assert!(read(&s, Source::Retry).page(Some(&token)).is_err());
    assert!(read(&s, Source::SessionLaunch).page(Some(&token)).is_err());
    assert!(c.page(Some(&c.cursor(1))).is_err());
    assert!(c.page(Some("999999.invalid")).is_err());
}
#[test]
fn cancelled_capture_never_returns_a_snapshot() {
    let (_tmp, s, _path) = fixture();
    assert!(capture(&s, ID, &query(Source::Retry), &AtomicBool::new(true)).is_err());
}
#[test]
fn archived_owner_is_explicit() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("retry.log"), "archived diagnostic").unwrap();
    fs::create_dir_all(s.run_store.archive_root()).unwrap();
    fs::rename(&path, s.run_store.archive_root().join(ID)).unwrap();
    let c = read(&s, Source::Retry);
    assert_eq!(c.header["archived"], true);
    assert_eq!(c.header["store_kind"], "global_archive");
}
async fn request(s: &AppState, suffix: &str) -> (StatusCode, Value) {
    let response = routes()
        .with_state(s.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/runs/{ID}/process-log?ws_id={WS}&host=local&{suffix}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn immutable_replay_and_eviction_do_not_reread_log() {
    let (_tmp, s, path) = fixture();
    fs::write(path.join("retry.log"), "old".repeat(10000)).unwrap();
    let (status, first) = request(&s, "source=retry").await;
    assert_eq!(status, StatusCode::OK);
    let snapshot = first["header"]["snapshot"].as_str().unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();
    fs::remove_file(path.join("retry.log")).unwrap();
    let (status, next) = request(
        &s,
        &format!("source=retry&snapshot={snapshot}&cursor={cursor}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["offset"], PAGE_BYTES);
    for _ in 0..4 {
        assert_eq!(request(&s, "source=retry").await.0, StatusCode::OK);
    }
    assert_eq!(
        request(&s, &format!("source=retry&snapshot={snapshot}"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        s.workflow_observations
            .process_logs
            .entries
            .lock()
            .unwrap()
            .len(),
        4
    );
}
#[tokio::test]
async fn typed_query_rejects_arbitrary_paths_and_orphan_cursor() {
    let (_tmp, s, _path) = fixture();
    assert_eq!(
        request(&s, "source=../../auth.json").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&s, "source=retry&path=auth.json").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&s, "source=retry&cursor=x").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn cancellation_after_capture_does_not_publish_or_evict_previous() {
    let (_tmp, s, _path) = fixture();
    let store = &s.workflow_observations.process_logs;
    let previous = Arc::new(read(&s, Source::Retry));
    publish(store, previous.clone(), &AtomicBool::new(false)).unwrap();
    let candidate = Arc::new(read(&s, Source::Retry));
    assert!(publish(store, candidate, &AtomicBool::new(true)).is_err());
    let cache = store.entries.lock().unwrap();
    assert_eq!(cache.len(), 1);
    assert!(Arc::ptr_eq(&cache[0], &previous));
}
#[tokio::test]
async fn expired_capture_is_not_replayed() {
    let (_tmp, s, _path) = fixture();
    let mut c = read(&s, Source::Retry);
    let token = c.snapshot.clone();
    c.born = Instant::now() - TTL - Duration::from_secs(1);
    s.workflow_observations
        .process_logs
        .entries
        .lock()
        .unwrap()
        .push(Arc::new(c));
    assert_eq!(
        request(&s, &format!("source=retry&snapshot={token}"))
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[test]
fn final_owner_and_pathname_changes_refuse_capture() {
    for (key, value) in [
        ("host_id", json!("remote")),
        ("host", json!(7)),
        ("workflow_name", json!("other")),
        ("workspace_path", json!("/other")),
    ] {
        let (_tmp, s, path) = fixture();
        fs::write(path.join("retry.log"), "log").unwrap();
        let result = capture_checked(
            &s,
            ID,
            &query(Source::Retry),
            &AtomicBool::new(false),
            || {
                let mut record: Value =
                    serde_json::from_slice(&fs::read(path.join("run.json")).unwrap()).unwrap();
                record[key] = value;
                fs::write(path.join("run.json"), record.to_string()).unwrap();
            },
        );
        assert!(result.is_err());
    }
    let (_tmp, s, path) = fixture();
    fs::write(path.join("retry.log"), "old").unwrap();
    assert!(capture_checked(
        &s,
        ID,
        &query(Source::Retry),
        &AtomicBool::new(false),
        || {
            fs::rename(path.join("retry.log"), path.join("previous.log")).unwrap();
            fs::write(path.join("retry.log"), "new").unwrap();
        }
    )
    .is_err());
}
#[test]
fn registered_project_local_owner_and_symlinked_directory_boundary() {
    let (tmp, s, path) = fixture();
    fs::write(path.join("retry.log"), "project diagnostic").unwrap();
    let project = tmp.path().canonicalize().unwrap().join("project");
    fs::create_dir_all(project.join(".rupu/runs")).unwrap();
    let local = project.join(".rupu/runs").join(ID);
    fs::rename(&path, &local).unwrap();
    fs::create_dir_all(s.global_dir.join("workspaces")).unwrap();
    let record: rupu_workspace::Workspace =
        serde_json::from_value(json!({"id":WS,"path":project,"created_at":"2026-11-09T10:00:00Z"}))
            .unwrap();
    let registration = s.global_dir.join("workspaces").join(format!("{WS}.toml"));
    fs::write(&registration, toml::to_string(&record).unwrap()).unwrap();
    fs::write(s.global_dir.join("workspaces/unrelated.toml"), "invalid").unwrap();
    let c = read(&s, Source::Retry);
    assert_eq!(c.header["store_kind"], "project_local");
    assert_eq!(c.text, "project diagnostic");
    let moved = project.join(".rupu/runs/moved");
    fs::rename(&local, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &local).unwrap();
    assert!(capture(&s, ID, &query(Source::Retry), &AtomicBool::new(false)).is_err());
    fs::remove_file(&local).unwrap();
    fs::rename(&moved, &local).unwrap();
    fs::write(
        &registration,
        toml::to_string(&rupu_workspace::Workspace {
            id: "ws_other".into(),
            ..record
        })
        .unwrap(),
    )
    .unwrap();
    assert!(capture(&s, ID, &query(Source::Retry), &AtomicBool::new(false)).is_err());
}
