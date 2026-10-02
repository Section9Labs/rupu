use crate::error::FleetError;
use crate::types::{BoardPost, ClaimGuard, ClaimOutcome, ClaimRecord, Directive};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Upper bound on claim/reap retries under contention (about 1s at
/// `RETRY_INTERVAL`). Exhausting it yields `Denied`, never `Err`.
const CLAIM_ATTEMPTS: u32 = 200;
const RETRY_INTERVAL: Duration = Duration::from_millis(5);
/// Longest readable prefix kept in a claim filename.
const STEM_PREFIX_MAX: usize = 64;

#[derive(Debug, Clone)]
pub struct Board {
    pub root: PathBuf,
}

impl Board {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn claims_dir(&self) -> PathBuf {
        self.root.join("board").join("claims")
    }

    fn claim_path(&self, key: &str) -> PathBuf {
        self.claims_dir().join(format!("{}.json", claim_stem(key)))
    }

    /// Lock target for serializing reaps of `key`. Never `O_EXCL`-created and
    /// never removed: it only exists so `flock` has something to lock.
    fn reap_lock_path(&self, key: &str) -> PathBuf {
        self.claims_dir()
            .join(format!("{}.reaplock", claim_stem(key)))
    }

    /// Atomically claim a work unit. Returns `Granted` with an RAII guard, or
    /// `Denied` with the current holder. A claim whose lease has expired is
    /// reaped and re-granted. Mirrors `AutoflowClaimStore::try_acquire_active_lock`.
    ///
    /// Contention never surfaces as `Err`: under any number of concurrent
    /// claimants exactly one is `Granted` and the rest are `Denied`. Reaping an
    /// expired lease is serialized per key by an `O_EXCL` mutex so a claimant
    /// holding a stale read can never delete another claimant's live lease.
    pub fn claim(&self, key: &str, owner: &str, ttl: Duration) -> Result<ClaimOutcome, FleetError> {
        let path = self.claim_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FleetError::Io {
                action: format!("create claims dir {}", parent.display()),
                source: e,
            })?;
        }
        let mut last_holder = String::new();
        for _ in 0..CLAIM_ATTEMPTS {
            if self.try_create_claim(&path, owner, ttl)? {
                return Ok(ClaimOutcome::Granted(ClaimGuard { path }));
            }
            // A lease exists. Inspect it.
            let Some(rec) = read_claim(&path)? else {
                // Released or reaped since our create attempt: just retry.
                continue;
            };
            if !is_expired(&rec) {
                return Ok(ClaimOutcome::Denied { holder: rec.owner });
            }
            last_holder = rec.owner;
            // Expired: one claimant at a time may reap. Whether we reaped, found
            // the lease already gone, or lost the mutex, we retry the create.
            if !self.reap_expired(key, &path)? {
                std::thread::sleep(RETRY_INTERVAL);
            }
        }
        Ok(ClaimOutcome::Denied {
            holder: last_holder,
        })
    }

    /// `Ok(true)` if the create succeeded, `Ok(false)` if a lease already
    /// exists (contention is not an error).
    fn try_create_claim(
        &self,
        path: &Path,
        owner: &str,
        ttl: Duration,
    ) -> Result<bool, FleetError> {
        let now = Utc::now();
        let expires =
            now + chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::seconds(0));
        let rec = ClaimRecord {
            owner: owner.to_string(),
            acquired_at: now.to_rfc3339(),
            lease_expires_at: expires.to_rfc3339(),
        };
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
        {
            Ok(mut f) => {
                let bytes = serde_json::to_vec(&rec)?;
                f.write_all(&bytes).map_err(|e| FleetError::Io {
                    action: format!("write claim {}", path.display()),
                    source: e,
                })?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(FleetError::Io {
                action: format!("create claim {}", path.display()),
                source: e,
            }),
        }
    }

    /// Try to remove an expired lease while holding the per-key reap lock (an
    /// advisory `flock` on `<stem>.reaplock`). Returns `Ok(true)` if this call
    /// was the sole reaper (the caller should retry the create immediately) and
    /// `Ok(false)` if another claimant holds the lock (the caller should back
    /// off briefly, then retry).
    ///
    /// The lease is re-read UNDER the lock and removed only if it is still
    /// expired: a stale read taken before another claimant reaped and
    /// re-granted must never delete that claimant's live lease. A lease that is
    /// already gone is not an error — someone else reaped it. The OS drops the
    /// lock when the `File` closes or the process dies, so a crashed reaper can
    /// never wedge the key.
    fn reap_expired(&self, key: &str, claim_path: &Path) -> Result<bool, FleetError> {
        let lock_path = self.reap_lock_path(key);
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| FleetError::Io {
                action: format!("open reap lock {}", lock_path.display()),
                source: e,
            })?;
        match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(rustix::io::Errno::WOULDBLOCK) => return Ok(false),
            Err(e) => {
                return Err(FleetError::Io {
                    action: format!("lock reap lock {}", lock_path.display()),
                    source: e.into(),
                })
            }
        }
        // Sole reaper until `lock` drops at the end of this function.
        if let Some(rec) = read_claim(claim_path)? {
            if is_expired(&rec) {
                match std::fs::remove_file(claim_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(FleetError::Io {
                            action: format!("reap claim {}", claim_path.display()),
                            source: e,
                        })
                    }
                }
            }
        }
        Ok(true)
    }

    pub fn claim_holder(&self, key: &str) -> Result<Option<String>, FleetError> {
        Ok(read_claim(&self.claim_path(key))?.map(|r| r.owner))
    }

    fn posts_path(&self) -> PathBuf {
        self.root.join("board").join("posts.jsonl")
    }

    /// Append one post to the board's append-only `posts.jsonl`.
    pub fn post(&self, post: &BoardPost) -> Result<(), FleetError> {
        append_jsonl(&self.posts_path(), post)
    }

    /// All posts, oldest first. An absent log is an empty board, not an error.
    pub fn read_posts(&self) -> Result<Vec<BoardPost>, FleetError> {
        read_jsonl(&self.posts_path())
    }

    fn directives_path(&self) -> PathBuf {
        self.root.join("board").join("directives.jsonl")
    }

    /// Append one lead->fleet directive to `directives.jsonl`.
    pub fn put_directive(&self, directive: &Directive) -> Result<(), FleetError> {
        append_jsonl(&self.directives_path(), directive)
    }

    /// All directives, oldest first. An absent log yields an empty list.
    pub fn read_directives(&self) -> Result<Vec<Directive>, FleetError> {
        read_jsonl(&self.directives_path())
    }
}

