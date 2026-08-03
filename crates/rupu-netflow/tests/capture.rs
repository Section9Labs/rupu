#![cfg(feature = "http")]

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

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
