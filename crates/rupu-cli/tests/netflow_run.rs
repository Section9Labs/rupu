//! A run's egress must reach BOTH destinations: the ledger (which
//! survives across runs and holds unattributed system egress) and the
//! run transcript (which streams live).

use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;

#[tokio::test]
async fn a_flow_reaches_both_the_ledger_and_the_transcript() {
    use rupu_netflow::{FanoutSink, NetflowPaths, NetflowWriterHandle};
    use rupu_transcript::{JsonlWriter, TranscriptSink};

    let tmp = tempfile::TempDir::new().unwrap();
    let transcript = tmp.path().join("transcript.jsonl");
    JsonlWriter::create(&transcript).unwrap();

    let paths = NetflowPaths::new(tmp.path());
    let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();

    let observed = Arc::new(MemorySink::default());
    let fanout = FanoutSink::new(vec![
        handle.writer.clone(),
        Arc::new(TranscriptSink::new(transcript.clone())),
        observed.clone(),
    ]);

    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/ping");
            then.status(200).body("pong");
        })
        .await;

    let client = rupu_netflow::http::client_with(
        FlowCtx {
            run_id: Some("run-x".into()),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        reqwest::Client::builder(),
        Arc::new(fanout),
    )
    .unwrap();

    client.get(server.url("/ping")).send().await.unwrap();
    handle.shutdown().await;

    // Ledger
    let flows = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].ctx.run_id.as_deref(), Some("run-x"));

    // Transcript
    let text = std::fs::read_to_string(&transcript).unwrap();
    assert!(text.contains(r#""type":"net_flow""#));

    // And the observer saw it too — fanout reaches every child.
    assert_eq!(observed.records().len(), 1);
}