fn is_expired(rec: &ClaimRecord) -> bool {
    chrono::DateTime::parse_from_rfc3339(&rec.lease_expires_at)
        .map(|t| t.with_timezone(&Utc) <= Utc::now())
        .unwrap_or(false)
}

/// How long a reader waits for a just-created claim file to be populated.
const SETTLE_ATTEMPTS: u32 = 200;
const SETTLE_INTERVAL: Duration = Duration::from_millis(5);

/// Read a claim record. The claim file is created with `O_EXCL` *before* its
/// record is written, so a racing reader can briefly observe an empty file;
/// that is "being written", not corruption, so wait for it to settle (bounded)
/// instead of surfacing a spurious parse error.
fn read_claim(path: &Path) -> Result<Option<ClaimRecord>, FleetError> {
    let mut attempts = 0;
    loop {
        match std::fs::read(path) {
            Ok(bytes) if bytes.is_empty() && attempts < SETTLE_ATTEMPTS => {
                attempts += 1;
                std::thread::sleep(SETTLE_INTERVAL);
            }
            Ok(bytes) => {
                let rec = serde_json::from_slice(&bytes).map_err(|e| FleetError::Parse {
                    path: path.display().to_string(),
                    source: e,
                })?;
                return Ok(Some(rec));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(FleetError::Io {
                    action: format!("read claim {}", path.display()),
                    source: e,
                })
            }
        }
    }
}

/// Append one JSON line to `path`, creating the file and parent dirs as
/// needed. The whole line is written with a single `write_all` on an
/// `O_APPEND` handle so concurrent appenders do not interleave mid-line.
fn append_jsonl<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), FleetError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| FleetError::Io {
            action: format!("create dir {}", parent.display()),
            source: e,
        })?;
    }
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| FleetError::Io {
            action: format!("open {}", path.display()),
            source: e,
        })?;
    f.write_all(&line).map_err(|e| FleetError::Io {
        action: format!("append {}", path.display()),
        source: e,
    })
}

