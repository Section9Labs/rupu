//! CP-side bucket poller — mirrors dead-drop results from a bucket host into
//! the central [`RunStore`] via [`NodeMirror`].
//!
//! [`poll_bucket_run`] is the testable unit; it is called once per in-flight
//! run per tick by the `run_bucket_poller` loop in `rupu-cli`.

use std::collections::HashSet;

use anyhow::Context as _;

use crate::{
    host::bucket::Bucket,
    node::{protocol::ArtifactFile, NodeMirror},
};

/// Poll one run from `bucket`, mirroring unconsumed result objects into the
/// central [`NodeMirror`] and returning `true` when the run's finished marker
/// is present (run is done).
///
/// # Idempotency
/// `consumed` tracks which bucket keys have already been mirrored.  Re-calling
/// with the same set is safe and performs no duplicate I/O — the function
/// skips every key already in the set and only appends lines for new keys.
///
/// # Returns
/// `Ok(true)` when `get_finished` returns `Some(status)` (the node wrote the
/// finished marker); `Ok(false)` when the run is still in-flight.
pub async fn poll_bucket_run(
    bucket: &dyn Bucket,
    mirror: &NodeMirror,
    host_id: &str,
    run_id: &str,
    consumed: &mut HashSet<String>,
) -> anyhow::Result<bool> {
    mirror_new_results(bucket, mirror, host_id, run_id, consumed).await?;

    // Check whether the node has written the finished marker.
    let status = bucket
        .get_finished(run_id)
        .await
        .with_context(|| format!("get_finished for run {run_id}"))?;

    if let Some(status) = status {
        // The node writes the marker strictly AFTER its last result object, but
        // the listing above was taken BEFORE this marker read: an object and the
        // marker can both have landed in the gap. Finishing on the stale
        // listing would mark the run terminal and never read that object (the
        // poller stops polling a finished run). Every object the node wrote
        // precedes the marker we just saw, so one re-list now sees them all.
        mirror_new_results(bucket, mirror, host_id, run_id, consumed).await?;
        mirror
            .finish(run_id, host_id, &status)
            .with_context(|| format!("mirror.finish for run {run_id}"))?;
        return Ok(true);
    }

    Ok(false)
}

/// List `run_id`'s result objects and mirror every one not yet in `consumed`.
async fn mirror_new_results(
    bucket: &dyn Bucket,
    mirror: &NodeMirror,
    host_id: &str,
    run_id: &str,
    consumed: &mut HashSet<String>,
) -> anyhow::Result<()> {
    let results = bucket
        .list_results(run_id)
        .await
        .with_context(|| format!("list_results for run {run_id}"))?;

    for (key, body) in results {
        if consumed.contains(&key) {
            continue;
        }

        // Classify by filename to the matching ArtifactFile variant.
        let Some(file) = classify_key(&key) else {
            tracing::debug!(key = %key, run_id = %run_id, "bucket poller: unknown result key, skipping");
            consumed.insert(key);
            continue;
        };

        let body_str = String::from_utf8_lossy(&body);

        match file {
            ArtifactFile::RunJson => {
                // run.json is a single JSON document, not newline-delimited.
                // Re-mirror on EVERY tick — the node overwrites this key each
                // tick with updated status (e.g. awaiting_approval mid-run).
                // Do NOT add "run.json" to `consumed` so each poll picks it up.
                mirror
                    .append(run_id, host_id, ArtifactFile::RunJson, &body_str)
                    .with_context(|| format!("mirror.append RunJson for run {run_id}"))?;
                continue; // skip the `consumed.insert` below
            }
            _ => {
                // JSONL: split on newline and mirror each non-empty line.
                for line in body_str.split('\n') {
                    let line = line.trim_end_matches('\r');
                    if line.is_empty() {
                        continue;
                    }
                    mirror
                        .append(run_id, host_id, file.clone(), line)
                        .with_context(|| {
                            format!("mirror.append {file:?} line for run {run_id}")
                        })?;
                }
            }
        }

        consumed.insert(key);
    }

    Ok(())
}

