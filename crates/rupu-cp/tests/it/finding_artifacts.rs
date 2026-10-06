//! The coordinator's `GET /api/findings/:id/artifacts/:sha256` for an
//! artifact recorded `external` on another host: pulled from that host on
//! first view (one shared, cancel-safe pull per blob, verified by size and
//! sha256 before it enters the store), then served; any failure is a 404
//! `{"unavailable": "<reason>"}` (spec
//! 2026-09-30-rupu-remote-findings-transport-design.md §B2).

// Throwaway in-process clients and mock remotes, not rupu's own egress.
#![allow(clippy::disallowed_methods)]

use rupu_coverage::report::{
    ArtifactKind, ArtifactRef, ArtifactStorage, ArtifactStore, FindingReport,
};
use rupu_coverage::{
    Attribution, CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingScope,
    Ledger, Severity, Surface,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn sha_of(bytes: &[u8]) -> String {
    rupu_coverage::report::sha256_reader(&mut &bytes[..]).unwrap()
}

fn register_workspace(global: &Path, id: &str, root: &Path) {
    let dir = global.join("workspaces");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{id}.toml")),
        format!(
            "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            root.display()
        ),
    )
    .unwrap();
}

fn finding_with(id: &str, artifact: ArtifactRef) -> FindingRecord {
    let mut report: FindingReport = serde_json::from_str(include_str!(
        "../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
    .unwrap();
    report.artifacts = vec![artifact];
    FindingRecord {
        id: id.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: FindingScope::File,
        summary: "s".into(),
        severity: Severity::High,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: "run_A".into(),
            model: "m".into(),
            surface: Surface::Agent,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: chrono::Utc::now(),
        profile: FindingProfile::Full,
        report: Some(report),
        tags: Vec::new(),
    }
}

/// Append a finding whose report lists only `artifact` to workspace `ws`.
fn write_finding(ws: &Path, id: &str, artifact: ArtifactRef) -> String {
    let rec = finding_with(id, artifact);
    let paths = CoveragePaths::new(ws, "t1");
    paths.ensure_dir().unwrap();
    rupu_coverage::append_record(&paths, Ledger::Findings, &rec).unwrap();
    rec.id
}

fn store_root(global: &Path) -> PathBuf {
    global.join("findings").join("artifacts")
}

fn store_blob(global: &Path, sha: &str, body: &[u8]) {
    let p = ArtifactStore::new(store_root(global)).blob_path(sha);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn stored(global: &Path, sha: &str) -> PathBuf {
    ArtifactStore::new(store_root(global)).blob_path(sha)
}

/// Every `.pull-*` temp file anywhere under the coordinator's store.
fn pull_leftovers(global: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if e.file_name().to_string_lossy().starts_with(".pull-") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&store_root(global), &mut out);
    out
}

async fn serve(state: rupu_cp::state::AppState) -> std::net::SocketAddr {
    serve_router(rupu_cp::server::router(state, None)).await
}

async fn serve_router(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn state(global: &Path) -> rupu_cp::state::AppState {
    rupu_cp::state::AppState::new(global.to_path_buf(), rupu_config::PricingConfig::default())
}

fn artifact(sha: &str, size: u64, kind: ArtifactKind, host: Option<&str>) -> ArtifactRef {
    ArtifactRef {
        path: "poc/exploit.py".into(),
        sha256: sha.into(),
        size,
        kind: Some(kind),
        stored: Some(ArtifactStorage::External),
        host: host.map(str::to_string),
    }
}

/// A coordinator with workspace `ws_a` at `ws` and one HTTP host at
/// `remote_url`; returns its state (not yet served) and the host id.
fn coordinator(global: &Path, ws: &Path, remote_url: &str) -> (rupu_cp::state::AppState, String) {
    register_workspace(global, "ws_a", ws);
    let st = state(global);
    let host = st.hosts.add_host("remote", remote_url, None).unwrap();
    (st, host.id)
}

/// A mock remote that advertises the blob feature; each test mocks the blob.
async fn mock_remote() -> httpmock::MockServer {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "9.9.9", "features": ["findings.artifact_blob"]
        }));
    });
    server
}

async fn unavailable_reason(resp: reqwest::Response) -> String {
    assert_eq!(resp.status(), 404);
    let v: serde_json::Value = resp.json().await.unwrap();
    v["unavailable"]
        .as_str()
        .unwrap_or_else(|| panic!("an `unavailable` body: {v}"))
        .to_string()
}

