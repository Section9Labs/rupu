//! Fix 1 (2026-08-03 whole-branch review of the netflow crate-and-capture
//! plan): a streamed response's two-phase completion pair — the `Flow`
//! line written at header time and the `Complete` line written once the
//! body finishes draining — must land in the SAME ledger.
//!
//! Before this fix, `http::complete()` re-resolved the process-global
//! `SINK` at completion time while the middleware wrote its `Flow` line to
//! `self.sink`, the instance the client was actually built with. Those
//! happened to be the same object today (both are the one process-wide
//! global), so nothing broke — but `FlowCompletionGuard` re-resolving the
//! global rather than carrying the sink it was given is exactly the shape
//! that dangles a `Flow` in one ledger and silently discards its
//! `Complete` in another the moment a per-workspace sink (Plan 2) is
//! introduced.
//!
//! The prior test coverage for the two-phase pair
//! (`rupu-netflow/tests/capture.rs::a_streamed_body_is_finalized_through_the_ledger`)
//! called `handle.writer.complete(...)` directly — bypassing
//! `FlowCompletionGuard` and `AnthropicClient` entirely — which is exactly
//! why this mismatch was invisible. This test instead drives the real
//! production path (`AnthropicClient::stream()`) and reads the ledger
//! back.
//!
//! `rupu_netflow::http::init` is process-wide and first-call-wins; this is
//! the only test in this binary, so there is no cross-test race.

use rupu_netflow::{FlowSink, NetflowPaths, NetflowWriterHandle};
use rupu_providers::types::{LlmRequest, Message};
use std::sync::Arc;

fn make_request() -> LlmRequest {
    LlmRequest {
        model: "claude-sonnet-4-6".into(),
        system: None,
        messages: vec![Message::user("hi")],
        max_tokens: 16,
        tools: vec![],
        cell_id: None,
        trace_id: None,
        thinking: None,
        context_window: None,
        task_type: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
    }
}

#[tokio::test]
async fn streamed_anthropic_response_flow_and_complete_land_in_the_same_ledger() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",",
                    "\"model\":\"claude-sonnet-4-6\",\"usage\":{\"input_tokens\":1}}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                ));
        })
        .await;

    let tmp = tempfile::TempDir::new().unwrap();
    let paths = NetflowPaths::new(tmp.path());
    let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();
    let sink: Arc<dyn FlowSink> = handle.writer.clone();
    rupu_netflow::http::init(sink);

    let mut client = rupu_providers::AnthropicClient::with_url(
        "test-key".into(),
        format!("{}/v1/messages", server.url("")),
    );

    let result = client.stream(&make_request(), |_ev| {}).await;
    assert!(result.is_ok(), "stream should succeed: {:?}", result.err());

    tokio::time::timeout(std::time::Duration::from_secs(10), handle.shutdown())
        .await
        .expect("writer shutdown deadlocked");

    let flows = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    assert_eq!(flows.len(), 1, "exactly one flow should have been recorded");
    assert!(
        flows[0].body_complete,
        "the streamed flow must be finalized by its Complete line, proving \
         the Flow written by the middleware and the Complete written by \
         FlowCompletionGuard landed in the SAME ledger"
    );
    assert!(
        flows[0].bytes_in.is_some(),
        "a finalized flow must have a known byte count"
    );
}
