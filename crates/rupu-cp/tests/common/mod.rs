//! Shared harness for driving a route whose target run resolves to
//! `RunLocation::Host` (`rupu-cp/src/api/run_resolve.rs`) — i.e. a run that
//! lives on a *registered remote host*, reached by proxying an HTTP request
//! through a `HostConnector`.
//!
//! ## Why this exists
//!
//! Every route that calls `resolve_run_location` (agents, transcripts,
//! sessions, source, runs, workflows, netflow) branches on `Global` /
//! `ProjectLocal` / `Host` / `Unpersisted` / `NotFound`. Before this file,
//! nothing in `rupu-cp`'s test suite drove any route's `Host` branch —
//! confirmed by grepping every file under `tests/` for `RunLocation`
//! (zero hits) and for `resolve_run_location` (zero hits) prior to this
//! module landing. The closest prior art, `tests/host_reads.rs`, drives the
//! *list* routes' fan-out across every registered host (`?host=<id>` /
//! no `host` param) — that never calls `resolve_run_location` at all,
//! because a list route doesn't resolve any single run's location; it
//! fans out to every host and merges. This harness is for the *other*
//! shape of route: one that takes a single run id, asks
//! `resolve_run_location` where it lives, and — in the `Host` branch —
//! proxies to that one host.
//!
//! ## How a run ends up `RunLocation::Host` in a test
//!
//! `resolve_run_location_uncached` (`run_resolve.rs`) tries, in order: the
//! global `RunStore`, every registered project's local store, the
//! autoflow-history `host_id` signal, then — the path this harness drives
//! — a bounded probe (`probe_hosts`) that fires `GET /api/runs/:id` at
//! every *registered* host and takes the first 2xx/parseable-JSON hit. So:
//! as long as a run id is not seeded into the global store, not seeded
//! into any registered project's local store, and not mentioned anywhere
//! in autoflow history, registering one remote host through the real
//! production mechanism (`HostRegistry::add_host`, an `HttpCp` transport
//! pointed at an `httpmock` server) and mocking that server's
//! `GET /api/runs/<id>` to answer 2xx is *sufficient* to make
//! `resolve_run_location` return `Host { host_id }` for it — no reaching
//! into private autoflow-history internals required, and no hand-mocked
//! `HostConnector` impl either: [`HttpHostConnector`](rupu_cp::host::http::HttpHostConnector)
//! is exercised for real, over a real loopback HTTP connection, with real
//! `serde_json` (de)serialization on both ends.
//!
//! ## Using this for another route
//!
//! 1. Call [`spawn_server_with_remote_host`] to get a running local CP
//!    (`addr`) with one remote host (`host_id`) registered, backed by your
//!    own `httpmock::MockServer`.
//! 2. Call [`mock_probe_hit`] for whatever run id you want to resolve onto
//!    that host.
//! 3. Mock the route-specific endpoint your route under test proxies to
//!    (e.g. for transcripts: `GET /api/transcript?path=...`; for a single
//!    run: `GET /api/runs/<id>` itself — see [`mock_probe_hit`]'s doc for
//!    the resulting registration-order subtlety if your route's own proxy
//!    target happens to collide with the probe's path) on the SAME
//!    `MockServer`, with whatever body exercises the behavior under test.
//! 4. Issue the request against `http://{addr}/<your route>` with
//!    `reqwest` and assert on the response. This drives the REAL route
//!    handler, the REAL `resolve_run_location` walk, and a REAL HTTP round
//!    trip to the mock server — catching wire/serde version-skew bugs a
//!    hand-built `HostConnector` mock cannot.

// Throwaway in-process mock-server client, not rupu's own egress — mirrors
// every other httpmock-based test file in this crate (e.g.
// `tests/host_reads.rs`).
#![allow(clippy::disallowed_methods)]
// Not every helper here is used by every test binary that `mod common;`s
// this file — `pub` items are exempt from the *private* dead-code lint
// already, but `mod`-level unused-import/-fn warnings can still fire for a
// binary that only uses a subset; silence rather than churn per-binary.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;

/// Start a local `rupu-cp` server with ONE remote host pre-registered
/// through the real production path (`HostRegistry::add_host`), pointed at
/// `mock_base_url`. Returns the local server's address and the newly
/// assigned `host_id`.
///
/// `global_dir` must be a fresh, unseeded directory — callers control what
/// a given run id resolves to purely through what they do (and don't) put
/// there, plus what they mock on the remote.
pub async fn spawn_server_with_remote_host(
    global_dir: &Path,
    mock_base_url: &str,
) -> (SocketAddr, String) {
    let state =
        rupu_cp::state::AppState::new(global_dir.into(), rupu_config::PricingConfig::default());
    let host = state
        .hosts
        .add_host("mock-remote", mock_base_url, None)
        .expect("add_host should succeed");
    let host_id = host.id.clone();
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, host_id)
}

/// Mount the `GET /api/runs/<run_id>` mock that `resolve_run_location`'s
/// bounded host-probe fallback (`api::run_resolve::probe_hosts`) fires at
/// every registered host to decide whether an otherwise-unplaced run lives
/// there. Any 2xx JSON body is sufficient — the probe only checks
/// `proxy_get_json(...).is_ok()`, never the body's shape.
///
/// Call this once per run id you want to land on the host, BEFORE the
/// request meant to resolve it there. `resolve_run_location` memoizes its
/// answer per run id for 8s (`RUN_LOCATION_CACHE_TTL`), which is generous
/// next to a single test's wall time, so one probe hit per run id per test
/// is enough even across several requests to the same route.
pub fn mock_probe_hit<'a>(mock: &'a httpmock::MockServer, run_id: &str) -> httpmock::Mock<'a> {
    mock.mock(|when, then| {
        when.method("GET").path(format!("/api/runs/{run_id}"));
        then.status(200).json_body(serde_json::json!({}));
    })
}
