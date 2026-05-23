use crate::ledger::events::{ConcernAssertion, FileTouchEvent, FindingRecord};
use crate::ledger::paths::CoveragePaths;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug)]
enum WriteRequest {
    File(FileTouchEvent),
    Concern(ConcernAssertion),
    Finding(FindingRecord),
    Flush(tokio::sync::oneshot::Sender<()>),
}

#[derive(Debug, Clone)]
pub struct CoverageWriter {
    tx: mpsc::Sender<WriteRequest>,
}

pub struct CoverageWriterHandle {
    pub writer: Arc<CoverageWriter>,
    task: JoinHandle<()>,
}

impl CoverageWriterHandle {
    /// Spawn the async writer task and return a handle.
    pub fn spawn(paths: CoveragePaths) -> std::io::Result<Self> {
        paths.ensure_dir()?;
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let task = tokio::spawn(run_writer(paths, rx));
        Ok(Self {
            writer: Arc::new(CoverageWriter { tx }),
            task,
        })
    }

    /// Block until pending writes have flushed, then shut down the task.
    pub async fn shutdown(self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.writer.tx.send(WriteRequest::Flush(tx)).await;
        let _ = rx.await;
        drop(self.writer);
        let _ = self.task.await;
    }
}

impl CoverageWriter {
    pub async fn record_file_touch(&self, event: FileTouchEvent) {
        let _ = self.tx.send(WriteRequest::File(event)).await;
    }

    pub async fn record_concern(&self, assertion: ConcernAssertion) {
        let _ = self.tx.send(WriteRequest::Concern(assertion)).await;
    }

    pub async fn record_finding(&self, record: FindingRecord) {
        let _ = self.tx.send(WriteRequest::Finding(record)).await;
    }
}

async fn run_writer(paths: CoveragePaths, mut rx: mpsc::Receiver<WriteRequest>) {
    let mut files_f = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.files)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(?e, path = ?paths.files, "open coverage files.jsonl");
            return;
        }
    };
    let mut concerns_f = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.concerns)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(?e, path = ?paths.concerns, "open coverage concerns.jsonl");
            return;
        }
    };
    let mut findings_f = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.findings)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(?e, path = ?paths.findings, "open coverage findings.jsonl");
            return;
        }
    };

    while let Some(req) = rx.recv().await {
        match req {
            WriteRequest::File(ev) => {
                if let Ok(line) = serde_json::to_string(&ev) {
                    let _ = files_f.write_all(line.as_bytes()).await;
                    let _ = files_f.write_all(b"\n").await;
                }
            }
            WriteRequest::Concern(a) => {
                if let Ok(line) = serde_json::to_string(&a) {
                    let _ = concerns_f.write_all(line.as_bytes()).await;
                    let _ = concerns_f.write_all(b"\n").await;
                }
            }
            WriteRequest::Finding(f) => {
                if let Ok(line) = serde_json::to_string(&f) {
                    let _ = findings_f.write_all(line.as_bytes()).await;
                    let _ = findings_f.write_all(b"\n").await;
                }
            }
            WriteRequest::Flush(ack) => {
                let _ = files_f.flush().await;
                let _ = concerns_f.flush().await;
                let _ = findings_f.flush().await;
                let _ = ack.send(());
            }
        }
    }
    let _ = files_f.flush().await;
    let _ = concerns_f.flush().await;
    let _ = findings_f.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, Surface};
    use chrono::Utc;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_test".to_string(),
            model: "mock".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[tokio::test]
    async fn writer_persists_many_file_events() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "test-target");
        let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();

        for i in 0..50 {
            handle
                .writer
                .record_file_touch(FileTouchEvent::Read {
                    path: format!("file{i}.rs"),
                    line_range: [1, (i + 1) as u32 * 10],
                    tool: "read_file".to_string(),
                    attribution: attribution(),
                    at: Utc::now(),
                })
                .await;
        }
        handle.shutdown().await;

        let contents = tokio::fs::read_to_string(&paths.files).await.unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 50);
        for line in lines {
            let _: FileTouchEvent = serde_json::from_str(line).unwrap();
        }
    }
}