#[tokio::test]
async fn a_remote_artifact_is_pulled_verified_stored_then_served() {
    // The remote: a real CP whose store holds the blob.
    let remote = tempfile::tempdir().unwrap();
    let body = b"print('poc')\n";
    let sha = sha_of(body);
    store_blob(remote.path(), &sha, body);
    let remote_addr = serve(state(remote.path())).await;

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &format!("http://{remote_addr}"));
    let id = write_finding(
        ws.path(),
        "find_ART1",
        artifact(&sha, body.len() as u64, ArtifactKind::Text, Some(&host)),
    );
    let addr = serve(st).await;

    let resp = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{sha}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let h = resp.headers().clone();
    assert_eq!(h["content-type"], "text/plain; charset=utf-8");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["content-security-policy"], "sandbox");
    assert_eq!(h["content-disposition"], "inline; filename=\"exploit.py\"");
    assert_eq!(h["content-length"], body.len().to_string().as_str());
    assert_eq!(resp.bytes().await.unwrap().as_ref(), body);
    // Now in the coordinator's own store, and nothing left behind.
    assert_eq!(std::fs::read(stored(coord.path(), &sha)).unwrap(), body);
    assert!(pull_leftovers(coord.path()).is_empty());
}

#[tokio::test]
async fn an_empty_remote_artifact_pulls_and_serves() {
    // A recorded size of 0 is a genuinely empty file, not "size unknown".
    let remote = tempfile::tempdir().unwrap();
    let sha = sha_of(b"");
    store_blob(remote.path(), &sha, b"");
    let remote_addr = serve(state(remote.path())).await;

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &format!("http://{remote_addr}"));
    let id = write_finding(
        ws.path(),
        "find_EMPTY",
        artifact(&sha, 0, ArtifactKind::Binary, Some(&host)),
    );
    let addr = serve(st).await;

    let resp = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{sha}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-length"], "0");
    assert!(resp.bytes().await.unwrap().is_empty());
    assert_eq!(std::fs::read(stored(coord.path(), &sha)).unwrap(), b"");
}

#[tokio::test]
async fn concurrent_first_views_share_one_pull() {
    static BODY: &[u8] = b"binary\x00poc";
    let sha = sha_of(BODY);
    let server = mock_remote().await;
    // Slow enough that both views are waiting while the pull runs.
    let blob = server.mock(|when, then| {
        when.method("GET")
            .path(format!("/api/findings/artifacts/{sha}"));
        then.status(200)
            .delay(Duration::from_millis(300))
            .body(BODY);
    });

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &server.base_url());
    let id = write_finding(
        ws.path(),
        "find_ART2",
        artifact(&sha, BODY.len() as u64, ArtifactKind::Binary, Some(&host)),
    );
    let addr = serve(st).await;
    let url = format!("http://{addr}/api/findings/{id}/artifacts/{sha}");
    let (a, b, c) = tokio::join!(reqwest::get(&url), reqwest::get(&url), reqwest::get(&url));
    for r in [a.unwrap(), b.unwrap(), c.unwrap()] {
        assert_eq!(r.status(), 200);
        let h = r.headers().clone();
        assert_eq!(h["content-type"], "application/octet-stream");
        assert_eq!(
            h["content-disposition"],
            "attachment; filename=\"exploit.py\""
        );
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert_eq!(h["content-security-policy"], "sandbox");
        assert_eq!(r.bytes().await.unwrap().as_ref(), BODY);
    }
    blob.assert_hits(1);
}

#[tokio::test]
async fn a_hash_mismatch_is_unavailable_and_stores_nothing() {
    // The finding recorded the sha of "expected!", but the host answers with
    // different bytes of the same length.
    let recorded = sha_of(b"expected!");
    let server = mock_remote().await;
    server.mock(|when, then| {
        when.method("GET")
            .path(format!("/api/findings/artifacts/{recorded}"));
        then.status(200).body("tampered!");
    });

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &server.base_url());
    let id = write_finding(
        ws.path(),
        "find_TAMPERED",
        artifact(&recorded, 9, ArtifactKind::Binary, Some(&host)),
    );
    let addr = serve(st).await;
    let r = reqwest::get(format!(
        "http://{addr}/api/findings/{id}/artifacts/{recorded}"
    ))
    .await
    .unwrap();
    let reason = unavailable_reason(r).await;
    assert!(reason.contains("mismatch"), "{reason}");
    assert!(reason.contains(&host), "names the host: {reason}");
    assert!(reason.contains(&sha_of(b"tampered!")), "{reason}");
    assert!(
        !stored(coord.path(), &recorded).exists(),
        "a mismatched blob must never enter the store"
    );
    assert!(
        pull_leftovers(coord.path()).is_empty(),
        "temp file removed: {:?}",
        pull_leftovers(coord.path())
    );
}

