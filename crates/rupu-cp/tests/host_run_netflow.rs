//! Coverage for the netflow `Host`-proxy path — `GET /api/runs/:id/netflow`
//! when `resolve_run_location` places the run on a *registered remote
//! host* (`RunLocation::Host`), via `run_netflow_from_host` /
//! `enforce_range_on_proxied_response` in `rupu-cp/src/api/netflow.rs`.
//!
//! This is the first test anywhere in `rupu-cp` to drive a route's `Host`
//! branch end to end (real HTTP round trip through a real
//! `HttpHostConnector` to a real `httpmock::MockServer`, through the real
//! axum router) — see `tests/common/mod.rs`'s module doc for the harness
//! this builds on and how the gap was confirmed.
//!
//! `enforce_range_on_proxied_response` already had a pure unit test
//! (`netflow.rs`'s own `enforce_range_on_proxied_response_recomputes_
//! hosts_and_window`) covering the correction logic in isolation. What was
//! missing — and what a hand-built `HostConnector` mock could never catch
//! — is proof that the SAME correction survives a REAL wire round trip:
//! real `serde_json` serialization on the mock "remote" side, a real
//! `reqwest` GET, real deserialization back into `NetflowResponse` on this
//! side, through the real `resolve_run_location` → `Host` → `resolve_host`
//! → `HttpHostConnector::proxy_get_json` path.

// Throwaway in-process mock-server client, not rupu's own egress — mirrors
// every other httpmock-based test file in this crate.
#![allow(clippy::disallowed_methods)]

mod common;

use reqwest::StatusCode;
use rupu_cp::api::netflow::{FlowView, NetflowResponse, WindowEcho};
use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, Origin, Outcome};

