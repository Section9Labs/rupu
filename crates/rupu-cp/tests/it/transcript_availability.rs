//! Availability is a property of the recording, not of its parsed event count.
//! Exercise the production router against disposable files; no worker or model.

use axum::{body::Body, http::Request, response::Response, Router};
use http_body_util::BodyExt;
use rupu_cp::{server::router, state::AppState};
use std::path::Path;
use tower::ServiceExt;

fn app(root: &Path, token: Option<&str>) -> Router {
    router(
        AppState::new(root.to_path_buf(), rupu_config::PricingConfig::default()),
        token.map(str::to_owned),
    )
}

fn request(route: &str, path: &Path, extra: &[(&str, &str)]) -> Request<Body> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("path", &path.to_string_lossy());
    query.extend_pairs(extra.iter().copied());
    Request::builder()
        .uri(format!("{route}?{}", query.finish()))
        .body(Body::empty())
        .unwrap()
}

async fn json(response: Response) -> (u16, serde_json::Value) {
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn event() -> String {
    serde_json::to_string(&rupu_transcript::Event::AssistantMessage {
        content: "recorded message".into(),
        thinking: None,
    })
    .unwrap()
}

#[tokio::test]
async fn missing_snapshot_matches_paged_unavailability_but_empty_file_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("not-created/run.jsonl");
    let app = app(root.path(), None);
    for route in ["/api/transcript", "/api/transcript/page"] {
        for extra in [vec![], vec![("host", "local"), ("run", "run_recorded")]] {
            let (status, body) = json(
                app.clone()
                    .oneshot(request(route, &path, &extra))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(status, 404, "{route}: {body}");
            assert_eq!(
                body,
                serde_json::json!({"error": "recorded transcript is not available yet"})
            );
        }
    }
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    let (status, body) = json(
        app.oneshot(request("/api/transcript", &path, &[]))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        serde_json::json!({"events": [], "summary": null, "unparsed": 0})
    );
}

#[tokio::test]
async fn recorded_events_and_malformed_tail_policy_are_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("run.jsonl");
    std::fs::write(
        &path,
        format!("\n{}\nnot json\n{}\n{{\"type\":", event(), event()),
    )
    .unwrap();
    let (status, body) = json(
        app(root.path(), None)
            .oneshot(request("/api/transcript", &path, &[]))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["events"].as_array().unwrap().len(), 2);
    assert_eq!(body["events"][0]["data"]["content"], "recorded message");
    assert_eq!(
        body["unparsed"], 1,
        "mid-file failures count; a torn last line does not"
    );
}

#[tokio::test]
async fn authentication_precedes_availability_and_correct_token_gets_404() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("missing.jsonl");
    let app = app(root.path(), Some("fixture-token"));
    for token in [None, Some("Bearer wrong"), Some("Bearer fixture-token")] {
        let mut req = request("/api/transcript", &path, &[]);
        if let Some(token) = token {
            req.headers_mut()
                .insert("authorization", token.parse().unwrap());
        }
        let (status, body) = json(app.clone().oneshot(req).await.unwrap()).await;
        assert_eq!(
            status,
            if token == Some("Bearer fixture-token") {
                404
            } else {
                401
            }
        );
        assert!(body.get("events").is_none());
    }
}

#[tokio::test]
async fn missing_paths_still_clear_root_extension_traversal_and_host_guards() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let app = app(root.path(), None);
    for path in [
        outside.path().join("missing.jsonl"),
        root.path().join("missing.txt"),
        root.path().join("nested/../missing.jsonl"),
    ] {
        let (status, body) = json(
            app.clone()
                .oneshot(request("/api/transcript", &path, &[]))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert!(body.get("events").is_none());
    }
    let (status, body) = json(
        app.oneshot(request(
            "/api/transcript",
            &root.path().join("missing.jsonl"),
            &[("host", "host_unknown")],
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(body["error"], "host host_unknown not found");
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_rejected_even_for_missing_targets() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
    let (status, body) = json(
        app(root.path(), None)
            .oneshot(request(
                "/api/transcript",
                &root.path().join("linked/missing.jsonl"),
                &[],
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "path is outside the allowed roots");
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_file_is_an_error_not_empty_history() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("run.jsonl");
    std::fs::write(&path, event()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
    // Root may bypass mode bits; do not claim a permission-denied fixture then.
    if std::fs::File::open(&path).is_ok() {
        eprintln!("mode bits do not deny reads for this user; skipping");
        return;
    }
    let (status, body) = json(
        app(root.path(), None)
            .oneshot(request("/api/transcript", &path, &[]))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 500);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("Permission denied"));
    assert!(body.get("events").is_none());
}

#[tokio::test]
async fn streaming_can_open_before_the_recording_exists_and_deliver_later() {
    use futures_util::StreamExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pending.jsonl");
    let response = app(root.path(), None)
        .oneshot(request("/api/transcript/stream", &path, &[]))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut stream = response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), stream.next())
            .await
            .is_err()
    );
    std::fs::write(&path, format!("{}\n", event())).unwrap();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = std::str::from_utf8(&frame).unwrap();
    assert!(text.contains("id: 1"), "{text}");
    assert!(text.contains("recorded message"), "{text}");
}

#[tokio::test]
async fn ssh_legacy_fallback_preserves_missing_and_path_errors_without_connecting() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(
        root.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    let host = state
        .hosts
        .add_ssh_host("fixture", "unused.invalid", None, None)
        .unwrap();
    let app = router(state, None);
    // Without a run id, this takes the existing local legacy fallback, never SSH.
    let path = root.path().join("missing.jsonl");
    let (status, body) = json(
        app.clone()
            .oneshot(request("/api/transcript", &path, &[("host", &host.id)]))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"], "recorded transcript is not available yet");
    let outside = tempfile::tempdir().unwrap();
    let (status, _) = json(
        app.oneshot(request(
            "/api/transcript",
            &outside.path().join("missing.jsonl"),
            &[("host", &host.id)],
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn completed_cache_marker_does_not_make_a_missing_file_empty() {
    let root = tempfile::tempdir().unwrap();
    let state = AppState::new(
        root.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    let host = state
        .hosts
        .add_ssh_host("fixture", "unused.invalid", None, None)
        .unwrap();
    let path = Path::new("/remote/transcripts/run_recorded.jsonl");
    let cache = rupu_cp::host::transcript_paths::cache_path(root.path(), &host.id, path).unwrap();
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(rupu_cp::host::transcript_paths::complete_marker(&cache), "").unwrap();
    let app = router(state, None);
    let (status, body) = json(
        app.clone()
            .oneshot(request("/api/transcript", path, &[("host", &host.id)]))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    std::fs::write(cache, "").unwrap();
    let (status, body) = json(
        app.oneshot(request("/api/transcript", path, &[("host", &host.id)]))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["events"], serde_json::json!([]));
}
