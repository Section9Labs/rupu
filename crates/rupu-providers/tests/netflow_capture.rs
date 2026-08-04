//! Proves provider egress reaches the netflow sink with provider
//! attribution — the whole point of the migration.

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

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