#[tokio::test]
async fn an_oversize_remote_answer_is_unavailable_and_stores_nothing() {
    // A remote that streams (chunked, no declared length) more bytes than the
    // finding recorded: the first chunk lands in the temp file, the second
    // overruns the cap mid-transfer.
    let recorded = b"four";
    let sha = sha_of(recorded);
    let remote = axum::Router::new()
        .route(
            "/api/host/info",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "version": "9.9.9", "features": ["findings.artifact_blob"]
                }))
            }),
        )
        .route(
            "/api/findings/artifacts/:sha",
            axum::routing::get(|| async {
                let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![
                    Ok(bytes::Bytes::from_static(b"fou")),
                    Ok(bytes::Bytes::from_static(b"r and then some")),
                ];
                axum::body::Body::from_stream(futures_util::stream::iter(chunks))
            }),
        );
    let remote_addr = serve_router(remote).await;

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &format!("http://{remote_addr}"));
    let id = write_finding(
        ws.path(),
        "find_BIG",
        artifact(
            &sha,
            recorded.len() as u64,
            ArtifactKind::Binary,
            Some(&host),
        ),
    );
    let addr = serve(st).await;
    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{sha}"))
        .await
        .unwrap();
    let reason = unavailable_reason(r).await;
    assert!(reason.contains(&host), "names the host: {reason}");
    assert!(reason.contains("exceeds"), "{reason}");
    assert!(!stored(coord.path(), &sha).exists());
    assert!(
        pull_leftovers(coord.path()).is_empty(),
        "partial temp file removed: {:?}",
        pull_leftovers(coord.path())
    );
}

#[tokio::test]
async fn a_blob_missing_on_the_remote_is_unavailable_naming_the_host() {
    let remote = tempfile::tempdir().unwrap();
    let remote_addr = serve(state(remote.path())).await;
    let sha = sha_of(b"never uploaded");

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &format!("http://{remote_addr}"));
    let id = write_finding(
        ws.path(),
        "find_GONE",
        artifact(&sha, 14, ArtifactKind::Binary, Some(&host)),
    );
    let addr = serve(st).await;
    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{sha}"))
        .await
        .unwrap();
    let reason = unavailable_reason(r).await;
    assert!(reason.contains(&host), "{reason}");
    assert!(
        reason.contains(&format!(
            "artifact {sha} is not in host http://{remote_addr}'s store"
        )),
        "{reason}"
    );
    assert!(
        !reason.contains("/api/"),
        "no bare URL as the reason: {reason}"
    );
    assert!(!stored(coord.path(), &sha).exists());
    assert!(pull_leftovers(coord.path()).is_empty());
}

/// The recorded size caps the pull, but it comes from an agent-writable
/// ledger: a forged size of many gigabytes would let a host (or a bucket
/// writer) fill this control plane's disk before the hash check fails. A
/// recorded size over the coordinator's own `[findings].artifact_max_bytes`
/// (default `DEFAULT_ARTIFACT_MAX_BYTES`) is refused as unavailable, naming
/// the limit, without contacting the host.
#[tokio::test]
async fn a_recorded_size_over_the_coordinators_limit_is_unavailable_without_a_pull() {
    let server = httpmock::MockServer::start_async().await;
    let info = server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "9.9.9", "features": ["findings.artifact_blob"]
        }));
    });
    let blob = server.mock(|when, then| {
        when.method("GET")
            .path_matches(httpmock::Regex::new(r"^/api/findings/artifacts/").unwrap());
        then.status(200).body("whatever");
    });
    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &server.base_url());
    let small = sha_of(b"fourteen bytes");
    let id = write_finding(
        ws.path(),
        "find_OVERCAP",
        artifact(&small, 14, ArtifactKind::Binary, Some(&host)),
    );
    // The coordinator's limit (8 bytes) is under the recorded 14.
    st.config.write().unwrap().findings.artifact_max_bytes = Some(8);
    let addr = serve(st).await;

    let r = reqwest::get(format!("http://{addr}/api/findings/{id}/artifacts/{small}"))
        .await
        .unwrap();
    let reason = unavailable_reason(r).await;
    assert!(reason.contains("artifact_max_bytes"), "{reason}");
    assert!(
        reason.contains("14 bytes") && reason.contains("8 bytes"),
        "{reason}"
    );
    info.assert_hits(0);
    blob.assert_hits(0);
    assert!(!stored(coord.path(), &small).exists());
    assert!(pull_leftovers(coord.path()).is_empty());
}

