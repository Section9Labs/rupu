//! Once SIGTERM has arrived (`credential_writes::terminating()`), a provider
//! error is not an outcome and climbs no ladder: the run closes as aborted
//! and the `HopBuilder` is never called. The provider raises the flag as it
//! returns the error, so the error reaches the runner's error handling with
//! the flag already up.
//!
//! Its own test binary: the flag is process-wide and never cleared. The
//! refused-reply case is `terminating_fallback_refusal.rs`, for the same
//! reason.

mod terminating_support;

use rupu_agent::runner::ScriptedTurn;
use rupu_providers::credential_writes;
use rupu_providers::reply_error::{parse_error_body, ErrorOrigin};
use rupu_transcript::Event;
use terminating_support::{assert_aborted_without_a_hop, run_terminating};

#[tokio::test]
async fn a_provider_error_while_terminating_takes_no_fallback_hop() {
    assert!(!credential_writes::terminating(), "fresh process");
    // An error the ladder would hop on (not found: rungs 1 and 2).
    let (res, builds, events) = run_terminating(
        ScriptedTurn::ReplyError {
            body: parse_error_body(
                "anthropic",
                ErrorOrigin::Http { status: 404 },
                r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-opus-5-5"}}"#,
                None,
                None,
            ),
        },
        "error",
    )
    .await;
    assert_aborted_without_a_hop(&res, builds, &events);
    assert!(
        !events.iter().any(|e| matches!(e, Event::Outcome { .. })),
        "an error while terminating is not an outcome: {events:?}"
    );
}
