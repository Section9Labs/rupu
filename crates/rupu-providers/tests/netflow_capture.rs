//! Proves provider egress reaches the netflow sink with provider
//! attribution — the whole point of the migration.

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

/// Exercises `rupu_netflow::http::client_with` directly with a hand-built
/// `FlowCtx` — this is the same shape as `rupu-netflow`'s own factory tests
/// and proves the netflow client factory itself attributes correctly. It
/// does NOT touch any migrated provider constructor (`AnthropicClient::new`,
/// `build_http_client_with_timeout`, `with_tuning`, etc.), so on its own it
/// would not catch a provider reverting to a bare `reqwest::Client`. See
/// `anthropic_client_reaches_the_netflow_sink` below for that half.
#[tokio::test]
async fn provider_client_records_flows_with_provider_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).body("{}");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx {
            run_id: Some("run-1".into()),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        rupu_providers::tuning::ProviderTuning::default().http_client_builder(),
        sink.clone(),
    )
    .unwrap();

    client.get(server.url("/v1/models")).send().await.unwrap();

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].ctx.origin,
        Origin::Provider("anthropic".to_string())
    );
    assert_eq!(records[0].ctx.run_id.as_deref(), Some("run-1"));
}

/// Drives an actual migrated provider constructor (`AnthropicClient::with_url`
/// → `build_http_client` → `client_with`) against a mock server and asserts
/// the resulting flow lands in the netflow sink with `Origin::Provider`. This
/// is the test that would catch a provider reverting to a bare
/// `reqwest::Client::new()` — the test above cannot, since it never calls
/// into `rupu-providers`' own client-construction code.
///
/// There is no process-global sink anymore (Task 4 removed `http::init`):
/// the sink is passed straight into `with_url` below.
#[tokio::test]
async fn anthropic_client_reaches_the_netflow_sink() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":[]}"#);
        })
        .await;

    let sink = Arc::new(MemorySink::default());

    let client = rupu_providers::AnthropicClient::with_url(
        "test-key".into(),
        format!("{}/v1/messages", server.url("")),
        sink.clone(),
    );

    <rupu_providers::AnthropicClient as rupu_providers::LlmProvider>::probe(&client)
        .await
        .expect("probe should succeed against the mocked /v1/models endpoint");

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path, "/v1/models");
    assert_eq!(
        records[0].ctx.origin,
        Origin::Provider("anthropic".to_string())
    );
}
