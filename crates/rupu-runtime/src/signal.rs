//! Whether this process is being stopped by a signal. SIGTERM marks it
//! through [`rupu_providers::credential_writes::terminating`]; SIGINT, which
//! ends the process by its default action once the children its runs
//! launched are stopped, marks it here first. An end a run records
//! meanwhile is recorded as that stop
//! ([`rupu_transcript::StopCause::StoppedBySignal`]), not as a failure of
//! its own.

use std::sync::atomic::{AtomicBool, Ordering};

static SIGINT: AtomicBool = AtomicBool::new(false);

/// Mark the process as being stopped by SIGINT; never cleared.
pub fn mark_sigint() {
    SIGINT.store(true, Ordering::SeqCst);
}

pub fn stopped_by_signal() -> bool {
    SIGINT.load(Ordering::SeqCst) || rupu_providers::credential_writes::terminating()
}
