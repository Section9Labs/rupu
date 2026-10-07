use crate::error::FleetError;
use crate::types::{BoardPost, ClaimGuard, ClaimOutcome, ClaimRecord, Directive, DirectiveEvent};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
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
    /// expired lease is serialized per key by a `flock` reap lock so a claimant
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
            if let Some(token) = self.try_create_claim(&path, owner, ttl)? {
                return Ok(ClaimOutcome::Granted(ClaimGuard {
                    path,
                    lock_path: self.reap_lock_path(key),
                    token,
                }));
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

    /// `Ok(Some(token))` if the create succeeded (the token is unique to this
    /// grant and recorded in the claim file), `Ok(None)` if a lease already
    /// exists (contention is not an error).
    fn try_create_claim(
        &self,
        path: &Path,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<String>, FleetError> {
        let now = Utc::now();
        let expires =
            now + chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::seconds(0));
        let token = ulid::Ulid::new().to_string();
        let rec = ClaimRecord {
            owner: owner.to_string(),
            acquired_at: now.to_rfc3339(),
            lease_expires_at: expires.to_rfc3339(),
            token: token.clone(),
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
                Ok(Some(token))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
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

    /// Append one lead->fleet directive to `directives.jsonl` and return its id.
    ///
    /// A directive with an empty `id` is given a fresh ULID; one that already
    /// has an id keeps it (writing a live id again replaces that directive).
    /// The id is what [`Board::retract_directive`] takes.
    pub fn put_directive(&self, directive: &Directive) -> Result<String, FleetError> {
        let mut directive = directive.clone();
        if !is_keyed(&directive.id) {
            directive.id = ulid::Ulid::new().to_string();
        }
        let id = directive.id.clone();
        append_jsonl(&self.directives_path(), &DirectiveEvent::Put(directive))?;
        Ok(id)
    }

    /// Retract the directive `id`: append a retraction to `directives.jsonl`.
    ///
    /// The log is append-only, so this lifts the directive from the live set
    /// (what [`Board::read_directives`] returns) without erasing it. Retracting
    /// an id nothing live carries is a harmless no-op on read; a directive
    /// written before ids existed has no id and cannot be retracted.
    pub fn retract_directive(&self, id: &str) -> Result<(), FleetError> {
        append_jsonl(
            &self.directives_path(),
            &DirectiveEvent::Retract {
                retract: id.to_string(),
            },
        )
    }

    /// The LIVE directives, oldest first: the directive log folded in file
    /// order, a put adding (or, for an id already live, replacing in place) a
    /// directive and a retraction removing it. An absent log yields an empty
    /// list.
    ///
    /// Unlike the other logs this one tolerates a corrupt line: it is skipped
    /// and logged, so one bad line cannot silence every standing directive (nor
    /// can it be mistaken for one). Directives with no id (legacy lines) are all
    /// live, none retractable.
    pub fn read_directives(&self) -> Result<Vec<Directive>, FleetError> {
        let path = self.directives_path();
        let text = read_log_text(&path)?;
        // Slot per put, in file order; a retraction empties its slot, so the
        // survivors stay oldest-first without a re-sort.
        let mut slots: Vec<Option<Directive>> = Vec::new();
        let mut slot_of: HashMap<String, usize> = HashMap::new();
        let mut legacy = 0usize;
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<DirectiveEvent>(line) {
                Ok(DirectiveEvent::Put(d)) if !is_keyed(&d.id) => {
                    legacy += 1;
                    slots.push(Some(d));
                }
                Ok(DirectiveEvent::Put(d)) => match slot_of.get(&d.id) {
                    Some(&slot) => slots[slot] = Some(d),
                    None => {
                        slot_of.insert(d.id.clone(), slots.len());
                        slots.push(Some(d));
                    }
                },
                Ok(DirectiveEvent::Retract { retract }) => {
                    if let Some(slot) = slot_of.remove(&retract) {
                        slots[slot] = None;
                    }
                }
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    line = n + 1,
                    error = %e,
                    "skipping an unreadable directive log line"
                ),
            }
        }
        if legacy > 0 {
            tracing::debug!(
                path = %path.display(),
                legacy,
                "directives without an id (written before ids existed) cannot be retracted"
            );
        }
        Ok(slots.into_iter().flatten().collect())
    }
}

