use crate::error::FleetError;
use crate::types::FleetMessage;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The reserved participant name the broadcast log lives under.
const BROADCAST: &str = "broadcast";

#[derive(Debug, Clone)]
pub struct Mailbox {
    pub root: PathBuf,
}

impl Mailbox {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn inbox_path(&self, participant: &str) -> PathBuf {
        self.root
            .join("mailboxes")
            .join(sanitize(participant))
            .join("inbox.jsonl")
    }

    /// Take the per-participant advisory lock that serializes `send` against
    /// `drain` (an exclusive `flock` on `<participant dir>/.lock`, held for as
    /// long as the returned `File` lives).
    ///
    /// Rename atomicity alone does not make a concurrent send safe: a `send`
    /// that already opened the inbox fd can write into the inode a `drain` has
    /// just renamed away and read, so the write "succeeds" and is lost. Holding
    /// this lock across a send's count+open+write, and across a drain's rename,
    /// means every send either finished before the rename (and is drained) or
    /// opens the fresh inbox after it (and is delivered by the next drain).
    ///
    /// The lock file is never removed; it only exists so `flock` has something
    /// to lock. The OS drops the lock when the `File` closes or the process
    /// dies, so a crashed holder can never wedge a mailbox.
    fn lock_exclusive(lock_path: &Path) -> Result<std::fs::File, FleetError> {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path)
            .map_err(|e| FleetError::Io {
                action: format!("open inbox lock {}", lock_path.display()),
                source: e,
            })?;
        loop {
            match rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive) {
                Ok(()) => return Ok(lock),
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => {
                    return Err(FleetError::Io {
                        action: format!("lock inbox {}", lock_path.display()),
                        source: e.into(),
                    })
                }
            }
        }
    }

    fn lock_path(&self, participant: &str) -> PathBuf {
        self.root
            .join("mailboxes")
            .join(sanitize(participant))
            .join(".lock")
    }

    fn broadcast_log_path(&self) -> PathBuf {
        self.root
            .join("mailboxes")
            .join(BROADCAST)
            .join("log.jsonl")
    }

    fn broadcast_cursor_path(&self, participant: &str) -> PathBuf {
        self.root
            .join("mailboxes")
            .join(sanitize(participant))
            .join("broadcast.cursor")
    }

    pub fn send(&self, to: &str, msg: &FleetMessage, cap: usize) -> Result<(), FleetError> {
        // `broadcast` is not a direct inbox: a plain `send` there would write a
        // file nobody drains. Callers broadcast via `broadcast_send`.
        if to == BROADCAST {
            return Err(FleetError::NotAnInbox {
                channel: to.to_string(),
            });
        }
        let path = self.inbox_path(to);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FleetError::Io {
                action: format!("create inbox dir {}", parent.display()),
                source: e,
            })?;
        }
        // Held (via `_lock`) until the end of the function: cap check, open and
        // append are one critical section, so the cap is exact and the append
        // can never land in an inode a concurrent drain has already taken.
        let _lock = Self::lock_exclusive(&self.lock_path(to))?;
        if count_lines(&path)? >= cap {
            return Err(FleetError::InboxFull {
                participant: to.to_string(),
                cap,
            });
        }
        let mut line = serde_json::to_vec(msg)?;
        line.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| FleetError::Io {
                action: format!("open inbox {}", path.display()),
                source: e,
            })?;
        f.write_all(&line).map_err(|e| FleetError::Io {
            action: format!("append inbox {}", path.display()),
            source: e,
        })
    }

    pub fn drain(&self, participant: &str) -> Result<Vec<FleetMessage>, FleetError> {
        let path = self.inbox_path(participant);
        // Atomically take the current inbox so concurrent sends land in a fresh file.
        let taken = path.with_extension(format!(
            "draining.{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        // Exclude in-flight sends for the whole drain. The rename is what
        // needs it (no sender can still hold the old inode afterwards, since
        // senders hold this lock from open to write); keeping it through the
        // read+delete also means two drains can never collide on the same
        // `draining.<nanos>` name.
        let _lock = match Self::lock_exclusive(&self.lock_path(participant)) {
            Ok(lock) => lock,
            // No mailbox directory yet: nothing was ever sent here.
            Err(FleetError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new())
            }
            Err(e) => return Err(e),
        };
        match std::fs::rename(&path, &taken) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(FleetError::Io {
                    action: format!("take inbox {}", path.display()),
                    source: e,
                })
            }
        }
        let msgs = read_messages(&taken)?;
        let _ = std::fs::remove_file(&taken);
        Ok(msgs)
    }

    /// Append `msg` to the shared broadcast log (`mailboxes/broadcast/log.jsonl`).
    ///
    /// Unlike [`Mailbox::send`], a broadcast is never consumed: it is an
    /// append-only log that every reader walks with its own cursor
    /// ([`Mailbox::read_broadcast`]). `cap` bounds the total number of lines in
    /// the log (the log is never trimmed), failing with
    /// [`FleetError::InboxFull`] once it is reached. The cap check and the
    /// append are one critical section under the broadcast lock.
    pub fn broadcast_send(&self, msg: &FleetMessage, cap: usize) -> Result<(), FleetError> {
        let path = self.broadcast_log_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FleetError::Io {
                action: format!("create broadcast dir {}", parent.display()),
                source: e,
            })?;
        }
        let _lock = Self::lock_exclusive(&self.lock_path(BROADCAST))?;
        if count_lines(&path)? >= cap {
            return Err(FleetError::InboxFull {
                participant: BROADCAST.to_string(),
                cap,
            });
        }
        let mut line = serde_json::to_vec(msg)?;
        line.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| FleetError::Io {
                action: format!("open broadcast log {}", path.display()),
                source: e,
            })?;
        f.write_all(&line).map_err(|e| FleetError::Io {
            action: format!("append broadcast log {}", path.display()),
            source: e,
        })
    }

    /// Return the broadcasts `participant` has not yet seen, and advance its
    /// cursor past them, so every reader sees each broadcast exactly once and
    /// no reader consumes one on another's behalf.
    ///
    /// The cursor (`mailboxes/<participant>/broadcast.cursor`) is the number of
    /// broadcast-log lines already delivered. A reader with no cursor yet is a
    /// late joiner: its cursor starts at the CURRENT end of the log, so its
    /// first read is empty (no backlog) and it sees only what is broadcast
    /// afterwards. An unreadable cursor is treated the same way, so a damaged
    /// file can never replay history.
    ///
    /// Only complete (newline-terminated) log lines are delivered; a line still
    /// being appended is picked up by the next read.
    pub fn read_broadcast(&self, participant: &str) -> Result<Vec<FleetMessage>, FleetError> {
        let log = self.broadcast_log_path();
        let cursor_path = self.broadcast_cursor_path(participant);
        let lock_path = self.lock_path(participant);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FleetError::Io {
                action: format!("create mailbox dir {}", parent.display()),
                source: e,
            })?;
        }
        let _lock = Self::lock_exclusive(&lock_path)?;

        let lines = complete_log_lines(&log)?;
        let total = lines.len();
        let cursor = match read_cursor(&cursor_path)? {
            Some(c) => c,
            None => {
                // First read: start at the tip. Nothing is delivered.
                write_cursor(&cursor_path, total)?;
                return Ok(Vec::new());
            }
        };

        let mut out = Vec::new();
        for line in lines.iter().skip(cursor) {
            match serde_json::from_str::<FleetMessage>(line) {
                Ok(m) => out.push(m),
                // A line we cannot parse is dropped rather than wedging every
                // reader on it forever; only this process writes the log.
                Err(e) => tracing::warn!(
                    path = %log.display(),
                    error = %e,
                    "broadcast log: skipping unparseable line"
                ),
            }
        }
        if total != cursor {
            write_cursor(&cursor_path, total)?;
        }
        Ok(out)
    }
}