/// Map a result-object filename (not the full path) to the matching
/// [`ArtifactFile`] variant.
///
/// Patterns (filename only, key layout: `runs/<run_id>/<key>`):
/// - `events*.jsonl`            → [`ArtifactFile::Events`]
/// - `step_results*.jsonl`      → [`ArtifactFile::StepResults`]
/// - `unit_checkpoints*.jsonl`  → [`ArtifactFile::UnitCheckpoints`]
/// - `usage*.jsonl`             → [`ArtifactFile::Usage`]
/// - `coverage*.jsonl`          → [`ArtifactFile::Coverage`]
/// - `run.json`                 → [`ArtifactFile::RunJson`]
/// - anything else              → `None` (caller skips + marks consumed)
fn classify_key(key: &str) -> Option<ArtifactFile> {
    if key == "run.json" {
        return Some(ArtifactFile::RunJson);
    }
    if key.ends_with(".jsonl") {
        if key.starts_with("events") {
            return Some(ArtifactFile::Events);
        }
        if key.starts_with("step_results") {
            return Some(ArtifactFile::StepResults);
        }
        if key.starts_with("unit_checkpoints") {
            return Some(ArtifactFile::UnitCheckpoints);
        }
        if key.starts_with("usage") {
            return Some(ArtifactFile::Usage);
        }
        if key.starts_with("coverage") {
            return Some(ArtifactFile::Coverage);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::bucket::{BucketError, ObjectStoreBucket};

    /// A [`Bucket`] over an in-memory store whose node finishes its run in the
    /// gap between the poller's `list_results` and its `get_finished`: on the
    /// first marker read the final result object lands, then the marker — the
    /// node's own order (marker strictly last).
    struct FinishesMidPoll {
        inner: ObjectStoreBucket,
        late: std::sync::Mutex<Option<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl Bucket for FinishesMidPoll {
        async fn put_job(&self, run_id: &str, b: &[u8]) -> Result<(), BucketError> {
            self.inner.put_job(run_id, b).await
        }
        async fn list_jobs(&self) -> Result<Vec<String>, BucketError> {
            self.inner.list_jobs().await
        }
        async fn claim_job(&self, run_id: &str, w: &str) -> Result<bool, BucketError> {
            self.inner.claim_job(run_id, w).await
        }
        async fn get_job(&self, run_id: &str) -> Result<Vec<u8>, BucketError> {
            self.inner.get_job(run_id).await
        }
        async fn put_control(&self, run_id: &str, seq: u64, b: &[u8]) -> Result<(), BucketError> {
            self.inner.put_control(run_id, seq, b).await
        }
        async fn list_control(&self, run_id: &str) -> Result<Vec<(u64, Vec<u8>)>, BucketError> {
            self.inner.list_control(run_id).await
        }
        async fn put_result(&self, run_id: &str, key: &str, b: &[u8]) -> Result<(), BucketError> {
            self.inner.put_result(run_id, key, b).await
        }
        async fn list_results(&self, run_id: &str) -> Result<Vec<(String, Vec<u8>)>, BucketError> {
            self.inner.list_results(run_id).await
        }
        async fn put_finished(&self, run_id: &str, status: &str) -> Result<(), BucketError> {
            self.inner.put_finished(run_id, status).await
        }
        async fn get_finished(&self, run_id: &str) -> Result<Option<String>, BucketError> {
            let late = self.late.lock().unwrap().take();
            if let Some((key, body)) = late {
                self.inner.put_result(run_id, &key, body.as_bytes()).await?;
                self.inner.put_finished(run_id, "completed").await?;
            }
            self.inner.get_finished(run_id).await
        }
        async fn probe(&self) -> Result<(), BucketError> {
            self.inner.probe().await
        }
        async fn put_worker_info(&self, id: &str, b: &[u8]) -> Result<(), BucketError> {
            self.inner.put_worker_info(id, b).await
        }
        async fn list_worker_info(&self) -> Result<Vec<Vec<u8>>, BucketError> {
            self.inner.list_worker_info().await
        }
    }

    /// The node writes its last result object and then the finished marker. A
    /// poll that LISTED before that and reads the marker after it must not
    /// finish the run on the stale listing — the object would be marked
    /// terminal-and-never-read for good. Seeing the marker triggers one
    /// re-list, so the late object is mirrored before the run is finished.
    #[tokio::test]
    async fn a_result_landing_between_the_list_and_the_marker_is_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(dir.path().join("runs")));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        let (run_id, host_id) = ("run_POLLRACE0000001", "host_RACE");
        mirror
            .create_run(
                run_id,
                host_id,
                &crate::node::protocol::RunSpec {
                    kind: crate::node::protocol::RunSpecKind::Workflow,
                    name: "w".into(),
                    inputs: Default::default(),
                    prompt: None,
                    mode: None,
                    target: None,
                    findings_profile: None,
                },
            )
            .unwrap();

        let inner = ObjectStoreBucket::new(
            std::sync::Arc::new(object_store::memory::InMemory::new()),
            "p",
        );
        inner
            .put_result(run_id, "coverage.0000.jsonl", b"first\n")
            .await
            .unwrap();
        let bucket = FinishesMidPoll {
            inner,
            late: std::sync::Mutex::new(Some(("coverage.0001.jsonl".into(), "last\n".into()))),
        };

        let mut consumed = HashSet::new();
        let done = poll_bucket_run(&bucket, &mirror, host_id, run_id, &mut consumed)
            .await
            .unwrap();
        assert!(done, "the marker was seen, so the run is done");
        assert_eq!(
            std::fs::read_to_string(mirror.coverage_path(run_id)).unwrap(),
            "first\nlast\n",
            "the object that landed after the list but before the marker is mirrored too"
        );
        assert!(consumed.contains("coverage.0001.jsonl"));
    }

    #[test]
    fn classify_key_maps_known_suffixes() {
        assert!(matches!(
            classify_key("events.0001.jsonl"),
            Some(ArtifactFile::Events)
        ));
        assert!(matches!(
            classify_key("step_results.0001.jsonl"),
            Some(ArtifactFile::StepResults)
        ));
        assert!(matches!(
            classify_key("unit_checkpoints.0001.jsonl"),
            Some(ArtifactFile::UnitCheckpoints)
        ));
        assert!(matches!(
            classify_key("usage.0001.jsonl"),
            Some(ArtifactFile::Usage)
        ));
        assert!(matches!(
            classify_key("coverage.0001.jsonl"),
            Some(ArtifactFile::Coverage)
        ));
        assert!(matches!(classify_key("run.json"), Some(ArtifactFile::RunJson)));
        assert!(classify_key("finished").is_none());
        assert!(classify_key("unknown.txt").is_none());
        assert!(classify_key("").is_none());
    }
}
