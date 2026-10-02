//! [`ObjectStoreBucket`] — object_store-backed implementation of the [`Bucket`] port.
//!
//! ## Atomic claim
//! `claim_job` uses `ObjectStore::put_opts` with `PutMode::Create`, which maps to
//! a conditional PUT (if-none-match: *) on S3/GCS.  The first caller gets `Ok(true)`;
//! any subsequent caller that finds the object already present receives
//! `object_store::Error::AlreadyExists` which we convert to `Ok(false)`.
//!
//! ## Backends
//! - Production: construct via `from_url` (delegates to `object_store::parse_url_opts`).
//! - Tests: construct via `new(Arc::new(InMemory::new()), prefix)` — no cloud required.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::TryStreamExt;
use object_store::{
    path::Path, ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload,
};

use super::{
    BucketError, Bucket,
    key_artifact, key_claim, key_control, key_finished, key_job, key_result, key_worker_info,
    prefix_control, prefix_results,
};

// ── struct ────────────────────────────────────────────────────────────────────

pub struct ObjectStoreBucket {
    store: Arc<dyn ObjectStore>,
    prefix: Path,
}

impl ObjectStoreBucket {
    /// Create from an already-constructed store (e.g. `InMemory` in tests).
    pub fn new(store: Arc<dyn ObjectStore>, prefix: &str) -> Self {
        let prefix = Path::parse(prefix).unwrap_or_else(|_| Path::from(prefix));
        Self { store, prefix }
    }

    /// Create from a URL in production; credentials are resolved by `object_store`
    /// via environment variables / instance metadata.
    pub fn from_url(url: &str, prefix: Option<&str>) -> Result<Self, BucketError> {
        let parsed = url::Url::parse(url)
            .map_err(|e| BucketError::Io(format!("invalid bucket url: {e}")))?;
        // Pass environment variables so object_store picks up the standard
        // credential env vars: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY /
        // GOOGLE_SERVICE_ACCOUNT_KEY / etc.  parse_url_opts accepts
        // IntoIterator<Item=(K: AsRef<str>, V: Into<String>)> and (String,String)
        // satisfies both bounds.  For file:// and memory:// backends the env
        // vars are silently ignored, so existing tests remain unaffected.
        let (store, url_path) = object_store::parse_url_opts(&parsed, std::env::vars())
            .map_err(|e| BucketError::Io(e.to_string()))?;
        let full_prefix = match prefix {
            Some(p) if !p.is_empty() => url_path.join(p),
            _ => url_path,
        };
        Ok(Self {
            store: Arc::from(store),
            prefix: full_prefix,
        })
    }

    // ── internal helpers ──────────────────────────────────────────────────────

    /// Resolve a relative key string against `self.prefix`.
    fn path(&self, relative: &str) -> Path {
        // Split on '/' and chain child calls so the path library handles
        // normalisation correctly.
        // Build path by joining each non-empty segment.
        let joined = self.prefix.as_ref().to_string() + "/" + relative;
        Path::parse(&joined)
            .expect("BUG: bucket key-layout helpers must produce valid object_store Paths")
    }

    /// Simple put — overwrites any existing object.
    async fn put_bytes(&self, path: &Path, body: &[u8]) -> Result<(), BucketError> {
        let payload: PutPayload = Bytes::copy_from_slice(body).into();
        self.store
            .put(path, payload)
            .await
            .map_err(|e| BucketError::Io(e.to_string()))?;
        Ok(())
    }

    /// Fetch raw bytes for `path`, mapping a not-found error to `BucketError::NotFound`.
    async fn get_bytes(&self, path: &Path) -> Result<Option<Vec<u8>>, BucketError> {
        match self.store.get(path).await {
            Ok(result) => {
                let bytes = result
                    .bytes()
                    .await
                    .map_err(|e| BucketError::Io(e.to_string()))?;
                Ok(Some(bytes.to_vec()))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(BucketError::Io(e.to_string())),
        }
    }

    /// Return `true` if `path` exists in the store.
    async fn exists(&self, path: &Path) -> Result<bool, BucketError> {
        match self.store.head(path).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(BucketError::Io(e.to_string())),
        }
    }

    /// List all objects under `dir_path` and collect into a `Vec<object_store::ObjectMeta>`.
    async fn list_all(
        &self,
        dir_path: &Path,
    ) -> Result<Vec<object_store::ObjectMeta>, BucketError> {
        let stream = self.store.list(Some(dir_path));
        stream
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| BucketError::Io(e.to_string()))
    }
}