fn count_lines(path: &Path) -> Result<usize, FleetError> {
    match std::fs::read(path) {
        Ok(b) => Ok(String::from_utf8_lossy(&b)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(FleetError::Io {
            action: format!("count {}", path.display()),
            source: e,
        }),
    }
}

/// The non-blank, newline-terminated lines of the broadcast log, in order. A
/// missing log is empty. The trailing fragment of a write still in flight has
/// no terminator yet and is excluded, so a reader never parses (or counts) a
/// half-written line.
fn complete_log_lines(path: &Path) -> Result<Vec<String>, FleetError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(FleetError::Io {
                action: format!("read broadcast log {}", path.display()),
                source: e,
            })
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let complete = match text.rfind('\n') {
        Some(i) => &text[..=i],
        None => return Ok(Vec::new()),
    };
    Ok(complete
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect())
}

/// A reader's persisted broadcast cursor, or `None` when it has none (or it is
/// not a valid integer).
fn read_cursor(path: &Path) -> Result<Option<usize>, FleetError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t.trim().parse().ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(FleetError::Io {
            action: format!("read broadcast cursor {}", path.display()),
            source: e,
        }),
    }
}

/// Persist a cursor atomically (temp file + rename), so a crash mid-write can
/// only leave the previous cursor, never a torn one.
fn write_cursor(path: &Path, value: usize) -> Result<(), FleetError> {
    let tmp = path.with_extension("cursor.tmp");
    std::fs::write(&tmp, format!("{value}\n")).map_err(|e| FleetError::Io {
        action: format!("write broadcast cursor {}", tmp.display()),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| FleetError::Io {
        action: format!("commit broadcast cursor {}", path.display()),
        source: e,
    })
}

fn read_messages(path: &Path) -> Result<Vec<FleetMessage>, FleetError> {
    let bytes = std::fs::read(path).map_err(|e| FleetError::Io {
        action: format!("read {}", path.display()),
        source: e,
    })?;
    let text = String::from_utf8_lossy(&bytes);
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        out.push(serde_json::from_str(line).map_err(|e| FleetError::Parse {
            path: path.display().to_string(),
            source: e,
        })?);
    }
    Ok(out)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(body: &str) -> FleetMessage {
        FleetMessage {
            from: "lead".into(),
            ts: "2026-10-01T00:00:00Z".into(),
            body: body.into(),
        }
    }

    #[test]
    fn send_then_drain_returns_messages_once() {
        let tmp = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(tmp.path());
        mb.send("heron", &msg("expand to staging subnet"), 100)
            .unwrap();
        mb.send("heron", &msg("here is more evidence"), 100)
            .unwrap();

        let first = mb.drain("heron").unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].body, "expand to staging subnet");

        // second drain is empty (messages consumed)
        assert!(mb.drain("heron").unwrap().is_empty());
    }

    #[test]
    fn send_rejects_when_inbox_full() {
        let tmp = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(tmp.path());
        mb.send("heron", &msg("a"), 1).unwrap();
        let err = mb.send("heron", &msg("b"), 1).unwrap_err();
        assert!(matches!(err, FleetError::InboxFull { cap: 1, .. }));
    }

    #[test]
    fn send_to_broadcast_is_refused() {
        // A plain `send` to the broadcast channel would write a file nobody
        // drains; it must be refused so callers use `broadcast_send`.
        let tmp = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(tmp.path());
        let err = mb.send("broadcast", &msg("x"), 64).unwrap_err();
        assert!(matches!(err, FleetError::NotAnInbox { .. }), "{err:?}");
        // and nothing was written to a would-be broadcast inbox
        assert!(mb.drain("broadcast").unwrap().is_empty());
    }

    /// Regression for "a concurrent send is never lost": a `send` that opened
    /// the inbox before a `drain` renamed it used to write into the unlinked
    /// inode, return Ok, and vanish. Hammer send against a drain loop and
    /// require that every message sent is drained exactly once.
    #[test]
    fn concurrent_sends_and_drains_lose_nothing() {
        use std::collections::BTreeSet;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        const ITERATIONS: usize = 20;
        const SENDERS: usize = 8;
        const PER_SENDER: usize = 50;

        for iter in 0..ITERATIONS {
            let tmp = tempfile::tempdir().unwrap();
            let mb = Arc::new(Mailbox::new(tmp.path()));
            let done = Arc::new(AtomicBool::new(false));

            let drainer = {
                let mb = Arc::clone(&mb);
                let done = Arc::clone(&done);
                std::thread::spawn(move || {
                    let mut got = Vec::new();
                    while !done.load(Ordering::SeqCst) {
                        got.extend(mb.drain("heron").unwrap());
                    }
                    got
                })
            };

            let senders: Vec<_> = (0..SENDERS)
                .map(|s| {
                    let mb = Arc::clone(&mb);
                    std::thread::spawn(move || {
                        for m in 0..PER_SENDER {
                            // cap far above the total so only loss, not
                            // backpressure, can make the counts differ.
                            mb.send("heron", &msg(&format!("s{s}-m{m}")), 1_000_000)
                                .unwrap();
                        }
                    })
                })
                .collect();
            for h in senders {
                h.join().unwrap();
            }
            done.store(true, Ordering::SeqCst);

            let mut drained = drainer.join().unwrap();
            // Anything that landed after the drainer's last pass.
            drained.extend(mb.drain("heron").unwrap());

            let bodies: Vec<String> = drained.into_iter().map(|m| m.body).collect();
            let unique: BTreeSet<&String> = bodies.iter().collect();
            assert_eq!(
                bodies.len(),
                SENDERS * PER_SENDER,
                "iteration {iter}: lost or duplicated messages"
            );
            assert_eq!(unique.len(), bodies.len(), "iteration {iter}: duplicates");
            for s in 0..SENDERS {
                for m in 0..PER_SENDER {
                    assert!(
                        unique.contains(&format!("s{s}-m{m}")),
                        "iteration {iter}: s{s}-m{m} was lost"
                    );
                }
            }
        }
    }

    #[test]
    fn concurrent_sends_respect_the_cap_exactly() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let mb = Arc::new(Mailbox::new(tmp.path()));
        let accepted = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|s| {
                let mb = Arc::clone(&mb);
                let accepted = Arc::clone(&accepted);
                std::thread::spawn(move || {
                    for m in 0..10 {
                        match mb.send("heron", &msg(&format!("s{s}-m{m}")), 5) {
                            Ok(()) => {
                                accepted.fetch_add(1, Ordering::SeqCst);
                            }
                            Err(FleetError::InboxFull { cap: 5, .. }) => {}
                            Err(e) => panic!("unexpected error: {e}"),
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(accepted.load(Ordering::SeqCst), 5);
        assert_eq!(mb.drain("heron").unwrap().len(), 5);
    }

    #[test]
    fn drain_of_unknown_participant_is_empty_and_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(tmp.path());
        assert!(mb.drain("ghost").unwrap().is_empty());
        assert!(!tmp.path().join("mailboxes").exists());
    }

    #[test]
    fn broadcast_fans_out_to_every_reader_once() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(dir.path());
        // Readers that joined before the broadcast.
        assert!(mb.read_broadcast("unit-a").unwrap().is_empty());
        assert!(mb.read_broadcast("unit-b").unwrap().is_empty());
        mb.broadcast_send(&msg("all hands"), 256).unwrap();
        let a1 = mb.read_broadcast("unit-a").unwrap();
        let b1 = mb.read_broadcast("unit-b").unwrap();
        assert_eq!(a1.len(), 1);
        assert_eq!(b1.len(), 1);
        assert_eq!(a1[0].body, "all hands");
        assert_eq!(b1[0].body, "all hands");
        // neither sees it again (cursor advanced)
        assert!(mb.read_broadcast("unit-a").unwrap().is_empty());
        assert!(mb.read_broadcast("unit-b").unwrap().is_empty());
    }

    #[test]
    fn a_late_reader_starts_at_the_current_log_tip() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(dir.path());
        mb.broadcast_send(&msg("early"), 256).unwrap();
        // late joiner's first read sees no backlog...
        assert!(mb.read_broadcast("late").unwrap().is_empty());
        mb.broadcast_send(&msg("after"), 256).unwrap();
        // ...but does see broadcasts sent after it first read.
        let got = mb.read_broadcast("late").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].body, "after");
    }

    #[test]
    fn a_reader_of_a_never_written_log_sees_the_first_broadcast() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(dir.path());
        assert!(mb.read_broadcast("early-bird").unwrap().is_empty());
        mb.broadcast_send(&msg("first ever"), 256).unwrap();
        let got = mb.read_broadcast("early-bird").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].body, "first ever");
    }

    #[test]
    fn broadcast_preserves_order_and_does_not_touch_direct_inboxes() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(dir.path());
        assert!(mb.read_broadcast("w").unwrap().is_empty());
        mb.send("w", &msg("direct"), 10).unwrap();
        mb.broadcast_send(&msg("one"), 10).unwrap();
        mb.broadcast_send(&msg("two"), 10).unwrap();
        let got: Vec<String> = mb
            .read_broadcast("w")
            .unwrap()
            .into_iter()
            .map(|m| m.body)
            .collect();
        assert_eq!(got, ["one", "two"]);
        // the direct inbox is independent: still drainable once.
        let direct = mb.drain("w").unwrap();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].body, "direct");
    }

    #[test]
    fn broadcast_send_respects_the_total_line_cap() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(dir.path());
        mb.broadcast_send(&msg("a"), 2).unwrap();
        mb.broadcast_send(&msg("b"), 2).unwrap();
        let err = mb.broadcast_send(&msg("c"), 2).unwrap_err();
        assert!(matches!(err, FleetError::InboxFull { cap: 2, .. }));
        // a read does not free room: the log is append-only.
        assert!(mb.read_broadcast("r").unwrap().is_empty());
        assert!(mb.broadcast_send(&msg("d"), 2).is_err());
    }

    #[test]
    fn a_damaged_cursor_resets_to_the_tip_instead_of_replaying() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(dir.path());
        mb.broadcast_send(&msg("old"), 10).unwrap();
        assert!(mb.read_broadcast("r").unwrap().is_empty());
        std::fs::write(mb.broadcast_cursor_path("r"), "not a number").unwrap();
        assert!(mb.read_broadcast("r").unwrap().is_empty());
        mb.broadcast_send(&msg("new"), 10).unwrap();
        let got = mb.read_broadcast("r").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].body, "new");
    }

    #[test]
    fn a_half_written_trailing_line_is_left_for_the_next_read() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(dir.path());
        assert!(mb.read_broadcast("r").unwrap().is_empty());
        mb.broadcast_send(&msg("whole"), 10).unwrap();
        // Simulate a writer caught mid-append: bytes with no terminator yet.
        let log = mb.broadcast_log_path();
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(br#"{"from":"lead","ts":"t","bo"#).unwrap();
        let first = mb.read_broadcast("r").unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].body, "whole");
        // Finish the line; the next read delivers it exactly once.
        f.write_all(b"dy\":\"rest\"}\n").unwrap();
        let second = mb.read_broadcast("r").unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].body, "rest");
        assert!(mb.read_broadcast("r").unwrap().is_empty());
    }
}
