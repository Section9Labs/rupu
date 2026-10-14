//! A run's end copies the flows its transcript holds and its netflow ledger
//! lacks into the ledger (`rupu_runtime::netflow::RunNetflow::finish`).

use std::sync::Arc;

use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, NetflowPaths, Origin, Outcome};
use rupu_runtime::assembly::Origin as RunOrigin;
use serial_test::serial;

use crate::assembly::{agent, launch};
use crate::support::{config, Home, MockScript, DONE};

fn flow(id: FlowId, host: &str) -> FlowRecord {
    FlowRecord {
        id,
        ts: chrono::Utc::now(),
        ctx: FlowCtx::system(Origin::Scm("github".into())),
        fidelity: Fidelity::Http,
        method: "GET".into(),
        scheme: "https".into(),
        host: host.into(),
        port: 443,
        path: "/repos".into(),
        peer_ip: None,
        resolved_ips: vec![],
        http_version: None,
        status: Some(200),
        outcome: Outcome::Ok,
        error: None,
        bytes_out: None,
        bytes_in: Some(64),
        body_complete: true,
        process: None,
        local_addr: None,
        direction: None,
        ttfb_ms: None,
        duration_ms: None,
    }
}

fn append_to_transcript(path: &std::path::Path, flow: FlowRecord) {
    let mut w = rupu_transcript::JsonlWriter::append(path).unwrap();
    w.write(&rupu_transcript::Event::NetFlow {
        flow: Box::new(flow),
    })
    .unwrap();
    w.flush().unwrap();
}

fn ledger_hosts(ledger: &std::path::Path) -> Vec<(String, String)> {
    rupu_netflow::ledger::read_flows(ledger)
        .unwrap()
        .into_iter()
        .map(|f| (f.id.to_string(), f.host))
        .collect()
}

#[tokio::test]
#[serial]
async fn a_run_end_copies_a_transcript_only_flow_into_the_ledger_once() {
    let home = Home::new();
    let assembler = home.assembler(config());
    let spec = launch(
        &home,
        agent(&home, ""),
        RunOrigin::Standalone,
        "run_backfill",
    );
    let transcript = spec.transcript_path.clone();
    let id = FlowId::from_parts(1, 7);
    // A flow that reaches the transcript and never the ledger, mid-run:
    // what a ledger writer that dropped the line leaves behind.
    let mut spec = spec;
    let hook_transcript = transcript.clone();
    spec.hooks.on_usage = Some(Arc::new(move |_| {
        append_to_transcript(&hook_transcript, flow(id, "lost.example"));
    }));
    let _m = MockScript::set(DONE);
    rupu_runtime::run_agent(&assembler, spec)
        .await
        .unwrap()
        .result
        .expect("the run completes");

    let paths = NetflowPaths::for_run(&home.global().join("netflow"), "run_backfill");
    assert_eq!(
        ledger_hosts(&paths.flows),
        vec![(
            "00000000010000000000000007".to_string(),
            "lost.example".to_string()
        )]
    );

    // A second finish over the same transcript (a later run under the same
    // id) appends nothing.
    assert_eq!(
        rupu_runtime::netflow::backfill_ledger(&paths, &transcript).unwrap(),
        0
    );
    assert_eq!(ledger_hosts(&paths.flows).len(), 1);
}

#[test]
fn a_backfill_skips_flows_the_ledger_has_and_mends_a_torn_tail() {
    let dir = tempfile::tempdir().unwrap();
    let paths = NetflowPaths::for_run(&dir.path().join("netflow"), "run_x");
    paths.ensure_dir().unwrap();
    let kept = FlowId::from_parts(1, 1);
    let lost = FlowId::from_parts(1, 2);
    let line = serde_json::to_string(&rupu_netflow::LedgerLine::Flow(Box::new(flow(
        kept,
        "kept.example",
    ))))
    .unwrap();
    std::fs::write(&paths.flows, format!("{line}\n{{\"type\":\"flow\",\"id\"")).unwrap();
    let transcript = dir.path().join("run_x.jsonl");
    append_to_transcript(&transcript, flow(kept, "kept-transcript-copy.example"));
    append_to_transcript(&transcript, flow(lost, "lost.example"));

    assert_eq!(
        rupu_runtime::netflow::backfill_ledger(&paths, &transcript).unwrap(),
        1
    );
    assert_eq!(
        ledger_hosts(&paths.flows),
        vec![
            (
                "00000000010000000000000001".to_string(),
                "kept.example".to_string()
            ),
            (
                "00000000010000000000000002".to_string(),
                "lost.example".to_string()
            ),
        ]
    );
}