/// With no `[findings].artifact_max_bytes`, the limit is the default.
#[tokio::test]
async fn a_recorded_size_over_the_default_limit_is_unavailable_without_a_pull() {
    let server = httpmock::MockServer::start_async().await;
    let info = server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "9.9.9", "features": ["findings.artifact_blob"]
        }));
    });
    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &server.base_url());
    let forged = sha_of(b"forged");
    let id = write_finding(
        ws.path(),
        "find_FORGED",
        artifact(
            &forged,
            rupu_coverage::report::DEFAULT_ARTIFACT_MAX_BYTES + 1,
            ArtifactKind::Binary,
            Some(&host),
        ),
    );
    let addr = serve(st).await;
    let r = reqwest::get(format!(
        "http://{addr}/api/findings/{id}/artifacts/{forged}"
    ))
    .await
    .unwrap();
    let reason = unavailable_reason(r).await;
    assert!(reason.contains("artifact_max_bytes"), "{reason}");
    assert!(
        reason.contains(&rupu_coverage::report::DEFAULT_ARTIFACT_MAX_BYTES.to_string()),
        "{reason}"
    );
    info.assert_hits(0);
}

/// A remote whose blob route parks every request until the test releases it,
/// announcing (`started`) and counting (`hits`) each one first.
struct GatedRemote {
    hits: std::sync::atomic::AtomicU32,
    started: tokio::sync::Notify,
    /// Holds no permits until released; each request borrows one and gives
    /// it back, so one release opens the gate for good.
    release: tokio::sync::Semaphore,
}

impl GatedRemote {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            hits: std::sync::atomic::AtomicU32::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        })
    }
}

async fn serve_gated_remote(gate: std::sync::Arc<GatedRemote>, body: &'static [u8]) -> String {
    let remote = axum::Router::new()
        .route(
            "/api/host/info",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "version": "9.9.9", "features": ["findings.artifact_blob"]
                }))
            }),
        )
        .route(
            "/api/findings/artifacts/:sha",
            axum::routing::get(move || {
                let gate = std::sync::Arc::clone(&gate);
                async move {
                    gate.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    gate.started.notify_one();
                    let _open = gate.release.acquire().await.unwrap();
                    body
                }
            }),
        );
    format!("http://{}", serve_router(remote).await)
}