/// Whether `id` names a directive. A blank id is "no id": minted over on a put,
/// never matched by a retraction.
fn is_keyed(id: &str) -> bool {
    !id.trim().is_empty()
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

/// The text of an append-only log, lossily decoded. A missing file is an empty
/// log.
fn read_log_text(path: &Path) -> Result<String, FleetError> {
    match std::fs::read(path) {
        Ok(b) => Ok(String::from_utf8_lossy(&b).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(FleetError::Io {
            action: format!("read {}", path.display()),
            source: e,
        }),
    }
}

/// Read every JSON line of `path`, oldest first. A missing file is an empty
/// log; blank lines are skipped; a malformed line is a `Parse` error.
fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, FleetError> {
    let text = read_log_text(path)?;
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
    fn a_stale_guards_drop_does_not_delete_a_successors_claim() {
        let dir = tempfile::tempdir().unwrap();
        let board = Board::new(dir.path());
        // A grants with a 0s lease (immediately expired).
        let a = board
            .claim("host:1.1.2.2", "agent-a", Duration::from_secs(0))
            .unwrap();
        let ClaimOutcome::Granted(a_guard) = a else {
            panic!("A granted")
        };
        // B claims the same key: reaps A's expired lease, re-grants to B.
        let b = board
            .claim("host:1.1.2.2", "agent-b", Duration::from_secs(3600))
            .unwrap();
        assert!(
            matches!(b, ClaimOutcome::Granted(_)),
            "B granted after reaping A"
        );
        // A's guard drops (A's run ends). It must NOT delete B's live claim.
        drop(a_guard);
        assert_eq!(
            board.claim_holder("host:1.1.2.2").unwrap().as_deref(),
            Some("agent-b"),
            "B still holds the key after A's stale guard dropped"
        );
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

    fn directive(body: &str) -> Directive {
        Directive {
            id: String::new(),
            author: "lead".into(),
            ts: "2026-10-01T00:00:00Z".into(),
            body: body.into(),
            addressed_to: None,
        }
    }

    /// The raw lines of `board/directives.jsonl` under `root`.
    fn raw_lines(root: &Path) -> Vec<String> {
        std::fs::read_to_string(root.join("board").join("directives.jsonl"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Write `lines` as the directive log verbatim (a hand-built or legacy log).
    fn write_raw(root: &Path, lines: &[&str]) {
        std::fs::create_dir_all(root.join("board")).unwrap();
        std::fs::write(
            root.join("board").join("directives.jsonl"),
            lines.join("\n") + "\n",
        )
        .unwrap();
    }

    #[test]
    fn directives_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        assert!(board.read_directives().unwrap().is_empty());

        board
            .put_directive(&Directive {
                id: String::new(),
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

    #[test]
    fn put_directive_mints_an_id_and_returns_it() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        let a = board.put_directive(&directive("one")).unwrap();
        let b = board.put_directive(&directive("two")).unwrap();
        assert!(!a.is_empty() && !b.is_empty());
        assert_ne!(a, b, "every directive gets its own id");

        let ds = board.read_directives().unwrap();
        assert_eq!(ds[0].id, a, "the returned id is the stored id");
        assert_eq!(ds[1].id, b);

        // A caller-supplied id is kept, not replaced.
        let mut chosen = directive("three");
        chosen.id = "dir-chosen".into();
        assert_eq!(board.put_directive(&chosen).unwrap(), "dir-chosen");
        assert_eq!(board.read_directives().unwrap()[2].id, "dir-chosen");
    }

    #[test]
    fn retracting_a_directive_removes_it_from_the_live_set() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        let id = board.put_directive(&directive("focus auth")).unwrap();
        board
            .put_directive(&directive("expand to staging"))
            .unwrap();
        assert_eq!(board.read_directives().unwrap().len(), 2);
        let before = raw_lines(tmp.path());

        board.retract_directive(&id).unwrap();

        let live = board.read_directives().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].body, "expand to staging");

        // Append-only: the two directive lines are byte-for-byte untouched and
        // the retraction is one more line after them.
        let after = raw_lines(tmp.path());
        assert_eq!(after.len(), 3, "2 directives + 1 retract: {after:?}");
        assert_eq!(after[..2], before[..]);
        let retract: serde_json::Value = serde_json::from_str(&after[2]).unwrap();
        assert_eq!(retract, serde_json::json!({ "retract": id }));
    }

    #[test]
    fn a_retraction_is_seen_by_a_fresh_handle_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        let id = board.put_directive(&directive("only")).unwrap();
        board.retract_directive(&id).unwrap();
        board.retract_directive(&id).unwrap();
        assert!(Board::new(tmp.path()).read_directives().unwrap().is_empty());
    }

    #[test]
    fn a_retraction_of_an_unknown_id_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        let id = board.put_directive(&directive("stays")).unwrap();
        board.retract_directive("no-such-id").unwrap();
        board.retract_directive("").unwrap();
        let live = board.read_directives().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, id);
    }

    #[test]
    fn live_directives_stay_oldest_first_across_retractions() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        let a = board.put_directive(&directive("a")).unwrap();
        board.put_directive(&directive("b")).unwrap();
        board.put_directive(&directive("c")).unwrap();
        board.retract_directive(&a).unwrap();
        board.put_directive(&directive("d")).unwrap();
        let bodies: Vec<_> = board
            .read_directives()
            .unwrap()
            .into_iter()
            .map(|d| d.body)
            .collect();
        assert_eq!(bodies, ["b", "c", "d"]);
    }

    #[test]
    fn a_put_after_a_retract_of_the_same_id_is_live_again() {
        // The log is folded in file order: the later event wins.
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        let mut d = directive("first wording");
        d.id = "dir-1".into();
        board.put_directive(&d).unwrap();
        board.retract_directive("dir-1").unwrap();
        assert!(board.read_directives().unwrap().is_empty());
        d.body = "second wording".into();
        board.put_directive(&d).unwrap();
        let live = board.read_directives().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].body, "second wording");
    }

    #[test]
    fn a_re_put_of_a_live_id_replaces_it_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let board = Board::new(tmp.path());
        let mut a = directive("a");
        a.id = "dir-a".into();
        board.put_directive(&a).unwrap();
        board.put_directive(&directive("b")).unwrap();
        a.body = "a, reworded".into();
        board.put_directive(&a).unwrap();
        let bodies: Vec<_> = board
            .read_directives()
            .unwrap()
            .into_iter()
            .map(|d| d.body)
            .collect();
        assert_eq!(bodies, ["a, reworded", "b"]);
    }

    #[test]
    fn a_legacy_directive_line_without_an_id_reads_as_a_put() {
        // Written before directives had ids: no `id`, no event wrapper.
        let tmp = tempfile::tempdir().unwrap();
        write_raw(
            tmp.path(),
            &[
                r#"{"author":"operator","ts":"2026-09-01T00:00:00Z","body":"old one"}"#,
                r#"{"author":"lead","ts":"2026-09-01T00:01:00Z","body":"old two","addressed_to":"scanner"}"#,
            ],
        );
        let board = Board::new(tmp.path());
        let ds = board.read_directives().unwrap();
        assert_eq!(ds.len(), 2, "both legacy lines are live, none collapsed");
        assert_eq!(ds[0].body, "old one");
        assert_eq!(ds[0].id, "");
        assert_eq!(ds[1].addressed_to.as_deref(), Some("scanner"));

        // Legacy lines cannot be retracted (no id), and new events fold on top.
        board.retract_directive("").unwrap();
        assert_eq!(board.read_directives().unwrap().len(), 2);
        let id = board.put_directive(&directive("new")).unwrap();
        board.retract_directive(&id).unwrap();
        assert_eq!(board.read_directives().unwrap().len(), 2);
    }

    #[test]
    fn a_corrupt_directive_line_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        write_raw(
            tmp.path(),
            &[
                r#"{"id":"d1","author":"lead","ts":"t","body":"kept"}"#,
                "this is not json",
                r#"{"retract":7}"#,
                "",
                r#"{"id":"d2","author":"lead","ts":"t","body":"also kept"}"#,
            ],
        );
        let ds = Board::new(tmp.path()).read_directives().unwrap();
        let bodies: Vec<_> = ds.iter().map(|d| d.body.as_str()).collect();
        assert_eq!(bodies, ["kept", "also kept"]);
    }

    #[test]
    fn put_and_retract_events_have_unambiguous_wire_shapes() {
        use crate::types::DirectiveEvent;
        // A legacy bare line is a put.
        let legacy: DirectiveEvent =
            serde_json::from_str(r#"{"author":"a","ts":"t","body":"b"}"#).unwrap();
        assert!(matches!(&legacy, DirectiveEvent::Put(d) if d.id.is_empty() && d.body == "b"));
        // A line with an id is a put.
        let put: DirectiveEvent =
            serde_json::from_str(r#"{"id":"x","author":"a","ts":"t","body":"b"}"#).unwrap();
        assert!(matches!(&put, DirectiveEvent::Put(d) if d.id == "x"));
        // A retract line is a retract, never a put.
        let retract: DirectiveEvent = serde_json::from_str(r#"{"retract":"x"}"#).unwrap();
        assert!(matches!(&retract, DirectiveEvent::Retract { retract } if retract == "x"));
        // Neither shape parses as the other: a half-formed line is an error.
        assert!(serde_json::from_str::<DirectiveEvent>(r#"{"id":"x"}"#).is_err());
        assert!(serde_json::from_str::<DirectiveEvent>(r#"{"author":"a"}"#).is_err());

        // What is written round-trips to the same shape: a put serializes as
        // the bare directive (so an older reader still parses it), a retract as
        // `{"retract": id}`.
        let line = serde_json::to_value(DirectiveEvent::Put(directive("x"))).unwrap();
        assert_eq!(line["body"], "x");
        assert!(line.get("retract").is_none());
        let line = serde_json::to_value(DirectiveEvent::Retract {
            retract: "x".into(),
        })
        .unwrap();
        assert_eq!(line, serde_json::json!({ "retract": "x" }));
    }
}
