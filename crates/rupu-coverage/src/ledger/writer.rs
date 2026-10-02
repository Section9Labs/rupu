use crate::ledger::events::FileTouchEvent;
use crate::ledger::paths::CoveragePaths;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug)]
enum WriteRequest {
    // Boxed: `Attribution` grew `agent`/`provider`, tipping clippy's
    // large_enum_variant against the 8-byte `Flush`.
    File(Box<FileTouchEvent>),
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
        let _ = self.tx.send(WriteRequest::File(Box::new(event))).await;
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

    while let Some(req) = rx.recv().await {
        match req {
            WriteRequest::File(ev) => persist_file_event(&mut files_f, &paths, &ev).await,
            WriteRequest::Flush(ack) => {
                flush_logged(&mut files_f, &paths.files).await;
                let _ = ack.send(());
            }
        }
    }
    flush_logged(&mut files_f, &paths.files).await;
}

/// Flush the ledger handle; a failure is logged, since the writer is a
/// detached task with no caller to return it to.
async fn flush_logged<W: AsyncWrite + Unpin>(ledger: &mut W, path: &std::path::Path) {
    if let Err(e) = ledger.flush().await {
        tracing::error!(?e, ?path, "flush coverage files.jsonl");
    }
}

/// Write one file-touch line to the ledger handle, then mirror it into the
/// run stream — and only once the line was written AND flushed, the same
/// order `append_record` keeps: a stream line for a record the ledger lost
/// would show a coordinator a touch this host's own coverage never had.
///
/// The flush is what makes that guard real. A `tokio::fs::File` accepts a
/// write into its buffer and reports the underlying I/O error on a LATER
/// write or flush, so `write_all` alone would stream the very line whose
/// write then failed. File touches are per tool call, not a hot path, so
/// flushing each line costs nothing that matters. A failure is logged (the
/// writer is a detached task; there is no caller to return it to) and the
/// line is not streamed.
async fn persist_file_event<W: AsyncWrite + Unpin>(
    ledger: &mut W,
    paths: &CoveragePaths,
    ev: &FileTouchEvent,
) {
    let mut line = match serde_json::to_string(ev) {
        Ok(line) => line,
        Err(e) => {
            tracing::error!(?e, "serialize coverage file-touch event");
            return;
        }
    };
    line.push('\n');
    if let Err(e) = write_and_flush(ledger, line.as_bytes()).await {
        tracing::error!(?e, path = ?paths.files, "write coverage files.jsonl; line not streamed");
        return;
    }
    line.pop();
    crate::ledger::stream::stream_json(paths, crate::ledger::stream::Ledger::Files, &line);
}

async fn write_and_flush<W: AsyncWrite + Unpin>(
    ledger: &mut W,
    bytes: &[u8],
) -> std::io::Result<()> {
    ledger.write_all(bytes).await?;
    ledger.flush().await
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
            codename: None,
            agent: None,
            provider: None,
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

    #[tokio::test]
    async fn writer_mirrors_file_events_into_the_run_stream() {
        use crate::ledger::stream::{RunStream, StreamLine};

        let tmp = tempfile::TempDir::new().unwrap();
        let stream = tmp.path().join("runs/run_test/coverage.jsonl");
        let paths =
            CoveragePaths::new(tmp.path(), "test-target").with_run_stream(Some(RunStream {
                path: stream.clone(),
                scope_name: "sec".to_string(),
            }));
        let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();
        handle
            .writer
            .record_file_touch(FileTouchEvent::Read {
                path: "a.rs".to_string(),
                line_range: [1, 10],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            })
            .await;
        handle.shutdown().await;

        let ledger = tokio::fs::read_to_string(&paths.files).await.unwrap();
        assert_eq!(ledger.lines().count(), 1);
        let streamed = tokio::fs::read_to_string(&stream).await.unwrap();
        let lines: Vec<&str> = streamed.lines().collect();
        assert_eq!(lines.len(), 1);
        match serde_json::from_str::<StreamLine>(lines[0]).unwrap() {
            StreamLine::Files { scope_name, record } => {
                assert_eq!(scope_name, "sec");
                assert_eq!(serde_json::to_string(&record).unwrap(), ledger.trim_end());
            }
            other => panic!("expected a files line, got {other:?}"),
        }
    }

    /// Which operation on the ledger handle fails.
    #[derive(Clone, Copy)]
    enum Fails {
        Write,
        Flush,
    }

    /// A ledger handle that fails one operation. `Flush` models a
    /// `tokio::fs::File`, whose `write_all` is accepted into its buffer and
    /// whose real I/O error only surfaces on the flush.
    struct BrokenLedger(Fails);

    impl AsyncWrite for BrokenLedger {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(match self.0 {
                Fails::Write => Err(std::io::Error::other("disk full")),
                Fails::Flush => Ok(buf.len()),
            })
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(match self.0 {
                Fails::Write => Ok(()),
                Fails::Flush => Err(std::io::Error::other("disk full")),
            })
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    fn read_event(path: &str) -> FileTouchEvent {
        FileTouchEvent::Read {
            path: path.to_string(),
            line_range: [1, 10],
            tool: "read_file".to_string(),
            attribution: attribution(),
            at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn a_failed_ledger_write_or_flush_is_not_streamed() {
        use crate::ledger::stream::RunStream;

        for fails in [Fails::Write, Fails::Flush] {
            let tmp = tempfile::TempDir::new().unwrap();
            let stream = tmp.path().join("runs/run_test/coverage.jsonl");
            let paths =
                CoveragePaths::new(tmp.path(), "test-target").with_run_stream(Some(RunStream {
                    path: stream.clone(),
                    scope_name: "sec".to_string(),
                }));

            persist_file_event(&mut BrokenLedger(fails), &paths, &read_event("a.rs")).await;
            assert!(
                !stream.exists(),
                "a record the ledger lost must not reach the stream"
            );

            // The healthy path still streams, to a ledger that took the line.
            let mut ledger = Vec::new();
            persist_file_event(&mut ledger, &paths, &read_event("b.rs")).await;
            assert_eq!(String::from_utf8(ledger).unwrap().lines().count(), 1);
            assert_eq!(std::fs::read_to_string(&stream).unwrap().lines().count(), 1);
        }
    }
}
