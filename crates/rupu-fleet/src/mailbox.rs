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

    pub fn send(&self, to: &str, msg: &FleetMessage, cap: usize) -> Result<(), FleetError> {
        let path = self.inbox_path(to);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FleetError::Io {
                action: format!("create inbox dir {}", parent.display()),
                source: e,
            })?;
        }
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
}
