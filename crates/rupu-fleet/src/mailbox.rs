use crate::error::FleetError;
use crate::types::FleetMessage;
use std::io::Write;
use std::path::{Path, PathBuf};

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

    pub fn send(&self, to: &str, msg: &FleetMessage, cap: usize) -> Result<(), FleetError> {
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
}