/// Read every JSON line of `path`, oldest first. A missing file is an empty
/// log; blank lines are skipped; a malformed line is a `Parse` error.
fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, FleetError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(FleetError::Io {
                action: format!("read {}", path.display()),
                source: e,
            })
        }
    };
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

/// Filename stem for a work-unit key: a readable sanitized prefix (for
/// debuggability) plus a short hex SHA-256 of the RAW key, so distinct keys
/// never collide (`a:b` vs `a-b`).
fn claim_stem(key: &str) -> String {
    let readable: String = key
        .chars()
        .take(STEM_PREFIX_MAX)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let digest = Sha256::digest(key.as_bytes());
    let hash: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{readable}-{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PostKind;

    #[test]
    fn claim_grants_then_denies_same_key() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());

        let first = board
            .claim("host:1.1.2.2", "agent-a", Duration::from_secs(60))
            .unwrap();
        assert!(matches!(first, ClaimOutcome::Granted(_)));

        let second = board
            .claim("host:1.1.2.2", "agent-b", Duration::from_secs(60))
            .unwrap();
        match second {
            ClaimOutcome::Denied { holder } => assert_eq!(holder, "agent-a"),
            ClaimOutcome::Granted(_) => panic!("second claim should be denied"),
        }
    }

    #[test]
    fn expired_lease_is_reaped_and_reclaimable() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());

        // zero TTL => immediately expired
        let _ = board
            .claim("svc:x", "agent-a", Duration::from_secs(0))
            .unwrap();
        // guard dropped at end of statement? No — bind it then drop explicitly:
        let g = board
            .claim("svc:y", "agent-a", Duration::from_secs(0))
            .unwrap();
        drop(g); // release so only the file remains to test reaping of a leftover

        // re-create a leftover expired lock by claiming with zero ttl and leaking the guard
        let g2 = board
            .claim("svc:z", "agent-a", Duration::from_secs(0))
            .unwrap();
        std::mem::forget(g2); // simulate a crashed owner that left the lock behind

        let outcome = board
            .claim("svc:z", "agent-b", Duration::from_secs(60))
            .unwrap();
        assert!(
            matches!(outcome, ClaimOutcome::Granted(_)),
            "expired lease must be reclaimable"
        );
    }

    #[test]
    fn concurrent_claims_have_exactly_one_winner() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut handles = Vec::new();
        for i in 0..16 {
            let root = root.clone();
            handles.push(std::thread::spawn(move || {
                let board = Board::new(&root);
                match board
                    .claim("race:key", &format!("agent-{i}"), Duration::from_secs(60))
                    .unwrap()
                {
                    ClaimOutcome::Granted(g) => {
                        // Hold the claim for the test's duration: leak the guard.
                        std::mem::forget(g);
                        true
                    }
                    ClaimOutcome::Denied { .. } => false,
                }
            }));
        }
        let wins = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(wins, 1, "exactly one thread may win the claim");
    }

    #[test]
    fn contention_on_expired_lease_has_exactly_one_winner_and_no_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        // Pre-expire a lock and leak the guard: a crashed owner's leftover.
        {
            let board = Board::new(&root);
            let g = board
                .claim("race:expired", "dead-owner", Duration::from_secs(0))
                .unwrap();
            let ClaimOutcome::Granted(g) = g else {
                panic!("seed claim must be granted");
            };
            std::mem::forget(g);
        }

        const N: usize = 16;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));
        let mut handles = Vec::new();
        for i in 0..N {
            let root = root.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let board = Board::new(&root);
                barrier.wait();
                board.claim(
                    "race:expired",
                    &format!("agent-{i}"),
                    Duration::from_secs(60),
                )
            }));
        }
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let errs: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        assert!(errs.is_empty(), "contention must never yield Err: {errs:?}");
        let wins = results
            .iter()
            .filter(|r| matches!(r, Ok(ClaimOutcome::Granted(_))))
            .count();
        assert_eq!(wins, 1, "exactly one claimant may win an expired lease");
    }

    #[test]
    fn keys_that_differ_only_in_punctuation_get_independent_claims() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());

        let pairs = [
            ("a:b", "a-b"),
            ("service:1.1.2.2:443", "service-1.1.2.2-443"),
            ("host:1.1.2.2", "host-1.1.2.2"),
        ];
        let mut guards = Vec::new();
        for (x, y) in pairs {
            for key in [x, y] {
                match board.claim(key, "agent", Duration::from_secs(60)).unwrap() {
                    ClaimOutcome::Granted(g) => guards.push(g),
                    ClaimOutcome::Denied { holder } => {
                        panic!("{key} wrongly denied (held by {holder}): distinct keys collided")
                    }
                }
            }
        }
        // The same raw key still contends with itself.
        assert!(matches!(
            board
                .claim("a:b", "other", Duration::from_secs(60))
                .unwrap(),
            ClaimOutcome::Denied { .. }
        ));
    }

    #[test]
    fn leftover_reaplock_with_no_live_holder_does_not_wedge_the_key() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());

        // Expired lease left by a dead owner.
        let ClaimOutcome::Granted(g) = board
            .claim("wedge:key", "dead-owner", Duration::from_secs(0))
            .unwrap()
        else {
            panic!("seed claim must be granted");
        };
        std::mem::forget(g);

        // A `.reaplock` left behind by a reaper that crashed. The OS dropped
        // its flock with the process, so there is no live holder.
        std::fs::File::create(board.reap_lock_path("wedge:key")).unwrap();

        let outcome = board
            .claim("wedge:key", "agent-b", Duration::from_secs(60))
            .unwrap();
        assert!(
            matches!(outcome, ClaimOutcome::Granted(_)),
            "a leftover reaplock with no live holder must not wedge the key"
        );
    }

    #[test]
    fn live_reap_lock_holder_blocks_reaping_without_erroring() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());

        let ClaimOutcome::Granted(g) = board
            .claim("held:key", "dead-owner", Duration::from_secs(0))
            .unwrap()
        else {
            panic!("seed claim must be granted");
        };
        std::mem::forget(g);

        // Another reaper is live in the critical section.
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(board.reap_lock_path("held:key"))
            .unwrap();
        rustix::fs::flock(&held, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();

        // We must back off: Denied (never Err), and the lease is left alone.
        let outcome = board
            .claim("held:key", "agent-b", Duration::from_secs(60))
            .unwrap();
        match outcome {
            ClaimOutcome::Denied { holder } => assert_eq!(holder, "dead-owner"),
            ClaimOutcome::Granted(_) => panic!("must not reap while another reaper holds the lock"),
        }

        // Once that reaper is gone (lock released on close), the key is free.
        drop(held);
        let outcome = board
            .claim("held:key", "agent-b", Duration::from_secs(60))
            .unwrap();
        assert!(matches!(outcome, ClaimOutcome::Granted(_)));
    }

    #[test]
    fn posts_round_trip_in_append_order() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        assert!(board.read_posts().unwrap().is_empty());

        board
            .post(&BoardPost {
                author: "recon".into(),
                ts: "2026-10-01T00:00:00Z".into(),
                kind: PostKind::Observation,
                body: "port 443 open on 1.1.2.2".into(),
                addressed_to: None,
            })
            .unwrap();
        board
            .post(&BoardPost {
                author: "lead".into(),
                ts: "2026-10-01T00:01:00Z".into(),
                kind: PostKind::Note,
                body: "focus on tls".into(),
                addressed_to: Some("recon".into()),
            })
            .unwrap();

        let posts = board.read_posts().unwrap();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].body, "port 443 open on 1.1.2.2");
        assert_eq!(posts[1].addressed_to.as_deref(), Some("recon"));
    }

    #[test]
    fn directives_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        assert!(board.read_directives().unwrap().is_empty());

        board
            .put_directive(&Directive {
                author: "lead".into(),
                ts: "2026-10-01T00:00:00Z".into(),
                body: "budget almost spent; converge and bank".into(),
                addressed_to: None,
            })
            .unwrap();
        let ds = board.read_directives().unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].body, "budget almost spent; converge and bank");
    }
}
