//! Inert capture backend for platforms without socket observation.
//!
//! It cannot observe anything, but it must not be silent about it: the
//! first bash call of each run writes one `LedgerLine::Capture` with
//! `CaptureState::Unavailable { reason }` so the CP can say capture was
//! unavailable rather than imply "no connections".

use rupu_netflow::{
    CallAttribution, CaptureCall, CaptureState, CaptureStateLine, SubprocessCapture,
};
use std::collections::HashSet;
use std::sync::Mutex;

/// Hands back inert calls and announces `Unavailable` once per run.
pub struct UnsupportedCapture {
    pub reason: String,
    announced: Mutex<HashSet<String>>,
}

impl UnsupportedCapture {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            announced: Mutex::new(HashSet::new()),
        }
    }

    /// Whether this is the first time `run_id` has been seen.
    fn first_sighting(&self, run_id: &str) -> bool {
        match self.announced.lock() {
            Ok(mut set) => set.insert(run_id.to_string()),
            // A poisoned marker set: announcing again is harmless
            // (a duplicate informational line), staying silent is not.
            Err(poisoned) => poisoned.into_inner().insert(run_id.to_string()),
        }
    }
}

impl SubprocessCapture for UnsupportedCapture {
    fn begin(&self, call: CallAttribution) -> Box<dyn CaptureCall> {
        if self.first_sighting(&call.run_id) {
            let line = CaptureStateLine {
                state: CaptureState::Unavailable {
                    reason: self.reason.clone(),
                },
                tool_call_id: Some(call.tool_call_id),
                note: None,
            };
            let sink = call.sink;
            match tokio::runtime::Handle::try_current() {
                // `begin` is sync and must be cheap: hand the one-shot
                // write to the ambient runtime.
                Ok(handle) => {
                    handle.spawn(async move { sink.capture_state(line).await });
                }
                // No runtime (a sync caller): drive the write on a
                // transient current-thread runtime. Sinks only enqueue.
                Err(_) => match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt.block_on(sink.capture_state(line)),
                    Err(e) => tracing::debug!(
                        error = %e,
                        "netwatch: no runtime to announce capture unavailable"
                    ),
                },
            }
        }
        Box::new(InertCall)
    }

    fn run_finished(&self, run_id: &str) {
        match self.announced.lock() {
            Ok(mut set) => {
                set.remove(run_id);
            }
            Err(poisoned) => {
                poisoned.into_inner().remove(run_id);
            }
        }
    }
}

struct InertCall;

impl CaptureCall for InertCall {
    fn shell_prefix(&self) -> Option<String> {
        None
    }

    fn spawned(&mut self, _pid: u32) {}

    fn finished(self: Box<Self>) {}
}
