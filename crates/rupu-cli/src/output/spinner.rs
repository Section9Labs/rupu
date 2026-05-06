//! No-op spinner shim.
//!
//! Earlier versions of this module ran a background thread that
//! re-wrote the cursor cell every 125 ms via ANSI `\x1b[s` /
//! `\x1b[u` (save / restore cursor). That design was structurally
//! broken: the spinner thread and the main print thread shared the
//! same cursor, so any text the print thread emitted would either
//! overwrite or be overwritten by the spinner's restore. The visible
//! result was scrambled output and a glyph that did not actually
//! animate.
//!
//! For a streaming line UI you cannot animate a glyph that has been
//! scrolled past — that's a constraint of the medium, not a bug to
//! fix. This module now exposes a no-op `SpinnerHandle` so callers
//! that previously held one don't break, but it does nothing.
//! `step_start` prints a static `◐` glyph instead, and the step
//! footer (`✓` / `✗`) is the visual completion cue.

/// Inert handle. Kept for caller compatibility; performs no work.
pub struct SpinnerHandle;

impl SpinnerHandle {
    /// No-op stop. Kept for symmetry with the old API.
    pub fn stop(self) {}
}

/// No-op spinner factory.
pub struct Spinner;

impl Spinner {
    /// Returns an inert handle. Performs no terminal I/O.
    pub fn start() -> SpinnerHandle {
        SpinnerHandle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_stop_does_not_panic() {
        let h = Spinner::start();
        h.stop();
    }
}
