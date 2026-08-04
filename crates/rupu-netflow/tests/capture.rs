#![cfg(feature = "http")]

use rupu_netflow::{FlowCtx, FlowRecord, FlowSink, MemorySink, Origin};
use std::sync::Arc;

/// A sink that panics on every call. Drives invariant 1 directly through
/// the middleware — NOT wrapped in a `FanoutSink`, since that already
/// isolates panics per-child on its own and is not on the production
/// path (nothing in the repo installs it yet; see `sink.rs`'s doc
/// comments). This proves the middleware itself is panic-safe
/// unconditionally, for whatever sink is actually plugged in.
struct PanicSink;

#[async_trait::async_trait]
impl FlowSink for PanicSink {
    async fn record(&self, _flow: FlowRecord) {
        panic!("intentional panic in PanicSink::record");
    }

    async fn complete(&self, _id: rupu_netflow::FlowId, _bytes_in: u64, _duration_ms: u64) {
        panic!("intentional panic in PanicSink::complete");
    }
}

#[tokio::test]
async fn a_panicking_sink_never_breaks_the_request() {
    // Invariant 1, non-negotiable per spec §10: "A panic in the middleware
    // must not propagate into the request path." Before the middleware
    // isolated its `sink.record(...).await` call in a spawned task, a
    // panicking sink would panic the whole request future — this test
    // would have failed (or aborted the test process) against that code.
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).body("hello");
        })
        .await;

    // Suppress panic-hook output so a passing test run stays clean; the
    // panic itself is expected and contained by `tokio::task::spawn`.
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::System),
        reqwest::Client::builder(),
        Arc::new(PanicSink),
    )
    .unwrap();

    let result = client.get(server.url("/v1/models")).send().await;

    std::panic::set_hook(old_hook);

    let resp = result.expect("a panicking sink must never fail the HTTP request");
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn records_a_successful_request() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).body("hello");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx {
            run_id: Some("run-7".into()),
            step_id: Some("step-2".into()),
            agent: Some("reviewer".into()),
            workspace_id: Some("ws".into()),
            origin: Origin::Provider("anthropic".into()),
        },
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    let resp = client.get(server.url("/v1/models")).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let records = sink.records();
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(r.method, "GET");
    assert_eq!(r.path, "/v1/models");
    assert_eq!(r.status, Some(200));
    assert_eq!(r.ctx.run_id.as_deref(), Some("run-7"));
    assert!(matches!(r.outcome, rupu_netflow::Outcome::Ok));
    assert!(r.peer_ip.is_some(), "remote_addr must be captured");
    assert!(r.ttfb_ms.is_some());
}

#[tokio::test]
async fn a_bodyless_get_records_a_known_zero_bytes_out_not_unknown() {
    // Fix 3 (2026-08-03 review): a GET has no body — that is a genuinely
    // known zero, not an unobserved count. Composed with `host_rollup`'s
    // (correct) collapse-on-unknown rule, treating "no body" as unknown
    // would permanently blank `bytes_out` for any host that ever served a
    // GET, which is exactly what the update checker and every
    // `/v1/models` probe do.
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).body("hello");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Update),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    client.get(server.url("/v1/models")).send().await.unwrap();

    let r = &sink.records()[0];
    assert_eq!(
        r.bytes_out,
        Some(0),
        "a bodyless GET must record a KNOWN zero, not None"
    );
}

#[tokio::test]
async fn never_stores_the_query_string() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/search");
            then.status(200);
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Update),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    let url = format!("{}?api_key=SUPERSECRET&q=x", server.url("/search"));
    client.get(url).send().await.unwrap();

    let r = &sink.records()[0];
    assert_eq!(r.path, "/search");
    assert!(!r.path.contains('?'));
    let json = serde_json::to_string(r).unwrap();
    assert!(
        !json.contains("SUPERSECRET"),
        "no part of the record may carry query values"
    );
}

#[tokio::test]
async fn records_an_http_error_status_as_http_error() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/boom");
            then.status(503);
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Cp),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    client.get(server.url("/boom")).send().await.unwrap();

    let r = &sink.records()[0];
    assert_eq!(r.status, Some(503));
    assert!(matches!(r.outcome, rupu_netflow::Outcome::HttpError));
}

#[tokio::test]
async fn records_a_transport_failure_with_no_status() {
    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::System),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    // Port 1 on loopback refuses connections.
    let result = client.get("http://127.0.0.1:1/nope").send().await;
    assert!(result.is_err());

    let r = &sink.records()[0];
    assert_eq!(r.status, None);
    assert!(matches!(
        r.outcome,
        rupu_netflow::Outcome::TransportError | rupu_netflow::Outcome::Timeout
    ));
    assert!(r.error.is_some());
}

#[tokio::test]
async fn a_caller_supplied_flow_id_is_used_so_it_can_complete_later() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200).body("stream");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Provider("anthropic".into())),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    let id = rupu_netflow::FlowId::from_parts(42, 42);
    client
        .post(server.url("/v1/messages"))
        .with_extension(id)
        .send()
        .await
        .unwrap();

    let records = sink.records();
    assert_eq!(records[0].id, id, "middleware must honour the caller's id");
    assert!(
        !records[0].body_complete,
        "a streamed body is not complete at header time"
    );
}

#[tokio::test]
async fn a_streamed_body_is_finalized_through_the_ledger() {
    use rupu_netflow::{NetflowPaths, NetflowWriterHandle};

    let server = httpmock::MockServer::start_async().await;
    // Chunked: no fixed length known ahead of the body draining. This is
    // the SSE shape every provider chat path has. Note: httpmock still
    // sets a Content-Length on the response despite the chunked header —
    // that's fine, since `body_complete` is driven by whether the caller
    // supplied a FlowId (declaring intent to finalize later), not by the
    // presence/absence of Content-Length. See middleware.rs.
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("transfer-encoding", "chunked")
                .body("data: one\n\ndata: two\n\n");
        })
        .await;

    let tmp = tempfile::TempDir::new().unwrap();
    let paths = NetflowPaths::new(tmp.path());
    let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();

    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Provider("anthropic".into())),
        reqwest::Client::builder(),
        handle.writer.clone(),
    )
    .unwrap();

    let id = rupu_netflow::FlowId::new();
    let mut resp = client
        .post(server.url("/v1/messages"))
        .with_extension(id)
        .send()
        .await
        .unwrap();

    // Drain exactly the way the provider's SSE loop does.
    let started = std::time::Instant::now();
    let mut total = 0u64;
    while let Some(chunk) = resp.chunk().await.unwrap() {
        total += chunk.len() as u64;
    }
    handle
        .writer
        .complete(id, total, started.elapsed().as_millis() as u64)
        .await;
    tokio::time::timeout(std::time::Duration::from_secs(10), handle.shutdown())
        .await
        .expect("writer shutdown deadlocked");

    let flows = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].id, id);
    assert_eq!(flows[0].bytes_in, Some(total));
    assert!(total > 0);
    assert!(
        flows[0].body_complete,
        "the later Complete line must fold in and flip body_complete, \
         even though httpmock's Content-Length made the header-time \
         record look complete-ish — a caller-supplied FlowId means the \
         middleware deliberately withholds body_complete until this \
         explicit finalization"
    );
}