fn flow_at(ts_secs: i64, host: &str) -> FlowRecord {
    FlowRecord {
        id: FlowId::new(),
        ts: chrono::DateTime::from_timestamp(ts_secs, 0).unwrap(),
        ctx: FlowCtx {
            run_id: None,
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        fidelity: Fidelity::Http,
        method: "POST".into(),
        scheme: "https".into(),
        host: host.into(),
        port: 443,
        path: "/v1/messages".into(),
        peer_ip: None,
        resolved_ips: vec![],
        http_version: None,
        status: Some(200),
        outcome: Outcome::Ok,
        error: None,
        bytes_out: Some(10),
        bytes_in: Some(20),
        body_complete: true,
        ttfb_ms: None,
        duration_ms: Some(30),
    }
}

/// The remote's own (unfiltered — as either an older build that ignores
/// `from`/`to`, or a same-build remote that was queried with no window at
/// all, would produce) response: both flows present, `hosts` rolled up from
/// BOTH, `window` unbounded, `dropped_total` from the whole ledger file.
fn unfiltered_remote_response(flows: Vec<FlowRecord>, dropped_total: u64) -> NetflowResponse {
    let hosts = rupu_netflow::ledger::host_rollup(&flows);
    NetflowResponse {
        flows: flows
            .into_iter()
            .map(|f| FlowView::from_flow(f, None))
            .collect(),
        hosts,
        window: WindowEcho::default(),
        dropped_total,
        asn_loaded: false,
    }
}

/// The three behaviors `enforce_range_on_proxied_response` promises,
/// proven over a real HTTP round trip rather than by calling the pure
/// function directly:
///
/// 1. `flows` is retained by the range this CP enforces, not whatever the
///    remote sent.
/// 2. `hosts` is RECOMPUTED from the retained set — not left as the
///    remote's own (here, unfiltered) rollup.
/// 3. `window` is stamped from the range THIS side enforced, not echoed
///    from the remote's (here, unbounded/null) value.
/// 4. `dropped_total` is left exactly as the remote reported it — it is
///    whole-ledger-file scoped by contract, so filtering must never touch
///    it.
#[tokio::test]
async fn host_proxy_recomputes_hosts_and_stamps_locally_enforced_window() {
    let tmp = tempfile::tempdir().unwrap();
    let run_id = "run_on_remote_host_flows";

    let mock = httpmock::MockServer::start_async().await;
    let (addr, host_id) = common::spawn_server_with_remote_host(tmp.path(), &mock.base_url()).await;
    let _probe = common::mock_probe_hit(&mock, run_id);

    let in_window = flow_at(150, "in.example");
    let out_of_window = flow_at(500, "out.example");
    let remote_resp = unfiltered_remote_response(vec![in_window, out_of_window], 3);

    let _netflow_mock = mock.mock(|when, then| {
        when.method("GET")
            .path(format!("/api/runs/{run_id}/netflow"));
        then.status(200)
            .json_body(serde_json::to_value(&remote_resp).unwrap());
    });

    let from = chrono::DateTime::from_timestamp(100, 0).unwrap();
    let to = chrono::DateTime::from_timestamp(200, 0).unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .query(&[("from", from.to_rfc3339()), ("to", to.to_rfc3339())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: NetflowResponse = resp.json().await.unwrap();

    assert_eq!(
        body.flows.len(),
        1,
        "the out-of-window flow must be dropped by the LOCAL enforcement, \
         not whatever the (unfiltered) remote sent: {:?}",
        body.flows
    );
    assert_eq!(body.flows[0].flow.host, "in.example");

    assert_eq!(
        body.hosts.len(),
        1,
        "hosts must be recomputed from the retained flow only, not left as \
         the remote's own 2-host unfiltered rollup: {:?}",
        body.hosts
    );
    assert_eq!(body.hosts[0].host, "in.example");

    assert_eq!(
        body.window.from,
        Some(from),
        "window must be stamped from the range THIS side enforced, not the \
         remote's unbounded echo"
    );
    assert_eq!(body.window.to, Some(to));

    assert_eq!(
        body.dropped_total, 3,
        "dropped_total is whole-ledger-file scoped and must be relayed \
         verbatim, never touched by the window correction"
    );

    // host_id sanity: confirms the run really did resolve onto the
    // registered remote (not e.g. silently served as `Unpersisted`/empty).
    assert!(!host_id.is_empty());
}

/// Version-skew path: an older remote emits the pre-rename `dropped` key
/// instead of `dropped_total`. `NetflowResponse` has no `#[serde(default)]`
/// on `dropped_total`, so this must fail closed with a NAMED version-skew
/// error — never silently default `dropped_total` to 0 and relay the rest
/// of the (looks-plausible) body as if it had been validated, and never a
/// bare/opaque serde error message an operator can't act on.
#[tokio::test]
async fn host_proxy_older_remote_dropped_rename_fails_closed_with_named_error() {
    let tmp = tempfile::tempdir().unwrap();
    let run_id = "run_on_remote_host_skew";

    let mock = httpmock::MockServer::start_async().await;
    let (addr, host_id) = common::spawn_server_with_remote_host(tmp.path(), &mock.base_url()).await;
    let _probe = common::mock_probe_hit(&mock, run_id);

    let remote_resp = unfiltered_remote_response(vec![flow_at(150, "in.example")], 7);
    let mut wire = serde_json::to_value(&remote_resp).unwrap();
    let dropped_value = wire
        .as_object_mut()
        .unwrap()
        .remove("dropped_total")
        .expect("sanity: NetflowResponse always serializes dropped_total");
    wire.as_object_mut()
        .unwrap()
        .insert("dropped".to_string(), dropped_value);

    let _netflow_mock = mock.mock(|when, then| {
        when.method("GET")
            .path(format!("/api/runs/{run_id}/netflow"));
        then.status(200).json_body(wire);
    });

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "a response this build can't parse must fail closed, never 200 \
         with data that only LOOKS filtered/valid"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    let message = body["error"]
        .as_str()
        .expect("ApiError body always carries a string `error` field");

    assert!(
        message.contains(&format!(
            "host {host_id} returned a netflow response this build cannot read"
        )),
        "expected the named version-skew message, got: {message:?}"
    );
    assert!(
        message.contains("the remote CP is likely older than this one"),
        "expected the version-skew explanation an operator can act on \
         (not a bare serde message), got: {message:?}"
    );
}
