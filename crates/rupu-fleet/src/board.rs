use crate::error::FleetError;
use crate::types::{ClaimGuard, ClaimOutcome, ClaimRecord};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Upper bound on claim/reap retries under contention (about 1s at
/// `RETRY_INTERVAL`). Exhausting it yields `Denied`, never `Err`.
const CLAIM_ATTEMPTS: u32 = 200;
const RETRY_INTERVAL: Duration = Duration::from_millis(5);
/// A `.reap` mutex older than this was left behind by a crashed reaper.
const REAP_MUTEX_STALE: Duration = Duration::from_secs(30);
/// Longest readable prefix kept in a claim filename.
const STEM_PREFIX_MAX: usize = 64;

#[derive(Debug, Clone)]
pub struct Board {
    pub root: PathBuf,
}

/// Holder of a per-key reap mutex (`<stem>.reap`, created `O_EXCL`). Dropping
/// it releases the mutex.
struct ReapGuard {
    path: PathBuf,
}

impl Drop for ReapGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
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

    fn reap_path(&self, key: &str) -> PathBuf {
        self.claims_dir().join(format!("{}.reap", claim_stem(key)))
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
        use std::io::Write;
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

    /// Try to remove an expired lease under the per-key reap mutex. Returns
    /// `Ok(true)` if this call held the mutex (the caller should retry the
    /// create immediately) and `Ok(false)` if another claimant is reaping (the
    /// caller should back off briefly, then retry).
    ///
    /// The lease is re-read UNDER the mutex and removed only if it is still
    /// expired: a stale read taken before another claimant reaped and
    /// re-granted must never delete that claimant's live lease. A lease that is
    /// already gone is not an error — someone else reaped it.
    fn reap_expired(&self, key: &str, claim_path: &Path) -> Result<bool, FleetError> {
        let Some(_guard) = self.acquire_reap_mutex(key)? else {
            return Ok(false);
        };
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

    /// `O_EXCL`-create the per-key reap mutex. `None` if someone else holds it.
    /// A mutex older than `REAP_MUTEX_STALE` belongs to a crashed reaper and is
    /// cleared so the key cannot stay wedged.
    fn acquire_reap_mutex(&self, key: &str) -> Result<Option<ReapGuard>, FleetError> {
        let path = self.reap_path(key);
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(_) => Ok(Some(ReapGuard { path })),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > REAP_MUTEX_STALE);
                if stale {
                    let _ = std::fs::remove_file(&path);
                }
                Ok(None)
            }
            Err(e) => Err(FleetError::Io {
                action: format!("create reap mutex {}", path.display()),
                source: e,
            }),
        }
    }

    pub fn claim_holder(&self, key: &str) -> Result<Option<String>, FleetError> {
        Ok(read_claim(&self.claim_path(key))?.map(|r| r.owner))
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
    fn stale_reap_mutex_from_a_crashed_reaper_does_not_wedge_the_key() {
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

        // ...and a reap mutex left by a reaper that crashed an hour ago.
        let reap = board.reap_path("wedge:key");
        let f = std::fs::File::create(&reap).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        drop(f);

        let outcome = board
            .claim("wedge:key", "agent-b", Duration::from_secs(60))
            .unwrap();
        assert!(
            matches!(outcome, ClaimOutcome::Granted(_)),
            "a stale reap mutex must be cleared, not wedge the key"
        );
    }
}