// ── trait impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl Bucket for ObjectStoreBucket {
    async fn put_job(&self, run_id: &str, envelope: &[u8]) -> Result<(), BucketError> {
        let path = self.path(&key_job(run_id));
        self.put_bytes(&path, envelope).await
    }

    async fn list_jobs(&self) -> Result<Vec<String>, BucketError> {
        let jobs_dir = self.path("jobs");
        let metas = self.list_all(&jobs_dir).await?;

        // Collect all stems that end with ".json" (these are the job envelopes).
        // Then exclude any whose claim file exists.
        let mut run_ids: Vec<String> = metas
            .iter()
            .filter_map(|m| {
                let name = m.location.filename()?;
                name.strip_suffix(".json").map(|id| id.to_string())
            })
            .collect();

        // Remove claimed jobs.
        let mut unclaimed = Vec::with_capacity(run_ids.len());
        for run_id in run_ids.drain(..) {
            let claim_path = self.path(&key_claim(&run_id));
            if !self.exists(&claim_path).await? {
                unclaimed.push(run_id);
            }
        }
        Ok(unclaimed)
    }

    async fn claim_job(&self, run_id: &str, worker: &str) -> Result<bool, BucketError> {
        let claim_path = self.path(&key_claim(run_id));
        let payload: PutPayload = Bytes::copy_from_slice(worker.as_bytes()).into();
        let opts = PutOptions {
            mode: PutMode::Create,
            ..Default::default()
        };
        match self.store.put_opts(&claim_path, payload, opts).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::AlreadyExists { .. }) => Ok(false),
            Err(e) => Err(BucketError::Io(e.to_string())),
        }
    }

    async fn get_job(&self, run_id: &str) -> Result<Vec<u8>, BucketError> {
        let path = self.path(&key_job(run_id));
        match self.get_bytes(&path).await? {
            Some(b) => Ok(b),
            None => Err(BucketError::NotFound(format!("job {run_id}"))),
        }
    }

    async fn put_control(&self, run_id: &str, seq: u64, envelope: &[u8]) -> Result<(), BucketError> {
        let path = self.path(&key_control(run_id, seq));
        self.put_bytes(&path, envelope).await
    }

    async fn list_control(&self, run_id: &str) -> Result<Vec<(u64, Vec<u8>)>, BucketError> {
        let dir = self.path(&prefix_control(run_id));
        let mut metas = self.list_all(&dir).await?;

        // Sort by filename (lexical == numeric due to zero-padding).
        metas.sort_by(|a, b| a.location.as_ref().cmp(b.location.as_ref()));

        let mut result = Vec::with_capacity(metas.len());
        for meta in metas {
            let name = meta
                .location
                .filename()
                .ok_or_else(|| BucketError::Io("control path has no filename".into()))?;
            // Filename format: <seq:020>.json
            let seq_str = name
                .strip_suffix(".json")
                .ok_or_else(|| BucketError::Io(format!("unexpected control filename: {name}")))?;
            let seq: u64 = seq_str
                .parse()
                .map_err(|_| BucketError::Io(format!("non-numeric seq in {name}")))?;
            let bytes = self
                .get_bytes(&meta.location)
                .await?
                .ok_or_else(|| BucketError::Io(format!("control object disappeared: {name}")))?;
            result.push((seq, bytes));
        }
        Ok(result)
    }

    async fn put_result(&self, run_id: &str, key: &str, body: &[u8]) -> Result<(), BucketError> {
        let path = self.path(&key_result(run_id, key));
        self.put_bytes(&path, body).await
    }

    async fn list_results(&self, run_id: &str) -> Result<Vec<(String, Vec<u8>)>, BucketError> {
        let dir = self.path(&prefix_results(run_id));
        let mut metas = self.list_all(&dir).await?;

        // Sort by full location path (key-ascending).
        metas.sort_by(|a, b| a.location.as_ref().cmp(b.location.as_ref()));

        // Exclude the finished marker from results.
        let finished_path = self.path(&key_finished(run_id));

        let mut results = Vec::with_capacity(metas.len());
        for meta in metas {
            if meta.location == finished_path {
                continue;
            }
            let key = meta
                .location
                .filename()
                .ok_or_else(|| BucketError::Io("result path has no filename".into()))?
                .to_string();
            let bytes = self
                .get_bytes(&meta.location)
                .await?
                .ok_or_else(|| BucketError::Io(format!("result object disappeared: {key}")))?;
            results.push((key, bytes));
        }
        Ok(results)
    }

    async fn put_finished(&self, run_id: &str, status: &str) -> Result<(), BucketError> {
        let path = self.path(&key_finished(run_id));
        self.put_bytes(&path, status.as_bytes()).await
    }

    async fn get_finished(&self, run_id: &str) -> Result<Option<String>, BucketError> {
        let path = self.path(&key_finished(run_id));
        match self.get_bytes(&path).await? {
            Some(b) => {
                let s = String::from_utf8(b)
                    .map_err(|e| BucketError::Io(format!("finished marker not utf-8: {e}")))?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    async fn probe(&self) -> Result<(), BucketError> {
        self.store
            .list_with_delimiter(Some(&self.prefix))
            .await
            .map_err(|e| BucketError::Io(e.to_string()))?;
        Ok(())
    }

    async fn put_worker_info(&self, worker_id: &str, body: &[u8]) -> Result<(), BucketError> {
        let path = self.path(&key_worker_info(worker_id));
        self.put_bytes(&path, body).await
    }

    async fn list_worker_info(&self) -> Result<Vec<Vec<u8>>, BucketError> {
        let mut metas = self.list_all(&self.path("nodes")).await?;
        metas.sort_by(|a, b| a.location.cmp(&b.location));
        let mut out = Vec::with_capacity(metas.len());
        for m in metas {
            if m.location.filename().is_some_and(|f| f.ends_with(".json")) {
                if let Some(body) = self.get_bytes(&m.location).await? {
                    out.push(body);
                }
            }
        }
        Ok(out)
    }

    async fn artifact_exists(&self, sha256: &str) -> Result<bool, BucketError> {
        self.exists(&self.path(&key_artifact(sha256))).await
    }

    async fn put_artifact_file(
        &self,
        sha256: &str,
        src: &std::path::Path,
    ) -> Result<(), BucketError> {
        use tokio::io::AsyncReadExt;
        let io = |e: std::io::Error| BucketError::Io(format!("reading artifact {sha256}: {e}"));
        // Open BEFORE starting the upload, so a source that can't be read
        // leaves nothing half-created in the bucket. The store is a
        // directory the run's agents can influence: a FIFO swapped in for a
        // blob must not park the worker's drain loop on the open, so this
        // refuses anything but a regular file (the tunnel's pull opens its
        // blobs the same way).
        let owned = src.to_path_buf();
        let file =
            tokio::task::spawn_blocking(move || crate::api::fs_open::open_regular_file(&owned))
                .await
                .map_err(|e| BucketError::Io(format!("opening artifact {sha256}: {e}")))?
                .map_err(io)?;
        let mut f = tokio::fs::File::from_std(file);

        let path = self.path(&key_artifact(sha256));
        let upload = self
            .store
            .put_multipart(&path)
            .await
            .map_err(|e| BucketError::Io(e.to_string()))?;
        let mut w = object_store::WriteMultipart::new(upload);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = match f.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    // Don't leave an unfinished multipart upload behind.
                    let _ = w.abort().await;
                    return Err(io(e));
                }
            };
            if n == 0 {
                break;
            }
            w.write(&buf[..n]);
        }
        w.finish()
            .await
            .map_err(|e| BucketError::Io(e.to_string()))?;
        Ok(())
    }

    async fn get_artifact_to_file(
        &self,
        sha256: &str,
        dest: &std::path::Path,
        max_bytes: u64,
    ) -> Result<(), BucketError> {
        use futures_util::StreamExt as _;
        use tokio::io::AsyncWriteExt;
        let io = |e: std::io::Error| BucketError::Io(format!("local write failed: {e}"));
        let too_big = || {
            BucketError::Io(format!(
                "artifact {sha256} exceeds its recorded {max_bytes} bytes"
            ))
        };
        let path = self.path(&key_artifact(sha256));
        let got = match self.store.get(&path).await {
            Ok(g) => g,
            Err(object_store::Error::NotFound { .. }) => {
                return Err(BucketError::NotFound(key_artifact(sha256)))
            }
            Err(e) => return Err(BucketError::Io(e.to_string())),
        };
        // A stored size over the cap is refused before anything is written;
        // the stream below is still held to the cap as it arrives.
        if got.meta.size > max_bytes {
            return Err(too_big());
        }
        let mut stream = got.into_stream();
        let mut file = tokio::fs::File::create(dest).await.map_err(io)?;
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| BucketError::Io(e.to_string()))?;
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err(too_big());
            }
            file.write_all(&chunk).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        Ok(())
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_bucket() -> ObjectStoreBucket {
        ObjectStoreBucket::new(
            Arc::new(object_store::memory::InMemory::new()),
            "test-prefix/host_1",
        )
    }

    #[tokio::test]
    async fn job_put_list_get_roundtrip() {
        let b = mem_bucket();
        b.put_job("run_1", br#"{"kind":"workflow"}"#).await.unwrap();
        assert_eq!(b.list_jobs().await.unwrap(), vec!["run_1".to_string()]);
        assert_eq!(b.get_job("run_1").await.unwrap(), br#"{"kind":"workflow"}"#);
    }

    #[tokio::test]
    async fn claim_is_atomic_once() {
        let b = mem_bucket();
        b.put_job("run_1", b"{}").await.unwrap();
        assert!(b.claim_job("run_1", "node-a").await.unwrap()); // first wins
        assert!(!b.claim_job("run_1", "node-b").await.unwrap()); // second loses
    }

    #[tokio::test]
    async fn control_and_results_ordered_by_seq_key() {
        let b = mem_bucket();
        b.put_control("run_1", 2, b"c2").await.unwrap();
        b.put_control("run_1", 1, b"c1").await.unwrap();
        let ctl = b.list_control("run_1").await.unwrap();
        assert_eq!(
            ctl.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![1, 2]
        );
        b.put_result("run_1", "events.0001.jsonl", b"line")
            .await
            .unwrap();
        assert_eq!(b.list_results("run_1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn worker_info_put_list_roundtrip() {
        let b = mem_bucket();
        assert!(b.list_worker_info().await.unwrap().is_empty());
        b.put_worker_info("node_b", b"{\"b\":1}").await.unwrap();
        b.put_worker_info("node_a", b"{\"a\":1}").await.unwrap();
        // Overwrite, not append.
        b.put_worker_info("node_a", b"{\"a\":2}").await.unwrap();
        assert_eq!(
            b.list_worker_info().await.unwrap(),
            vec![b"{\"a\":2}".to_vec(), b"{\"b\":1}".to_vec()]
        );
        // Worker markers are not jobs.
        assert!(b.list_jobs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn artifact_put_exists_get_roundtrip_and_cap() {
        let b = mem_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("blob");
        std::fs::write(&src, b"artifact!").unwrap();
        let sha = "ab".repeat(32);
        assert!(!b.artifact_exists(&sha).await.unwrap());
        b.put_artifact_file(&sha, &src).await.unwrap();
        assert!(b.artifact_exists(&sha).await.unwrap());
        let dest = tmp.path().join("out");
        b.get_artifact_to_file(&sha, &dest, 9).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"artifact!");
        assert!(b.get_artifact_to_file(&sha, &dest, 8).await.is_err());
        assert!(matches!(
            b.get_artifact_to_file(&"cd".repeat(32), &dest, 9).await,
            Err(BucketError::NotFound(_))
        ));
    }

    /// A blob bigger than one read buffer round-trips byte-for-byte (the
    /// upload is streamed in pieces, not read whole).
    #[tokio::test]
    async fn artifact_larger_than_one_buffer_roundtrips() {
        let b = mem_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let body: Vec<u8> = (0..(3 << 20) + 17).map(|i| (i % 251) as u8).collect();
        let src = tmp.path().join("big");
        std::fs::write(&src, &body).unwrap();
        let sha = "12".repeat(32);
        b.put_artifact_file(&sha, &src).await.unwrap();
        let dest = tmp.path().join("big.out");
        b.get_artifact_to_file(&sha, &dest, body.len() as u64)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // Exactly one byte under the real size is over the cap.
        assert!(b
            .get_artifact_to_file(&sha, &dest, body.len() as u64 - 1)
            .await
            .is_err());
    }

    /// Artifacts live under `artifacts/`, not under jobs/results/nodes, so
    /// they don't show up in any of the run-scoped listings.
    #[tokio::test]
    async fn artifacts_do_not_pollute_other_listings() {
        let b = mem_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("blob");
        std::fs::write(&src, b"x").unwrap();
        b.put_artifact_file(&"ab".repeat(32), &src).await.unwrap();
        assert!(b.list_jobs().await.unwrap().is_empty());
        assert!(b.list_worker_info().await.unwrap().is_empty());
        assert!(b.list_results("run_1").await.unwrap().is_empty());
    }

    /// The upload opens its source without ever parking the worker: a FIFO
    /// where a blob should be is refused, and no upload is left behind.
    #[cfg(unix)]
    #[tokio::test]
    async fn artifact_upload_refuses_a_fifo_source_without_blocking() {
        let b = mem_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("fifo");
        if !crate::api::fs_open::test_support::mkfifo(&fifo) {
            return;
        }
        let sha = "fe".repeat(32);
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            b.put_artifact_file(&sha, &fifo),
        )
        .await
        .expect("opening a FIFO blocked the upload");
        assert!(matches!(res, Err(BucketError::Io(_))), "{res:?}");
        assert!(!b.artifact_exists(&sha).await.unwrap());
    }

    /// A source that isn't there is an error, not an empty upload.
    #[tokio::test]
    async fn artifact_upload_of_a_missing_source_fails_and_uploads_nothing() {
        let b = mem_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let sha = "ee".repeat(32);
        assert!(b
            .put_artifact_file(&sha, &tmp.path().join("absent"))
            .await
            .is_err());
        assert!(!b.artifact_exists(&sha).await.unwrap());
    }

    #[tokio::test]
    async fn finished_marker_roundtrip() {
        let b = mem_bucket();
        assert_eq!(b.get_finished("run_1").await.unwrap(), None);
        b.put_finished("run_1", "completed").await.unwrap();
        assert_eq!(
            b.get_finished("run_1").await.unwrap().as_deref(),
            Some("completed")
        );
    }
}
