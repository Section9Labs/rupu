//! A run that fails while its process is being stopped by SIGINT (a Ctrl-C)
//! failed of that stop, as one under SIGTERM does: its record's typed cause
//! is the signal stop, not a failure of its own.
//!
//! Its own test binary: the flag is process-wide and never cleared.

use rupu_agent::RunError;
use rupu_transcript::StopCause;

#[test]
fn a_failure_under_sigint_is_the_signal_stop() {
    let failure = RunError::OperatorStop { turn: 2 };
    assert_eq!(failure.cause(), None);

    rupu_providers::credential_writes::mark_sigint();

    assert!(rupu_providers::credential_writes::stopped_by_signal());
    assert!(!rupu_providers::credential_writes::terminating());
    assert_eq!(
        failure.cause().as_ref().and_then(StopCause::of),
        Some(StopCause::StoppedBySignal)
    );
}
