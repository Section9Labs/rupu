//! Once SIGTERM has arrived (`credential_writes::terminating()`), the
//! recovery ladder selects, builds and announces no fallback hop. Here the
//! flag goes up while the provider serves a refused reply: the reply is
//! classified (its `Outcome` is written) and reaches the ladder, which must
//! close the run as aborted without calling the `HopBuilder`.
//!
//! Its own test binary: the flag is process-wide and never cleared, and the
//! refusal must be served before it goes up.

mod terminating_support;

use rupu_agent::runner::ScriptedTurn;
use rupu_providers::credential_writes;
use rupu_providers::types::{ContentBlock, Stop, StopReason, Usage};
use rupu_transcript::Event;
use terminating_support::{assert_aborted_without_a_hop, run_terminating};

#[tokio::test]
async fn a_refusal_served_as_sigterm_arrives_takes_no_fallback_hop() {
    assert!(!credential_writes::terminating(), "fresh process");
    // A refusal climbs rungs 1 and 2; the chain has a buildable entry on each.
    let (res, builds, events) = run_terminating(
        ScriptedTurn::Reply {
            content: vec![ContentBlock::Text { text: "no".into() }],
            stop: Stop::synthetic(StopReason::Refusal, "anthropic"),
            usage: Usage::default(),
        },
        "refusal",
    )
    .await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Outcome { outcome, .. } if outcome.class == "refusal"
        )),
        "the refusal was served and classified, so the ladder was entered: {events:?}"
    );
    assert_aborted_without_a_hop(&res, builds, &events);
}
