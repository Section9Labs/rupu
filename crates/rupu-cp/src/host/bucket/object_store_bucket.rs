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

// ── artifact transfer tuning ──────────────────────────────────────────────────

/// Size of one multipart upload part (S3's minimum for all but the last).
const ARTIFACT_PART_BYTES: usize = 5 << 20;

/// Upload parts in flight at once. Reading local disk outruns any uplink, so
/// without a bound the whole blob is buffered and every part starts at once —
/// each then racing the HTTP client's per-request timeout on a slow link.
const ARTIFACT_UPLOAD_IN_FLIGHT: usize = 4;

/// Size of one ranged read when downloading. Each range is its own request
/// with its own timeout, so a large blob on a slow link still completes.
const ARTIFACT_RANGE_BYTES: u64 = 8 << 20;

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

    /// Stream `src` to `artifacts/<sha256>` as a multipart upload of
    /// `part_bytes` parts, at most `max_in_flight` of them uploading at once.
    ///
    /// Driven through [`object_store::MultipartUpload`] directly rather than
    /// `WriteMultipart`: its `finish` drops the upload without aborting when a
    /// part failed, which leaves orphaned parts in the bucket. Here every
    /// failure — a read, a part, the completion — aborts the upload (the
    /// abort's own error is ignored) and returns the original error.
    async fn upload_artifact_file(
        &self,
        sha256: &str,
        src: &std::path::Path,
        part_bytes: usize,
        max_in_flight: usize,
    ) -> Result<(), BucketError> {
        let read_err =
            |e: std::io::Error| BucketError::Io(format!("reading artifact {sha256}: {e}"));
        let store_err = |e: object_store::Error| BucketError::Io(e.to_string());
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
                .map_err(read_err)?;
        let mut f = tokio::fs::File::from_std(file);

        // Up to one part off the file; shorter than `part_bytes` means EOF.
        let first = Self::read_part(&mut f, part_bytes)
            .await
            .map_err(read_err)?;
        let path = self.path(&key_artifact(sha256));
        if first.len() < part_bytes {
            // The whole blob (possibly none of it) fits one part: a plain PUT.
            // No multipart to orphan, and S3 can't complete a zero-part one.
            self.store
                .put(&path, PutPayload::from(Bytes::from(first)))
                .await
                .map_err(store_err)?;
            return Ok(());
        }

        let mut upload = self.store.put_multipart(&path).await.map_err(store_err)?;
        let mut parts: tokio::task::JoinSet<object_store::Result<()>> = Default::default();
        let sent = async {
            let mut chunk = first;
            loop {
                let last = chunk.len() < part_bytes;
                // Back pressure: wait for a slot before starting another part,
                // so the file is read no faster than the uplink takes it.
                while parts.len() >= max_in_flight.max(1) {
                    Self::join_part(&mut parts).await?;
                }
                parts.spawn(upload.put_part(PutPayload::from(Bytes::from(chunk))));
                if last {
                    return Ok(());
                }
                chunk = Self::read_part(&mut f, part_bytes)
                    .await
                    .map_err(read_err)?;
                if chunk.is_empty() {
                    // The file ended exactly on a part boundary.
                    return Ok(());
                }
            }
        }
        .await;
        // Every outstanding part, then the completion.
        let sent = match sent {
            Ok(()) => {
                let mut done = Ok(());
                while !parts.is_empty() {
                    if let Err(e) = Self::join_part(&mut parts).await {
                        done = Err(e);
                        break;
                    }
                }
                match done {
                    Ok(()) => upload.complete().await.map(|_| ()).map_err(store_err),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        if let Err(e) = sent {
            // Stop the parts still uploading, then drop what was uploaded.
            parts.shutdown().await;
            let _ = upload.abort().await;
            return Err(e);
        }
        Ok(())
    }

    /// Up to `part_bytes` from `f`: fewer only at end of file.
    async fn read_part(f: &mut tokio::fs::File, part_bytes: usize) -> std::io::Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::with_capacity(part_bytes);
        f.take(part_bytes as u64).read_to_end(&mut buf).await?;
        Ok(buf)
    }

    /// Wait for the next upload part to finish.
    async fn join_part(
        parts: &mut tokio::task::JoinSet<object_store::Result<()>>,
    ) -> Result<(), BucketError> {
        match parts.join_next().await {
            None | Some(Ok(Ok(()))) => Ok(()),
            Some(Ok(Err(e))) => Err(BucketError::Io(e.to_string())),
            Some(Err(e)) => Err(BucketError::Io(format!("artifact upload part: {e}"))),
        }
    }

    /// Download `artifacts/<sha256>` into `dest` as ranged reads of at most
    /// `range_bytes` each (one request per range, so a slow link never trips
    /// the whole-request timeout a single big GET would), refusing an object
    /// over `max_bytes` before anything is read or `dest` is created.
    async fn download_artifact_to_file(
        &self,
        sha256: &str,
        dest: &std::path::Path,
        max_bytes: u64,
        range_bytes: u64,
    ) -> Result<(), BucketError> {
        use tokio::io::AsyncWriteExt;
        let io = |e: std::io::Error| BucketError::Io(format!("local write failed: {e}"));
        let too_big = || {
            BucketError::Io(format!(
                "artifact {sha256} exceeds its recorded {max_bytes} bytes"
            ))
        };
        let not_found = || BucketError::NotFound(key_artifact(sha256));
        let path = self.path(&key_artifact(sha256));
        let size = match self.store.head(&path).await {
            Ok(meta) => meta.size,
            Err(object_store::Error::NotFound { .. }) => return Err(not_found()),
            Err(e) => return Err(BucketError::Io(e.to_string())),
        };
        if size > max_bytes {
            return Err(too_big());
        }
        let mut file = tokio::fs::File::create(dest).await.map_err(io)?;
        let mut written: u64 = 0;
        while written < size {
            let end = size.min(written + range_bytes.max(1));
            let chunk = match self.store.get_range(&path, written..end).await {
                Ok(c) => c,
                Err(object_store::Error::NotFound { .. }) => return Err(not_found()),
                Err(e) => return Err(BucketError::Io(e.to_string())),
            };
            if chunk.len() as u64 != end - written {
                return Err(BucketError::Io(format!(
                    "artifact {sha256}: short read of bytes {written}..{end} \
                     (got {} bytes)",
                    chunk.len()
                )));
            }
            written += chunk.len() as u64;
            if written > max_bytes {
                return Err(too_big());
            }
            file.write_all(&chunk).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        // Every byte the object holds, no fewer.
        if written != size {
            return Err(BucketError::Io(format!(
                "artifact {sha256}: wrote {written} of {size} bytes"
            )));
        }
        Ok(())
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
        self.upload_artifact_file(sha256, src, ARTIFACT_PART_BYTES, ARTIFACT_UPLOAD_IN_FLIGHT)
            .await
    }

    async fn get_artifact_to_file(
        &self,
        sha256: &str,
        dest: &std::path::Path,
        max_bytes: u64,
    ) -> Result<(), BucketError> {
        self.download_artifact_to_file(sha256, dest, max_bytes, ARTIFACT_RANGE_BYTES)
            .await
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
        // Over the cap is refused up front: nothing is written, and `dest`
        // is not even created.
        let capped = tmp.path().join("capped");
        assert!(matches!(
            b.get_artifact_to_file(&sha, &capped, 8).await,
            Err(BucketError::Io(m)) if m.contains("exceeds its recorded 8 bytes")
        ));
        assert!(!capped.exists(), "the early size refusal created dest");
        let absent = tmp.path().join("absent");
        assert!(matches!(
            b.get_artifact_to_file(&"cd".repeat(32), &absent, 9).await,
            Err(BucketError::NotFound(_))
        ));
        assert!(!absent.exists());
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

    // ── instrumented store: concurrency, abort, ranged reads ─────────────────

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

    /// What a [`SpyStore`] saw, and what it should break.
    #[derive(Debug)]
    struct Spy {
        /// `put_part` futures currently running / the most at once.
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
        parts: AtomicUsize,
        completed: AtomicUsize,
        aborted: AtomicUsize,
        /// The Nth `put_part` (0-based) fails; `usize::MAX` = none.
        fail_part: AtomicUsize,
        fail_complete: AtomicBool,
        /// Ranged reads asked for, in order, and whole-object reads.
        ranges: std::sync::Mutex<Vec<std::ops::Range<u64>>>,
        full_gets: AtomicUsize,
        /// Answer every ranged read one byte short.
        short_reads: AtomicBool,
    }

    impl Spy {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                in_flight: AtomicUsize::new(0),
                max_in_flight: AtomicUsize::new(0),
                parts: AtomicUsize::new(0),
                completed: AtomicUsize::new(0),
                aborted: AtomicUsize::new(0),
                fail_part: AtomicUsize::new(usize::MAX),
                fail_complete: AtomicBool::new(false),
                ranges: Default::default(),
                full_gets: AtomicUsize::new(0),
                short_reads: AtomicBool::new(false),
            })
        }
    }

    #[derive(Debug)]
    struct SpyStore {
        inner: object_store::memory::InMemory,
        spy: Arc<Spy>,
    }

    impl std::fmt::Display for SpyStore {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "SpyStore")
        }
    }

    #[derive(Debug)]
    struct SpyUpload {
        inner: Box<dyn object_store::MultipartUpload>,
        spy: Arc<Spy>,
    }

    fn spy_err(what: &'static str) -> object_store::Error {
        object_store::Error::Generic {
            store: "spy",
            source: what.into(),
        }
    }

    #[async_trait]
    impl object_store::MultipartUpload for SpyUpload {
        fn put_part(&mut self, data: PutPayload) -> object_store::UploadPart {
            let n = self.spy.parts.fetch_add(1, SeqCst);
            let spy = Arc::clone(&self.spy);
            let part = self.inner.put_part(data);
            Box::pin(async move {
                let now = spy.in_flight.fetch_add(1, SeqCst) + 1;
                spy.max_in_flight.fetch_max(now, SeqCst);
                // Long enough that unbounded parts would overlap.
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                let r = if n == spy.fail_part.load(SeqCst) {
                    Err(spy_err("part failed"))
                } else {
                    part.await
                };
                spy.in_flight.fetch_sub(1, SeqCst);
                r
            })
        }
        async fn complete(&mut self) -> object_store::Result<object_store::PutResult> {
            if self.spy.fail_complete.load(SeqCst) {
                return Err(spy_err("complete failed"));
            }
            self.spy.completed.fetch_add(1, SeqCst);
            self.inner.complete().await
        }
        async fn abort(&mut self) -> object_store::Result<()> {
            self.spy.aborted.fetch_add(1, SeqCst);
            self.inner.abort().await
        }
    }

    #[async_trait]
    impl ObjectStore for SpyStore {
        async fn put_opts(
            &self,
            location: &Path,
            payload: PutPayload,
            opts: PutOptions,
        ) -> object_store::Result<object_store::PutResult> {
            self.inner.put_opts(location, payload, opts).await
        }
        async fn put_multipart_opts(
            &self,
            location: &Path,
            opts: object_store::PutMultipartOptions,
        ) -> object_store::Result<Box<dyn object_store::MultipartUpload>> {
            let inner = self.inner.put_multipart_opts(location, opts).await?;
            Ok(Box::new(SpyUpload {
                inner,
                spy: Arc::clone(&self.spy),
            }))
        }
        async fn get_opts(
            &self,
            location: &Path,
            mut options: object_store::GetOptions,
        ) -> object_store::Result<object_store::GetResult> {
            match &options.range {
                Some(object_store::GetRange::Bounded(r)) => {
                    self.spy.ranges.lock().unwrap().push(r.clone());
                    if self.spy.short_reads.load(SeqCst) && r.end - r.start > 1 {
                        options.range = Some(object_store::GetRange::Bounded(r.start..r.end - 1));
                    }
                }
                None if !options.head => {
                    self.spy.full_gets.fetch_add(1, SeqCst);
                }
                _ => {}
            }
            self.inner.get_opts(location, options).await
        }
        fn delete_stream(
            &self,
            locations: futures_util::stream::BoxStream<'static, object_store::Result<Path>>,
        ) -> futures_util::stream::BoxStream<'static, object_store::Result<Path>> {
            self.inner.delete_stream(locations)
        }
        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> futures_util::stream::BoxStream<'static, object_store::Result<object_store::ObjectMeta>>
        {
            self.inner.list(prefix)
        }
        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<object_store::ListResult> {
            self.inner.list_with_delimiter(prefix).await
        }
        async fn copy_opts(
            &self,
            from: &Path,
            to: &Path,
            options: object_store::CopyOptions,
        ) -> object_store::Result<()> {
            self.inner.copy_opts(from, to, options).await
        }
    }

    fn spy_bucket() -> (ObjectStoreBucket, Arc<Spy>) {
        let spy = Spy::new();
        let store = SpyStore {
            inner: object_store::memory::InMemory::new(),
            spy: Arc::clone(&spy),
        };
        (
            ObjectStoreBucket::new(Arc::new(store), "test-prefix/host_1"),
            spy,
        )
    }

    fn patterned(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    /// Upload parts are produced no faster than the uplink takes them: with
    /// many parts in the blob, never more than the limit are in flight (an
    /// unbounded writer would buffer the whole blob and start every part at
    /// once, each then racing the client's per-request timeout).
    #[tokio::test]
    async fn artifact_upload_bounds_in_flight_parts() {
        let (b, spy) = spy_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let body = patterned(24 * 1024);
        let src = tmp.path().join("blob");
        std::fs::write(&src, &body).unwrap();
        let sha = "34".repeat(32);

        b.upload_artifact_file(&sha, &src, 1024, 3).await.unwrap();

        assert_eq!(spy.parts.load(SeqCst), 24);
        let max = spy.max_in_flight.load(SeqCst);
        assert!((2..=3).contains(&max), "max in flight {max}");
        assert_eq!(spy.completed.load(SeqCst), 1);
        assert_eq!(spy.aborted.load(SeqCst), 0);
        let dest = tmp.path().join("out");
        b.get_artifact_to_file(&sha, &dest, body.len() as u64)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    /// A failed part aborts the multipart upload (no orphaned parts, nothing
    /// visible at the key) and surfaces the part's error.
    #[tokio::test]
    async fn artifact_upload_aborts_when_a_part_fails() {
        let (b, spy) = spy_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("blob");
        std::fs::write(&src, patterned(16 * 1024)).unwrap();
        let sha = "56".repeat(32);
        spy.fail_part.store(5, SeqCst);

        let err = b
            .upload_artifact_file(&sha, &src, 1024, 3)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("part failed"), "{err}");
        assert_eq!(spy.aborted.load(SeqCst), 1, "the upload was not aborted");
        assert_eq!(spy.completed.load(SeqCst), 0);
        assert!(!b.artifact_exists(&sha).await.unwrap());
    }

    /// A failed completion aborts too, exactly once.
    #[tokio::test]
    async fn artifact_upload_aborts_when_completion_fails() {
        let (b, spy) = spy_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("blob");
        std::fs::write(&src, patterned(4 * 1024)).unwrap();
        let sha = "78".repeat(32);
        spy.fail_complete.store(true, SeqCst);

        let err = b
            .upload_artifact_file(&sha, &src, 1024, 3)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("complete failed"), "{err}");
        assert_eq!(spy.aborted.load(SeqCst), 1);
        assert!(!b.artifact_exists(&sha).await.unwrap());
    }

    /// How a blob maps onto parts: under one part is a plain PUT (no
    /// multipart), exactly N parts is N parts (no empty trailing part), one
    /// byte over is N+1. Every shape reads back byte-exact.
    #[tokio::test]
    async fn artifact_upload_part_boundaries() {
        let (b, spy) = spy_bucket();
        let tmp = tempfile::tempdir().unwrap();
        for (i, (len, parts)) in [
            (1023usize, 0usize),
            (1024, 1),
            (3 * 1024, 3),
            (3 * 1024 + 1, 4),
        ]
        .into_iter()
        .enumerate()
        {
            let body = patterned(len);
            let src = tmp.path().join(format!("blob{i}"));
            std::fs::write(&src, &body).unwrap();
            let sha = format!("{i:02x}").repeat(32);
            let before = spy.parts.load(SeqCst);
            b.upload_artifact_file(&sha, &src, 1024, 2).await.unwrap();
            assert_eq!(spy.parts.load(SeqCst) - before, parts, "{len} bytes");
            let dest = tmp.path().join(format!("out{i}"));
            b.get_artifact_to_file(&sha, &dest, len as u64)
                .await
                .unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), body, "{len} bytes");
        }
        assert_eq!(spy.aborted.load(SeqCst), 0);
    }

    /// An empty blob uploads (as a plain PUT — S3 can't complete a multipart
    /// with no parts) and reads back empty.
    #[tokio::test]
    async fn empty_artifact_roundtrips() {
        let (b, spy) = spy_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("empty");
        std::fs::write(&src, b"").unwrap();
        let sha = "9a".repeat(32);
        b.upload_artifact_file(&sha, &src, 1024, 3).await.unwrap();
        assert!(b.artifact_exists(&sha).await.unwrap());
        let dest = tmp.path().join("out");
        b.download_artifact_to_file(&sha, &dest, 0, 1024)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"");
        assert!(spy.ranges.lock().unwrap().is_empty());
        assert_eq!(spy.parts.load(SeqCst), 0, "an empty blob used a multipart");
    }

    /// The download is a series of bounded ranged reads (each its own
    /// request, so a slow link never hits the whole-request timeout), never
    /// one whole-object GET, and reassembles byte-exact.
    #[tokio::test]
    async fn artifact_download_is_ranged_and_byte_exact() {
        let (b, spy) = spy_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let body = patterned((3 << 20) + 17);
        let src = tmp.path().join("blob");
        std::fs::write(&src, &body).unwrap();
        let sha = "bc".repeat(32);
        b.put_artifact_file(&sha, &src).await.unwrap();

        let dest = tmp.path().join("out");
        let size = body.len() as u64;
        b.download_artifact_to_file(&sha, &dest, size, 1 << 20)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(spy.full_gets.load(SeqCst), 0, "a whole-object GET was used");
        assert_eq!(
            *spy.ranges.lock().unwrap(),
            vec![
                0..(1 << 20),
                (1 << 20)..(2 << 20),
                (2 << 20)..(3 << 20),
                (3 << 20)..size,
            ]
        );
    }

    /// The trait method uses the production range size: a blob smaller than
    /// one range is one ranged read.
    #[tokio::test]
    async fn artifact_download_through_the_trait_uses_ranged_reads() {
        let (b, spy) = spy_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("blob");
        std::fs::write(&src, b"small").unwrap();
        let sha = "de".repeat(32);
        b.put_artifact_file(&sha, &src).await.unwrap();
        let dest = tmp.path().join("out");
        b.get_artifact_to_file(&sha, &dest, 5).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"small");
        assert_eq!(*spy.ranges.lock().unwrap(), vec![0..5]);
        assert_eq!(spy.full_gets.load(SeqCst), 0);
    }

    /// A store that answers a range short is an error (never a silent
    /// truncated `Ok`), and an over-cap object is refused before any read.
    #[tokio::test]
    async fn artifact_download_rejects_a_short_read_and_an_over_cap_object() {
        let (b, spy) = spy_bucket();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("blob");
        std::fs::write(&src, patterned(4096)).unwrap();
        let sha = "f0".repeat(32);
        b.put_artifact_file(&sha, &src).await.unwrap();

        spy.short_reads.store(true, SeqCst);
        let err = b
            .download_artifact_to_file(&sha, &tmp.path().join("short"), 4096, 1024)
            .await
            .unwrap_err();
        assert!(
            matches!(err, BucketError::Io(ref m) if m.contains("short")),
            "{err:?}"
        );
        spy.short_reads.store(false, SeqCst);

        spy.ranges.lock().unwrap().clear();
        let over = tmp.path().join("over");
        let err = b
            .download_artifact_to_file(&sha, &over, 4095, 1024)
            .await
            .unwrap_err();
        assert!(
            matches!(err, BucketError::Io(ref m) if m.contains("exceeds its recorded 4095 bytes")),
            "{err:?}"
        );
        assert!(!over.exists());
        assert!(
            spy.ranges.lock().unwrap().is_empty(),
            "read before refusing"
        );
    }
}
