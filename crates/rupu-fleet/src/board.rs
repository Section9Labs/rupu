use crate::error::FleetError;
use crate::types::{ClaimGuard, ClaimOutcome, ClaimRecord};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
        self.claims_dir().join(format!("{}.json", sanitize(key)))
    }

    /// Atomically claim a work unit. Returns `Granted` with an RAII guard, or
    /// `Denied` with the current holder. A claim whose lease has expired is
    /// reaped and re-granted. Mirrors `AutoflowClaimStore::try_acquire_active_lock`.
    pub fn claim(&self, key: &str, owner: &str, ttl: Duration) -> Result<ClaimOutcome, FleetError> {
        let path = self.claim_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FleetError::Io {
                action: format!("create claims dir {}", parent.display()),
                source: e,
            })?;
        }
        match self.try_create_claim(&path, owner, ttl) {
            Ok(()) => Ok(ClaimOutcome::Granted(ClaimGuard { path })),
            Err(FleetError::Claimed { .. }) => {
                if self.reap_if_expired(&path)? {
                    // lease was expired and removed; retry once
                    match self.try_create_claim(&path, owner, ttl) {
                        Ok(()) => Ok(ClaimOutcome::Granted(ClaimGuard { path })),
                        Err(FleetError::Claimed { holder, .. }) => {
                            Ok(ClaimOutcome::Denied { holder })
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    let holder = self.claim_holder(key)?.unwrap_or_default();
                    Ok(ClaimOutcome::Denied { holder })
                }
            }
            Err(e) => Err(e),
        }
    }

    fn try_create_claim(&self, path: &Path, owner: &str, ttl: Duration) -> Result<(), FleetError> {
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
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let holder = read_claim(path)?.map(|r| r.owner).unwrap_or_default();
                Err(FleetError::Claimed {
                    key: path.display().to_string(),
                    holder,
                })
            }
            Err(e) => Err(FleetError::Io {
                action: format!("create claim {}", path.display()),
                source: e,
            }),
        }
    }

    fn reap_if_expired(&self, path: &Path) -> Result<bool, FleetError> {
        let Some(rec) = read_claim(path)? else {
            return Ok(false);
        };
        let expired = chrono::DateTime::parse_from_rfc3339(&rec.lease_expires_at)
            .map(|t| t.with_timezone(&Utc) <= Utc::now())
            .unwrap_or(false);
        if expired {
            std::fs::remove_file(path).map_err(|e| FleetError::Io {
                action: format!("reap claim {}", path.display()),
                source: e,
            })?;
        }
        Ok(expired)
    }

    pub fn claim_holder(&self, key: &str) -> Result<Option<String>, FleetError> {
        Ok(read_claim(&self.claim_path(key))?.map(|r| r.owner))
    }
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

/// Map a work-unit key to a safe filename component.
fn sanitize(key: &str) -> String {
    key.chars()
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
}
