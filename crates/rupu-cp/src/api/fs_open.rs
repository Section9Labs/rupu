//! Opening agent-writable workspace files without a check-then-open race.
//!
//! Agents run as the same OS user as `cp serve`, so anything under a
//! workspace can change between a handler's `is_file()` check and its
//! `open`. Swapping in a FIFO there turns a plain blocking `open(O_RDONLY)`
//! into a wait for a writer that never comes, parking the thread (a
//! blocking-pool slot, or a runtime worker on an async path) indefinitely.
//! [`open_regular_file`] closes that window by opening first and checking
//! the type on the handle it got.

use rustix::fs::{Mode, OFlags};
use std::fs::File;
use std::path::Path as FsPath;

/// Open `path` read-only and hand it back only if the opened handle is a
/// regular file; FIFOs, sockets, devices and directories are refused.
///
/// - `O_NONBLOCK` makes opening a FIFO return immediately (no writer wait);
///   the type check then rejects it. On a regular file it has no effect on
///   reads, so it is left set.
/// - `O_NOFOLLOW` refuses a symlink in the *final* component only. Parent
///   directories are still followed, so callers keep their containment
///   check (`source::resolve_under_workspace`) and pass its canonical path.
/// - `O_NOCTTY` keeps a swapped-in terminal device from becoming the
///   daemon's controlling terminal before the type check refuses it.
///
/// Blocking syscalls: async callers run this under `spawn_blocking`.
pub(crate) fn open_regular_file(path: &FsPath) -> std::io::Result<File> {
    let flags =
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::NOCTTY | OFlags::CLOEXEC;
    let file = File::from(rustix::fs::open(path, flags, Mode::empty())?);
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ))
    }
}

/// FIFO + deadline helpers for tests that need an open to fail fast rather
/// than hang the suite.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path as FsPath;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Create a FIFO at `path` via the system `mkfifo`; `false` when that's
    /// unavailable, in which case the caller skips.
    pub(crate) fn mkfifo(path: &FsPath) -> bool {
        let made = std::process::Command::new("mkfifo").arg(path).status();
        if made.is_ok_and(|s| s.success()) {
            true
        } else {
            eprintln!("mkfifo unavailable; skipping");
            false
        }
    }

    /// Run `f` on a helper thread and panic (rather than hang the suite) if
    /// it hasn't returned within a few seconds.
    pub(crate) fn within_deadline<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("blocked instead of returning promptly")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{mkfifo, within_deadline};
    use super::*;
    use std::io::Read;

    fn open_with_deadline(path: &FsPath) -> std::io::Result<File> {
        let path = path.to_path_buf();
        within_deadline(move || open_regular_file(&path))
    }

    #[test]
    fn reads_a_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();

        let mut text = String::new();
        open_with_deadline(&path)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "fn main() {}\n");
    }

    #[test]
    fn refuses_a_fifo_promptly() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pipe.rs");
        if !mkfifo(&path) {
            return;
        }
        // No writer ever opens the FIFO: a blocking open would park here
        // forever, so the deadline is what this test actually asserts.
        assert!(open_with_deadline(&path).is_err());
    }

    #[test]
    fn refuses_a_symlinked_final_component() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("real.rs");
        std::fs::write(&target, "fn main() {}\n").unwrap();
        let link = tmp.path().join("link.rs");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(open_with_deadline(&link).is_err());
    }

    #[test]
    fn refuses_a_directory() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(open_with_deadline(tmp.path()).is_err());
    }
}
