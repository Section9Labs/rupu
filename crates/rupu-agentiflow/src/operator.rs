//! File-backed operator -> lead steering channel (spec §17), realized without
//! the session worker. `rupu agentiflow send` enqueues; the envelope drains
//! once per round. One JSON file per message under `<root>/steering/`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// One operator steering message. `stop` asks the envelope to wind down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorMessage {
    pub ts: String,
    pub body: String,
    pub stop: bool,
}

/// Process-local tiebreaker so two enqueues in the same nanosecond never collide.
static SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct OperatorQueue {
    root: PathBuf,
}

impl OperatorQueue {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn dir(&self) -> PathBuf {
        self.root.join("steering")
    }

    /// Write one JSON file per message, atomically (temp file, then rename).
    /// Filenames are `<nanos:020>-<pid>-<seq:06>.json`: fixed-width so the
    /// lexical order is arrival order; the pid and counter break ties across
    /// and within processes.
    pub fn enqueue(&self, msg: &OperatorMessage) -> std::io::Result<()> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir)?;
        let name = format!(
            "{:020}-{}-{:06}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        );
        // `.tmp` never matches drain's `*.json` filter, so a half-written
        // message is invisible until the rename lands.
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(
            &tmp,
            serde_json::to_vec(msg).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, dir.join(format!("{name}.json")))
    }

    /// Read every queued message oldest-first and remove it. A file that
    /// cannot be parsed is skipped (and removed, so it is not re-skipped every
    /// round) rather than failing the drain. A missing directory is empty.
    pub fn drain(&self) -> std::io::Result<Vec<OperatorMessage>> {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(self.dir()) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        files.sort();
        let mut out = Vec::with_capacity(files.len());
        for path in files {
            let Ok(raw) = std::fs::read(&path) else {
                continue; // vanished under a concurrent drain, or unreadable: leave it
            };
            match serde_json::from_slice::<OperatorMessage>(&raw) {
                Ok(msg) => out.push(msg),
                Err(err) => {
                    tracing::warn!(path = %path.display(), %err, "skipping unparseable steering message")
                }
            }
            if let Err(err) = std::fs::remove_file(&path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), %err, "could not remove drained steering message");
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_then_drain_once_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let q = OperatorQueue::new(tmp.path());
        q.enqueue(&OperatorMessage {
            ts: "t1".into(),
            body: "focus auth".into(),
            stop: false,
        })
        .unwrap();
        q.enqueue(&OperatorMessage {
            ts: "t2".into(),
            body: "wrap up".into(),
            stop: true,
        })
        .unwrap();
        let msgs = q.drain().unwrap();
        assert_eq!(msgs.len(), 2);
        assert!(msgs.iter().any(|m| m.stop));
        assert!(q.drain().unwrap().is_empty());
    }

    #[test]
    fn missing_dir_is_empty_and_order_is_arrival_order() {
        let tmp = tempfile::tempdir().unwrap();
        let q = OperatorQueue::new(tmp.path());
        assert!(q.drain().unwrap().is_empty());
        for i in 0..20 {
            q.enqueue(&OperatorMessage {
                ts: i.to_string(),
                body: String::new(),
                stop: false,
            })
            .unwrap();
        }
        let ts: Vec<String> = q.drain().unwrap().into_iter().map(|m| m.ts).collect();
        assert_eq!(ts, (0..20).map(|i| i.to_string()).collect::<Vec<_>>());
    }

    #[test]
    fn torn_file_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let q = OperatorQueue::new(tmp.path());
        q.enqueue(&OperatorMessage {
            ts: "ok".into(),
            body: "b".into(),
            stop: false,
        })
        .unwrap();
        std::fs::write(
            tmp.path()
                .join("steering")
                .join("00000000000000000000-0-000000.json"),
            b"{not json",
        )
        .unwrap();
        let msgs = q.drain().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].ts, "ok");
        assert!(q.drain().unwrap().is_empty());
    }
}
