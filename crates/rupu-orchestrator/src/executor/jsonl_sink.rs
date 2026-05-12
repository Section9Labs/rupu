//! JsonlSink — appends serialized events to <run_dir>/events.jsonl,
//! one JSON line per event. Append-only, never rotated. fsync on
//! drop. Write failures log a warning but never propagate.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::warn;

use crate::executor::sink::EventSink;
use crate::executor::Event;

pub struct JsonlSink {
    path: PathBuf,
    file: Mutex<File>,
}

impl JsonlSink {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EventSink for JsonlSink {
    fn emit(&self, _run_id: &str, ev: &Event) {
        let line = match serde_json::to_string(ev) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "JsonlSink: failed to serialize event");
                return;
            }
        };
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!(error = %e, "JsonlSink: file mutex poisoned");
                return;
            }
        };
        if let Err(e) = writeln!(*guard, "{line}") {
            warn!(error = %e, path = %self.path.display(), "JsonlSink: append failed");
        }
    }
}

impl Drop for JsonlSink {
    fn drop(&mut self) {
        if let Ok(guard) = self.file.lock() {
            let _ = guard.sync_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Event;
    use crate::runs::StepKind;

    #[test]
    fn writes_each_event_as_one_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let sink = JsonlSink::create(&path).expect("create");

        sink.emit(
            "r",
            &Event::StepStarted {
                run_id: "r".into(),
                step_id: "s1".into(),
                kind: StepKind::Linear,
                agent: None,
            },
        );
        sink.emit(
            "r",
            &Event::StepCompleted {
                run_id: "r".into(),
                step_id: "s1".into(),
                success: true,
                duration_ms: 17,
            },
        );
        drop(sink);

        let body = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("step_started"));
        assert!(lines[1].contains("step_completed"));
    }

    #[test]
    fn round_trips_through_serde() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let sink = JsonlSink::create(&path).expect("create");
        sink.emit(
            "r",
            &Event::StepStarted {
                run_id: "r".into(),
                step_id: "s1".into(),
                kind: StepKind::Linear,
                agent: Some("classifier".into()),
            },
        );
        drop(sink);
        let body = std::fs::read_to_string(&path).unwrap();
        let ev: Event = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        match ev {
            Event::StepStarted { step_id, agent, .. } => {
                assert_eq!(step_id, "s1");
                assert_eq!(agent.as_deref(), Some("classifier"));
            }
            _ => panic!("expected StepStarted"),
        }
    }
}
