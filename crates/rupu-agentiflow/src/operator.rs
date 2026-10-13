//! The operator's control queue for an agentiflow (spec §17, F1b §3.3):
//! `rupu agentiflow stop` queues a graceful stop and the envelope drains the
//! queue once per round. One JSON file per request under `<root>/steering/`.
//!
//! Messages for the lead don't come through here: they go through the
//! flow's message space (`rupu agentiflow send`, F1b), where the lead hears
//! them before its next model call. A steering message an older binary
//! queued here drains as [`Control::Steering`], for the envelope to re-send
//! through the space.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// One queued file, in the shape every binary has written: a stop
/// (`stop: true`), or before F1b a steering message (`interrupt` was `send
/// --now`; files written before it existed lack it).
#[derive(Debug, Serialize, Deserialize)]
struct QueuedFile {
    ts: String,
    body: String,
    stop: bool,
    #[serde(default)]
    interrupt: bool,
}

/// What the operator asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Wind the flow down at the next round boundary.
    Stop { ts: String },
    /// A message for the lead queued by a binary from before F1b; `urgent`
    /// is its `send --now` mark.
    Steering {
        ts: String,
        body: String,
        urgent: bool,
    },
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

    /// Queue a graceful stop, in the file shape older envelopes also read.
    pub fn request_stop(&self) -> std::io::Result<()> {
        self.enqueue(&QueuedFile {
            ts: chrono::Utc::now().to_rfc3339(),
            body: "operator stop".into(),
            stop: true,
            interrupt: false,
        })
    }

    /// Write one JSON file per request, atomically (temp file, then rename).
    /// Filenames are `<nanos:020>-<pid>-<seq:06>.json`: fixed-width so the
    /// lexical order is arrival order; the pid and counter break ties across
    /// and within processes.
    fn enqueue(&self, file: &QueuedFile) -> std::io::Result<()> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir)?;
        let name = format!(
            "{:020}-{}-{:06}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        );
        // `.tmp` never matches drain's `*.json` filter, so a half-written
        // request is invisible until the rename lands.
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(
            &tmp,
            serde_json::to_vec(file).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, dir.join(format!("{name}.json")))
    }

    /// Read every queued request oldest-first and remove it. A file that
    /// cannot be parsed is skipped (and removed, so it is not re-skipped every
    /// round) rather than failing the drain. A missing directory is empty.
    pub fn drain(&self) -> std::io::Result<Vec<Control>> {
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
            match serde_json::from_slice::<QueuedFile>(&raw) {
                Ok(f) if f.stop => out.push(Control::Stop { ts: f.ts }),
                Ok(f) => out.push(Control::Steering {
                    ts: f.ts,
                    body: f.body,
                    urgent: f.interrupt,
                }),
                Err(err) => {
                    tracing::warn!(path = %path.display(), %err, "skipping an unparseable operator request")
                }
            }
            if let Err(err) = std::fs::remove_file(&path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), %err, "could not remove a drained operator request");
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A steering message as a binary from before F1b queued it.
    pub(crate) fn queue_legacy_steering(root: &Path, body: &str, interrupt: bool) {
        OperatorQueue::new(root)
            .enqueue(&QueuedFile {
                ts: "2026-10-05T00:00:00Z".into(),
                body: body.into(),
                stop: false,
                interrupt,
            })
            .unwrap();
    }

    #[test]
    fn a_stop_drains_once_and_is_what_older_envelopes_read() {
        let tmp = tempfile::tempdir().unwrap();
        let q = OperatorQueue::new(tmp.path());
        q.request_stop().unwrap();
        let file = std::fs::read_dir(tmp.path().join("steering"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!(raw["stop"], true);
        assert_eq!(raw["interrupt"], false);
        assert_eq!(raw["body"], "operator stop");
        let drained = q.drain().unwrap();
        assert!(matches!(drained.as_slice(), [Control::Stop { .. }]));
        assert!(q.drain().unwrap().is_empty());
    }

    #[test]
    fn legacy_steering_drains_in_arrival_order_with_its_urgency() {
        let tmp = tempfile::tempdir().unwrap();
        let q = OperatorQueue::new(tmp.path());
        assert!(q.drain().unwrap().is_empty());
        queue_legacy_steering(tmp.path(), "focus auth", false);
        queue_legacy_steering(tmp.path(), "drop the web leg", true);
        q.request_stop().unwrap();
        let drained = q.drain().unwrap();
        assert_eq!(drained.len(), 3);
        assert_eq!(
            drained[0],
            Control::Steering {
                ts: "2026-10-05T00:00:00Z".into(),
                body: "focus auth".into(),
                urgent: false,
            }
        );
        assert!(matches!(
            &drained[1],
            Control::Steering { body, urgent: true, .. } if body == "drop the web leg"
        ));
        assert!(matches!(drained[2], Control::Stop { .. }));
    }

    #[test]
    fn a_file_written_before_interrupt_existed_drains_as_not_urgent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("steering");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("00000000000000000001-0-000000.json"),
            br#"{"ts":"t","body":"focus auth","stop":false}"#,
        )
        .unwrap();
        assert_eq!(
            OperatorQueue::new(tmp.path()).drain().unwrap(),
            vec![Control::Steering {
                ts: "t".into(),
                body: "focus auth".into(),
                urgent: false,
            }]
        );
    }

    #[test]
    fn torn_file_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let q = OperatorQueue::new(tmp.path());
        q.request_stop().unwrap();
        std::fs::write(
            tmp.path()
                .join("steering")
                .join("00000000000000000000-0-000000.json"),
            b"{not json",
        )
        .unwrap();
        let drained = q.drain().unwrap();
        assert!(matches!(drained.as_slice(), [Control::Stop { .. }]));
        assert!(q.drain().unwrap().is_empty());
    }
}
