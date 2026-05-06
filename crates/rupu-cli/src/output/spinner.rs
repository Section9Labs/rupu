//! Animated spinner for streaming steps.
//!
//! `Spinner` cycles through `◐ ◓ ◑ ◒` every 125 ms and writes the current
//! frame to the saved cursor position via ANSI `\x1b[s` / `\x1b[u`. It runs
//! on its own `std::thread` so it is usable in synchronous polling contexts.
//!
//! # Cross-terminal compatibility note
//! `\x1b[s` / `\x1b[u` (ANSI save/restore cursor) are supported by every
//! VT100-compatible emulator (iTerm2, Terminal.app, Alacritty, Windows
//! Terminal, tmux, GNU Screen). They are *not* available in raw pipes or
//! dumb terminals. `std::io::IsTerminal` is checked at construction time; if
//! the stream is not a TTY the spinner degrades to a no-op (no animation, no
//! cursor noise).

use std::io::{self, IsTerminal, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

// ── Frame sequence ────────────────────────────────────────────────────────────

/// The four spinner frames in order.
pub const FRAMES: [char; 4] = ['◐', '◓', '◑', '◒'];

/// Return the frame at position `idx % 4`.
#[inline]
pub fn frame_at(idx: usize) -> char {
    FRAMES[idx % FRAMES.len()]
}

// ── SpinnerHandle ─────────────────────────────────────────────────────────────

/// Returned by [`Spinner::start_if_tty`]. Dropping (or calling
/// [`SpinnerHandle::stop`]) signals the animation thread to exit.
pub struct SpinnerHandle {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SpinnerHandle {
    /// Stop the spinner and join the thread. Idempotent.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for SpinnerHandle {
    fn drop(&mut self) {
        // Signal the thread. We don't join here (might be called from
        // an async context where blocking is undesirable), but the
        // thread is short-lived and will exit within 125 ms.
        self.stop.store(true, Ordering::Relaxed);
    }
}

// ── Spinner ───────────────────────────────────────────────────────────────────

/// Manages a single animated spinner cell in the line-stream output.
///
/// Usage:
/// ```text
/// // Before printing the step_start line, save cursor position:
/// //   print!("\x1b[s");
/// // Print the glyph + step line.
/// // Then start the spinner — it restores to that saved pos each tick.
/// let handle = Spinner::start_if_tty("...");
/// // ... step runs ...
/// handle.stop(); // or just drop it
/// ```
pub struct Spinner;

impl Spinner {
    /// Start the spinner background thread if stdout is a TTY.
    ///
    /// The caller must have already emitted `\x1b[s` (save cursor) to the
    /// terminal *before* calling this, and the cursor must be positioned such
    /// that the spinner glyph occupies one character cell at the saved
    /// position.
    ///
    /// `color_ansi` is the complete ANSI SGR string to wrap each frame, e.g.
    /// `"\x1b[38;2;59;130;246m"`. Pass `""` for no color (`NO_COLOR` mode).
    pub fn start_if_tty(color_ansi: &str) -> SpinnerHandle {
        let stop = Arc::new(AtomicBool::new(false));

        if !io::stdout().is_terminal() {
            // Non-TTY (pipe, CI runner, etc.) — no animation.
            return SpinnerHandle { stop, thread: None };
        }

        let color_owned = color_ansi.to_string();
        let stop_clone = Arc::clone(&stop);

        let thread = std::thread::spawn(move || {
            let reset = "\x1b[0m";
            let mut idx = 0usize;
            while !stop_clone.load(Ordering::Relaxed) {
                let frame = frame_at(idx);
                // Restore saved cursor position, overwrite the glyph cell,
                // then re-save so the next tick lands in the same spot.
                let cell = format!(
                    "\x1b[u{color}{frame}{reset}\x1b[s",
                    color = color_owned,
                    frame = frame,
                    reset = reset,
                );
                let mut stdout = io::stdout();
                let _ = stdout.write_all(cell.as_bytes());
                let _ = stdout.flush();
                idx += 1;
                std::thread::sleep(Duration::from_millis(125));
            }
            // Restore cursor one final time so printing continues from the
            // right position after the spinner stops.
            let _ = io::stdout().write_all(b"\x1b[u");
            let _ = io::stdout().flush();
        });

        SpinnerHandle {
            stop,
            thread: Some(thread),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_sequence_cycles() {
        assert_eq!(frame_at(0), '◐');
        assert_eq!(frame_at(1), '◓');
        assert_eq!(frame_at(2), '◑');
        assert_eq!(frame_at(3), '◒');
        // Wraps around after 4.
        assert_eq!(frame_at(4), '◐');
        assert_eq!(frame_at(7), '◒');
        assert_eq!(frame_at(8), '◐');
    }

    #[test]
    fn all_frames_are_unique() {
        let frames = FRAMES;
        let mut seen = std::collections::HashSet::new();
        for f in frames {
            assert!(seen.insert(f), "duplicate frame: {f}");
        }
    }

    #[test]
    fn frames_len_is_4() {
        assert_eq!(FRAMES.len(), 4);
    }

    #[test]
    fn non_tty_returns_noop_handle() {
        // In test runner (non-TTY) the handle should be a no-op with no thread.
        // We can't easily assert there's no thread, but we can verify the
        // handle drops without panicking.
        let handle = Spinner::start_if_tty("\x1b[38;2;59;130;246m");
        drop(handle);
    }
}
