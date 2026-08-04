//! OAuth egress must be visible. These are the calls that carry
//! credentials, so their destinations are exactly what an operator
//! wants accounted for.

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn oauth_client_records_flows_under_system_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/oauth/token");
            then.status(200).body("{}");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    let client = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::System),
        reqwest::Client::builder(),
        sink.clone(),
    )
    .unwrap();

    client
        .post(server.url("/oauth/token"))
        .send()
        .await
        .unwrap();

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path, "/oauth/token");
    assert_eq!(records[0].ctx.origin, Origin::System);
}
