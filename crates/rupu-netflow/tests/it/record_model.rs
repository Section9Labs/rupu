//! Pure record-model tests: wire shapes, ledger folds and port traits that
//! need no `http` feature. Deliberately UNGATED so a bare
//! `cargo test -p rupu-netflow` exercises them.

use chrono::{TimeZone, Utc};
use rupu_netflow::ledger::explorer::{origin_key, timeline_view, ExplorerFilters, ExplorerFlow};
use rupu_netflow::ledger::views::{read_flows, read_flows_and_dropped};
use rupu_netflow::{
    CaptureState, Direction, Fidelity, FlowCtx, FlowId, FlowProcess, FlowRecord, LedgerLine,
    Origin, Outcome, SocketCompletion,
};
use std::io::Write;

fn socket_flow(id: FlowId) -> FlowRecord {
    FlowRecord {
        id,
        ts: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        ctx: FlowCtx::system(Origin::Subprocess("curl".into())),
        fidelity: Fidelity::Socket,
        method: "TCP".into(),
        scheme: "tcp".into(),
        host: "93.184.216.34".into(),
        port: 443,
        path: String::new(),
        peer_ip: None,
        resolved_ips: vec![],
        http_version: None,
        status: None,
        outcome: Outcome::Ok,
        error: None,
        bytes_out: None,
        bytes_in: None,
        body_complete: false,
        process: Some(FlowProcess {
            pid: 4412,
            name: "curl".into(),
        }),
        local_addr: Some("10.0.0.2:51234".into()),
        direction: Some(Direction::Outbound),
        ttfb_ms: None,
        duration_ms: None,
    }
}

fn write_ledger(lines: &[LedgerLine]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for l in lines {
        serde_json::to_writer(&mut f, l).unwrap();
        f.write_all(b"\n").unwrap();
    }
    f.flush().unwrap();
    f
}

// ---- Tasks 1-4: pure-model tests (moved from capture.rs) ----

