//! `octocrab` calls are recorded at Coarse fidelity — host, outcome and
//! timing are real; bytes and peer IP are genuinely unknown and stay
//! `None` rather than being guessed.
//!
//! `a_coarse_record_states_what_it_does_not_know`,
//! `an_http_status_failure_records_an_http_error`, and
//! `a_genuine_network_failure_records_a_transport_error` exercise
//! `coarse_flow` directly — they prove the constructor's field shape and
//! its error-classification, nothing more.
//!
//! `a_retried_octocrab_attempt_emits_one_coarse_record_per_attempt` is the
//! test that actually proves emission: it drives the real
//! `GithubClient::with_retry_octocrab` — the production retry loop, not a
//! stand-in — with a closure that fails once (a `RateLimited` error, same
//! as a real octocrab 403/429 would classify to) and then succeeds, and
//! asserts the client's stored sink received exactly one `Coarse` record per
//! attempt: a failed one, then a succeeded one. That is the "retried call
//! emits more than one" case the task calls out explicitly.
//!
//! It cannot drive an actual octocrab HTTP call against a mock server and
//! still observe a clean two-attempt sequence without either (a) racing a
//! mock-server swap against `with_retry`'s real backoff sleep, or (b)
//! waiting out the real exponential backoff (>=1s for the first retry,
//! unavoidable — `classify_octocrab_error` discards response headers for
//! non-403 statuses, so even a `Retry-After` header can't shorten it).
//! Calling `with_retry_octocrab` directly with a hand-written closure
//! sidesteps both: it is still the exact production method under test,
//! just with a controlled `Result` instead of a real HTTP round trip, and
//! it lets the RateLimited case carry an explicit near-zero
//! `retry_after` so the test stays fast and deterministic.

use rupu_netflow::{Fidelity, MemorySink, Outcome};
use rupu_scm::connectors::github::client::GithubClient;
use rupu_scm::ScmError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn a_coarse_record_states_what_it_does_not_know() {
    let sink = Arc::new(MemorySink::default());
    let record = rupu_scm::connectors::github::client::coarse_flow(
        "api.github.com",
        "/repos/foo/bar",
        None,
        42,
    );
    rupu_netflow::FlowSink::record(sink.as_ref(), record).await;

    let r = &sink.records()[0];
    assert_eq!(r.fidelity, Fidelity::Coarse);
    assert_eq!(r.host, "api.github.com");
    assert_eq!(r.path, "/repos/foo/bar");
    assert_eq!(r.duration_ms, Some(42));
    assert!(matches!(r.outcome, Outcome::Ok));
    assert_eq!(r.bytes_in, None, "Coarse cannot know body size");
    assert_eq!(r.bytes_out, None, "Coarse cannot know body size");
    assert_eq!(r.peer_ip, None, "Coarse cannot know the peer");
    assert_eq!(
        r.status, None,
        "octocrab does not surface the raw status here"
    );
    assert!(
        !r.body_complete,
        "no body was observed at this boundary; asserting completeness would claim coverage we don't have"
    );
}

/// A rate-limited or missing-resource GitHub call is a real HTTP exchange
/// — GitHub answered with a status code — not a network fault. Every
/// `ScmError` variant classified from an HTTP status (see
/// `classify_scm_error` / `classify_octocrab_error`) must map to
/// `Outcome::HttpError`, not `TransportError`. This replaces the pre-fix
/// `a_failed_attempt_records_a_transport_error`, which asserted the
/// opposite (and wrong) mapping as correct.
#[tokio::test]
async fn an_http_status_failure_records_an_http_error() {
    let not_found = ScmError::NotFound {
        what: "repo".into(),
    };
    let record = rupu_scm::connectors::github::client::coarse_flow(
        "api.github.com",
        "/repos/foo/bar",
        Some(&not_found),
        10,
    );
    assert!(
        matches!(record.outcome, Outcome::HttpError),
        "a 404 is an HTTP exchange, not a transport fault: {:?}",
        record.outcome
    );

    let rate_limited = ScmError::RateLimited { retry_after: None };
    let record = rupu_scm::connectors::github::client::coarse_flow(
        "api.github.com",
        "/repos/foo/bar",
        Some(&rate_limited),
        10,
    );
    assert!(
        matches!(record.outcome, Outcome::HttpError),
        "a rate limit is an HTTP exchange, not a transport fault: {:?}",
        record.outcome
    );
}

/// A genuine transport fault (no HTTP response at all — octocrab's
/// `Hyper`/`Service` variants, classified to `ScmError::Network`) is the
/// ONLY case that should still record `Outcome::TransportError`.
#[tokio::test]
async fn a_genuine_network_failure_records_a_transport_error() {
    let network = ScmError::Network(anyhow::anyhow!("connection reset"));
    let record = rupu_scm::connectors::github::client::coarse_flow(
        "api.github.com",
        "/repos/foo/bar",
        Some(&network),
        10,
    );
    assert!(matches!(record.outcome, Outcome::TransportError));
}

/// Drives the real production retry loop (not a copy of it) with a
/// closure that fails on the first attempt and succeeds on the second,
/// and proves the client's stored sink receives one `Coarse` record per
/// attempt — not one per logical call.
#[tokio::test]
async fn a_retried_octocrab_attempt_emits_one_coarse_record_per_attempt() {
    rupu_scm::install_default_crypto_provider();
    let sink = Arc::new(MemorySink::default());

    // No base_url ⇒ host_label derives to "api.github.com" from the
    // default public GraphQL URL, computed once at construction.
    let client = GithubClient::new("ghp_fake".into(), None, Some(1), sink.clone());

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_closure = calls.clone();
    let result: Result<&'static str, ScmError> = client
        .with_retry_octocrab(move || {
            let calls_for_closure = calls_for_closure.clone();
            async move {
                let n = calls_for_closure.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // Same recoverable variant a real octocrab 403/429
                    // classifies to. A tiny explicit retry_after keeps
                    // the test fast without touching backoff() at all.
                    Err(ScmError::RateLimited {
                        retry_after: Some(Duration::from_millis(1)),
                    })
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

    assert_eq!(result.expect("second attempt succeeds"), "ok");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "exactly one retry happened"
    );

    let records = sink.records();
    assert_eq!(
        records.len(),
        2,
        "one Coarse record per attempt, not per logical call: {records:?}"
    );
    for r in &records {
        assert_eq!(r.fidelity, Fidelity::Coarse);
        assert_eq!(r.host, "api.github.com");
        assert_eq!(r.method, "*", "with_retry_octocrab does not know the verb");
        assert_eq!(r.path, "*", "with_retry_octocrab does not know the path");
        assert_eq!(r.bytes_in, None);
        assert_eq!(r.bytes_out, None);
        assert_eq!(r.peer_ip, None);
        assert_eq!(r.status, None);
    }
    assert_eq!(
        records[0].outcome,
        Outcome::HttpError,
        "first attempt failed with a rate limit — a real HTTP exchange, not a transport fault"
    );
    assert_eq!(records[1].outcome, Outcome::Ok, "second attempt succeeded");
}