#[tokio::test]
async fn a_client_that_disconnects_mid_pull_does_not_strand_the_pull() {
    static BODY: &[u8] = b"slow remote poc";
    let sha = sha_of(BODY);
    let gate = GatedRemote::new();
    let remote_url = serve_gated_remote(std::sync::Arc::clone(&gate), BODY).await;

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &remote_url);
    let id = write_finding(
        ws.path(),
        "find_SLOW",
        artifact(&sha, BODY.len() as u64, ArtifactKind::Binary, Some(&host)),
    );
    let addr = serve(st).await;
    let url = format!("http://{addr}/api/findings/{id}/artifacts/{sha}");

    // The browser asks, the coordinator's pull reaches the remote, and the
    // browser goes away while the remote is still answering (axum drops the
    // handler's future when its connection closes).
    let browser = tokio::spawn({
        let url = url.clone();
        async move { reqwest::get(&url).await }
    });
    tokio::time::timeout(Duration::from_secs(5), gate.started.notified())
        .await
        .expect("the pull never reached the remote");
    browser.abort();
    assert!(browser.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(200)).await;
    gate.release.add_permits(1);

    // The pull it started still finishes, verified, into the store.
    let blob_path = stored(coord.path(), &sha);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !blob_path.is_file() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the abandoned pull never completed"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(std::fs::read(&blob_path).unwrap(), BODY);
    assert!(
        pull_leftovers(coord.path()).is_empty(),
        "{:?}",
        pull_leftovers(coord.path())
    );

    // The next view is served from the store: no second remote request.
    let r = reqwest::get(&url).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.bytes().await.unwrap().as_ref(), BODY);
    assert_eq!(gate.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// Plan A's end to end, continued (spec "End to end", Ruling R10): a remote
/// unit's coverage stream is ingested on the coordinator — which rewrites
/// its artifact to `external` on that host — and the artifact then
/// downloads through the coordinator.
#[tokio::test]
async fn a_streamed_remote_finding_downloads_end_to_end() {
    // The executing host: a real CP whose store holds the blob its unit's
    // `report_finding` copied there.
    let remote = tempfile::tempdir().unwrap();
    let body = b"#!/bin/sh\necho owned\n";
    let sha = sha_of(body);
    store_blob(remote.path(), &sha, body);
    let remote_addr = serve(state(remote.path())).await;

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &format!("http://{remote_addr}"));

    // The unit's stream, exactly as a remote `rupu run` writes it: the
    // artifact is `copied` into THAT host's store, with no host recorded.
    let copied = ArtifactRef {
        path: "poc/run.sh".into(),
        sha256: sha.clone(),
        size: body.len() as u64,
        kind: Some(ArtifactKind::Text),
        stored: Some(ArtifactStorage::Copied),
        host: None,
    };
    let lines = [
        rupu_coverage::StreamLine::Begin {
            v: rupu_coverage::STREAM_VERSION,
            run_id: "run_UNIT".into(),
        },
        rupu_coverage::StreamLine::Findings {
            scope_name: "repo".into(),
            record: finding_with("find_E2E", copied),
        },
    ];
    let stream: String = lines
        .iter()
        .map(|l| serde_json::to_string(l).unwrap() + "\n")
        .collect();
    let ingested = rupu_coverage::ingest_unit_stream(
        ws.path(),
        &rupu_coverage::IngestSource {
            host: Some(host.clone()),
        },
        stream.as_bytes(),
    )
    .unwrap();
    assert!(ingested.begin_seen);
    assert_eq!(ingested.appended, 1);

    // On the coordinator the artifact is now `external` on that host.
    let tid = rupu_coverage::target_id(ws.path(), "repo");
    let recs = rupu_coverage::read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap();
    let a = &recs[0].report.as_ref().unwrap().artifacts[0];
    assert_eq!(a.stored, Some(ArtifactStorage::External));
    assert_eq!(a.host.as_deref(), Some(host.as_str()));
    assert!(!stored(coord.path(), &sha).exists());

    let addr = serve(st).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/findings/find_E2E/artifacts/{sha}"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(resp.headers()["content-security-policy"], "sandbox");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), body);
    assert_eq!(std::fs::read(stored(coord.path(), &sha)).unwrap(), body);
}

/// An `image` evidence block's file travels the same road as a listed
/// artifact: ingest marks it external on the executing host, the coordinator
/// pulls and verifies it on first view, and serves it inline as the image it
/// is (by magic bytes).
#[tokio::test]
async fn a_remote_image_block_downloads_end_to_end() {
    let remote = tempfile::tempdir().unwrap();
    let mut body = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    body.extend_from_slice(b"pretend pixels");
    let sha = sha_of(&body);
    store_blob(remote.path(), &sha, &body);
    let remote_addr = serve(state(remote.path())).await;

    let coord = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (st, host) = coordinator(coord.path(), ws.path(), &format!("http://{remote_addr}"));

    let copied = ArtifactRef {
        path: "shots/crash.png".into(),
        sha256: sha.clone(),
        size: body.len() as u64,
        kind: Some(ArtifactKind::Binary),
        stored: Some(ArtifactStorage::Copied),
        host: None,
    };
    let mut rec = finding_with("find_IMG", copied.clone());
    {
        let report = rec.report.as_mut().unwrap();
        report.artifacts = vec![];
        report
            .blocks
            .push(rupu_coverage::report::EvidenceBlock::Image {
                artifact: copied,
                caption: Some("the crash".into()),
            });
    }
    let lines = [
        rupu_coverage::StreamLine::Begin {
            v: rupu_coverage::STREAM_VERSION,
            run_id: "run_UNIT".into(),
        },
        rupu_coverage::StreamLine::Findings {
            scope_name: "repo".into(),
            record: rec,
        },
    ];
    let stream: String = lines
        .iter()
        .map(|l| serde_json::to_string(l).unwrap() + "\n")
        .collect();
    let ingested = rupu_coverage::ingest_unit_stream(
        ws.path(),
        &rupu_coverage::IngestSource {
            host: Some(host.clone()),
        },
        stream.as_bytes(),
    )
    .unwrap();
    assert_eq!(ingested.appended, 1);
    assert!(!stored(coord.path(), &sha).exists());

    let addr = serve(st).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/findings/find_IMG/artifacts/{sha}"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "image/png");
    assert_eq!(resp.headers()["content-security-policy"], "sandbox");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), body.as_slice());
    assert_eq!(std::fs::read(stored(coord.path(), &sha)).unwrap(), body);
}
