//! Fleet-host egress must be visible. `HttpHostConnector` is the client
//! `rupu cp serve` uses to reach every remote host the operator configured
//! — genuinely interesting egress, not just internal plumbing.
//!
//! Drives the real `HttpHostConnector::info()` production code path (not a
//! hand-built netflow client) against a mock server and asserts the flow
//! lands in the sink under `Origin::Cp`.

use rupu_cp::host::connector::HostConnector as _;
use rupu_cp::host::http::HttpHostConnector;
use rupu_netflow::{MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn host_connector_info_call_is_recorded_under_cp_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/api/host/info");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"version":"9.9.9"}"#);
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    rupu_netflow::http::init(sink.clone());

    let conn = HttpHostConnector::new(server.base_url(), None);
    let info = conn.info().await.expect("mock server answers 200");
    assert!(info.reachable);
    assert_eq!(info.version.as_deref(), Some("9.9.9"));

    let records = sink.records();
    let r = records
        .iter()
        .find(|r| r.path == "/api/host/info")
        .expect("HttpHostConnector::info's request must be recorded in the netflow sink");
    assert_eq!(r.ctx.origin, Origin::Cp);
}