#[test]
fn socket_fidelity_serializes_snake_case_and_round_trips() {
    let json = serde_json::to_string(&Fidelity::Socket).unwrap();
    assert_eq!(json, r#""socket""#);
    assert_eq!(
        serde_json::from_str::<Fidelity>(&json).unwrap(),
        Fidelity::Socket
    );
}

#[test]
fn subprocess_origin_tags_and_keys() {
    let o = Origin::Subprocess("curl".into());
    let json = serde_json::to_value(&o).unwrap();
    assert_eq!(json, serde_json::json!({"kind":"subprocess","name":"curl"}));
    assert_eq!(serde_json::from_value::<Origin>(json).unwrap(), o);
    assert_eq!(origin_key(&o), "subprocess:curl");
}

#[test]
fn flow_ctx_tool_call_id_is_optional_and_omitted_when_none() {
    let mut c = FlowCtx::system(Origin::Subprocess("curl".into()));
    assert!(serde_json::to_string(&c)
        .unwrap()
        .find("tool_call_id")
        .is_none());
    c.tool_call_id = Some("toolu_01Ab".into());
    let json = serde_json::to_string(&c).unwrap();
    assert!(json.contains(r#""tool_call_id":"toolu_01Ab""#));
    assert_eq!(serde_json::from_str::<FlowCtx>(&json).unwrap(), c);
}

#[test]
fn flow_process_and_direction_round_trip() {
    let p = FlowProcess {
        pid: 4412,
        name: "curl".into(),
    };
    assert_eq!(
        serde_json::from_str::<FlowProcess>(&serde_json::to_string(&p).unwrap()).unwrap(),
        p
    );
    let d = Direction::Outbound;
    assert_eq!(serde_json::to_string(&d).unwrap(), r#""outbound""#);
    assert_eq!(
        serde_json::from_str::<Direction>(r#""inbound""#).unwrap(),
        Direction::Inbound
    );
}

// ---- Review findings from Tasks 1-4 ----

/// The explorer tags a lane with its LEAST-observable contributor. Pin the
/// ordering `Coarse < Socket < Http < Full` through every pairing, in both
/// arrival orders, so a new `Fidelity` variant can't silently mis-rank.
#[test]
fn explorer_ranks_fidelity_coarse_socket_http_full() {
    let order = [
        Fidelity::Coarse,
        Fidelity::Socket,
        Fidelity::Http,
        Fidelity::Full,
    ];
    let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let explorer_flow = |n: u64, fidelity: Fidelity| {
        let mut flow = socket_flow(FlowId::from_parts(n, n as u128));
        flow.fidelity = fidelity;
        flow.ts = at;
        ExplorerFlow {
            run_id: None,
            workflow: None,
            asn: None,
            flow,
        }
    };
    for (i, lo) in order.iter().enumerate() {
        for hi in &order[i + 1..] {
            for flows in [
                vec![explorer_flow(1, *lo), explorer_flow(2, *hi)],
                vec![explorer_flow(1, *hi), explorer_flow(2, *lo)],
            ] {
                let t = timeline_view(
                    &flows,
                    at - chrono::Duration::seconds(1),
                    at + chrono::Duration::seconds(1),
                    &ExplorerFilters::default(),
                    &[],
                    4,
                );
                assert_eq!(t.lanes.len(), 1, "same endpoint is one lane");
                assert_eq!(t.lanes[0].fidelity, *lo, "{lo:?} must rank below {hi:?}");
            }
        }
    }
}

#[test]
fn socket_fields_are_absent_when_none_and_legacy_json_parses() {
    let mut f = socket_flow(FlowId::from_parts(1, 1));
    f.process = None;
    f.local_addr = None;
    f.direction = None;
    f.fidelity = Fidelity::Http;
    let v = serde_json::to_value(&f).unwrap();
    for k in ["process", "local_addr", "direction"] {
        assert!(v.get(k).is_none(), "{k} must be omitted when None");
    }
    // A record written before these fields existed still parses.
    let back: FlowRecord = serde_json::from_value(v).unwrap();
    assert_eq!(back, f);
    // And with them present they serialize.
    let v = serde_json::to_value(socket_flow(FlowId::from_parts(1, 1))).unwrap();
    assert_eq!(v["process"]["pid"], 4412);
    assert_eq!(v["direction"], "outbound");
    assert_eq!(v["local_addr"], "10.0.0.2:51234");
}

// ---- Task 5: SocketComplete ----

#[test]
fn socket_complete_round_trips_and_folds_into_its_flow() {
    let id = FlowId::from_parts(5, 5);
    let c = SocketCompletion {
        id,
        duration_ms: 553,
        bytes_in: Some(48000),
        bytes_out: Some(1200),
        outcome: Some(Outcome::Ok),
        error: None,
    };
    let line = LedgerLine::SocketComplete(c.clone());
    let json = serde_json::to_string(&line).unwrap();
    assert!(json.contains(r#""type":"socket_complete""#));
    assert_eq!(serde_json::from_str::<LedgerLine>(&json).unwrap(), line);

    let ledger = write_ledger(&[LedgerLine::Flow(Box::new(socket_flow(id))), line]);
    let flows = read_flows(ledger.path()).unwrap();
    assert_eq!(flows.len(), 1);
    assert!(flows[0].body_complete);
    assert_eq!(flows[0].bytes_in, Some(48000));
    assert_eq!(flows[0].bytes_out, Some(1200));
    assert_eq!(flows[0].duration_ms, Some(553));
    assert_eq!(flows[0].outcome, Outcome::Ok);
}

#[test]
fn socket_complete_none_fields_leave_the_flow_and_unknown_ids_are_ignored() {
    let id = FlowId::from_parts(6, 6);
    let mut flow = socket_flow(id);
    flow.bytes_out = Some(7);
    let ledger = write_ledger(&[
        LedgerLine::Flow(Box::new(flow)),
        LedgerLine::SocketComplete(SocketCompletion {
            id,
            duration_ms: 10,
            bytes_in: None,
            bytes_out: None,
            outcome: Some(Outcome::TransportError),
            error: Some("reset by peer".into()),
        }),
        LedgerLine::SocketComplete(SocketCompletion {
            id: FlowId::from_parts(99, 99),
            duration_ms: 1,
            bytes_in: Some(1),
            bytes_out: None,
            outcome: None,
            error: None,
        }),
    ]);
    let flows = read_flows(ledger.path()).unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(
        flows[0].bytes_out,
        Some(7),
        "None leaves the field as written"
    );
    assert_eq!(flows[0].bytes_in, None);
    assert_eq!(flows[0].outcome, Outcome::TransportError);
    assert_eq!(flows[0].error.as_deref(), Some("reset by peer"));
}

#[tokio::test]
async fn memory_and_fanout_sinks_carry_socket_completions() {
    use rupu_netflow::{FanoutSink, FlowSink, MemorySink, NullSink};
    use std::sync::Arc;
    let a = Arc::new(MemorySink::default());
    let b = Arc::new(MemorySink::default());
    let fan = FanoutSink::new(vec![a.clone(), Arc::new(NullSink), b.clone()]);
    let c = SocketCompletion {
        id: FlowId::from_parts(8, 8),
        duration_ms: 3,
        bytes_in: Some(1),
        bytes_out: None,
        outcome: None,
        error: None,
    };
    fan.complete_socket(c.clone()).await;
    assert_eq!(a.socket_completions(), vec![c.clone()]);
    assert_eq!(b.socket_completions(), vec![c]);
    // The HTTP completion path is separate and untouched.
    assert!(a.completions().is_empty());
}

// ---- Task 6: Capture ----

#[test]
fn capture_line_round_trips_and_is_ignored_by_read_flows() {
    let line = LedgerLine::Capture {
        ts: Utc::now(),
        state: CaptureState::Unavailable {
            reason: "cgroup v2 not mounted".into(),
        },
        tool_call_id: Some("toolu_01Ab".into()),
        note: None,
    };
    let json = serde_json::to_string(&line).unwrap();
    assert!(json.contains(r#""type":"capture""#));
    assert_eq!(serde_json::from_str::<LedgerLine>(&json).unwrap(), line);

    let active = LedgerLine::Capture {
        ts: Utc::now(),
        state: CaptureState::Active {
            backend: "cgroup-ebpf".into(),
        },
        tool_call_id: None,
        note: Some("note".into()),
    };
    let json = serde_json::to_string(&active).unwrap();
    assert!(json.contains(r#""state":"active""#));
    assert!(!json.contains("tool_call_id"));
    assert_eq!(serde_json::from_str::<LedgerLine>(&json).unwrap(), active);

    // The fold: a Capture line neither adds a flow nor touches the count.
    let ledger = write_ledger(&[
        LedgerLine::Flow(Box::new(socket_flow(FlowId::from_parts(1, 1)))),
        line,
        LedgerLine::Dropped {
            count: 2,
            ts: Utc::now(),
        },
    ]);
    let (flows, dropped) = read_flows_and_dropped(ledger.path()).unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(dropped, 2);
}
