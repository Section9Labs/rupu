//! Proves rupu-update's own release-check egress reaches the netflow sink
//! under `Origin::Update` — not just the netflow factory in isolation.

use rupu_netflow::{MemorySink, Origin};
use rupu_update::model::ReleaseSource;
use rupu_update::GithubReleaseSource;
use std::sync::Arc;

/// Drives `GithubReleaseSource::list_releases` (real rupu-update code, not
/// a hand-built netflow client) against a mock server and asserts the
/// resulting flow lands in the sink with `Origin::Update` and no run
/// attribution — the updater has no run context.
#[tokio::test]
async fn release_check_is_recorded_under_update_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/Section9Labs/rupu/releases");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    rupu_netflow::http::init(sink.clone());

    let source = GithubReleaseSource::with_api_base("Section9Labs/rupu", server.url(""));

    source
        .list_releases()
        .await
        .expect("mock server returns an empty release list");

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].ctx.origin, Origin::Update);
    assert_eq!(records[0].ctx.run_id, None, "the updater has no run");
}
