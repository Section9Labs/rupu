//! The subprocess-capture port.
//!
//! The agent's `bash` tool spawns children whose network connections the
//! in-process HTTP middleware cannot see. A backend (OS-specific, built in
//! a later plan) observes those connections and reports them to the run's
//! [`FlowSink`](crate::sink::FlowSink). This module defines only the seam:
//! the traits the bash tool calls and an inert [`NoopCapture`]. It has no
//! OS code.

use crate::sink::FlowSink;
use std::sync::Arc;

/// Everything a backend needs to attribute one bash call's connections.
pub struct CallAttribution {
    pub run_id: String,
    pub step_id: Option<String>,
    pub agent: Option<String>,
    pub codename: Option<String>,
    /// The model's tool-call id for this bash invocation.
    pub tool_call_id: String,
    /// Where this run's flows go.
    pub sink: Arc<dyn FlowSink>,
}

/// A capture backend, shared by every bash call in a process.
///
/// `begin` and [`CaptureCall::finished`] are synchronous and MUST be cheap:
/// heavy work belongs on the backend's own thread. Implementations must
/// never let a capture failure affect the bash call itself — a backend that
/// cannot capture reports that through a `LedgerLine::Capture` line and
/// hands back an inert call.
pub trait SubprocessCapture: Send + Sync {
    /// Start tracking one bash call.
    fn begin(&self, call: CallAttribution) -> Box<dyn CaptureCall>;

    /// A run ended; release anything held for it.
    fn run_finished(&self, run_id: &str);
}

/// One tracked bash call.
pub trait CaptureCall: Send {
    /// Shell text to prepend to the command (e.g. to enter a cgroup), if
    /// the backend needs the child to cooperate.
    fn shell_prefix(&self) -> Option<String>;

    /// The child was spawned with this pid.
    fn spawned(&mut self, pid: u32);

    /// The call is over; flush its flows.
    fn finished(self: Box<Self>);
}

/// The capture a disabled or unsupported backend hands back: inert.
pub struct NoopCapture;

struct NoopCall;

impl SubprocessCapture for NoopCapture {
    fn begin(&self, _call: CallAttribution) -> Box<dyn CaptureCall> {
        Box::new(NoopCall)
    }

    fn run_finished(&self, _run_id: &str) {}
}

impl CaptureCall for NoopCall {
    fn shell_prefix(&self) -> Option<String> {
        None
    }

    fn spawned(&mut self, _pid: u32) {}

    fn finished(self: Box<Self>) {}
}
