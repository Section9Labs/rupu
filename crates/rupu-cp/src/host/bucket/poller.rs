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
    // The marker is read BEFORE the listing. The node writes it strictly
    // AFTER its last result object, so a listing taken after seeing it holds
    // every object the run wrote; finishing on a listing taken before it
    // could mark the run terminal with an object never read (the poller
    // stops polling a finished run).
    let status = bucket
        .get_finished(run_id)
        .await
        .with_context(|| format!("get_finished for run {run_id}"))?;
    let failure = mirror_new_results(bucket, mirror, host_id, run_id, consumed).await?;

    let Some(status) = status else {
        return Ok(false);
    };
    // A worker that failed a job before it ran (unusable skills, a spawn
    // failure) says why in `failure.txt`, written before the marker.
    let reason = match (status.as_str(), failure) {
        ("failed", Some(body)) => failure_reason(&body),
        _ => None,
    }
    .map(|r| format!("host {host_id}: {r}"));
    mirror
        .finish_with_reason(run_id, host_id, &status, reason)
        .await
        .with_context(|| format!("mirror.finish for run {run_id}"))?;
    Ok(true)
}

/// Result key of a worker's reason for failing a job. It is neither
/// `run.json` nor a `.jsonl` stream, so [`classify_key`] (and an older
/// poller's) skips it.
pub const FAILURE_KEY: &str = "failure.txt";

/// Longest failure reason the coordinator keeps.
const MAX_FAILURE_REASON: usize = 4096;

/// The reason in a worker's `failure.txt` body, trimmed and capped; `None`
/// when it is blank.
fn failure_reason(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    let text: String = text.trim().chars().take(MAX_FAILURE_REASON).collect();
    (!text.is_empty()).then_some(text)
}

/// List `run_id`'s result objects and mirror every one not yet in `consumed`.
/// Returns the listing's [`FAILURE_KEY`] body, if the worker wrote one.
async fn mirror_new_results(
    bucket: &dyn Bucket,
    mirror: &NodeMirror,
    host_id: &str,
    run_id: &str,
    consumed: &mut HashSet<String>,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut results = bucket
        .list_results(run_id)
        .await
        .with_context(|| format!("list_results for run {run_id}"))?;
    // A listing comes back in key order, which is not chunk order past seq
    // 9,999 (see `result_order`); lines must land in the order written.
    results.sort_by(|(a, _), (b, _)| result_order(a).cmp(&result_order(b)));

    let mut failure = None;
    for (key, body) in results {
        if key == FAILURE_KEY {
            // Not a stream: the caller reads it once the run has failed.
            failure = Some(body);
            continue;
        }
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
                    .await
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
                        .await
                        .with_context(|| format!("mirror.append {file:?} line for run {run_id}"))?;
                }
            }
        }

        consumed.insert(key);
    }

    Ok(failure)
}

/// The order result objects are mirrored in: by kind, then by the numeric
/// sequence parsed from `<kind>.<seq>.jsonl`. The worker zero-pads `seq` to
/// four digits (`result_key`), which keeps lexicographic order right only up
/// to 9,999 — `coverage.10000.jsonl` sorts before `coverage.9999.jsonl` —
/// and older workers write that format, so the key format stays and the
/// order comes from the number. A key that does not parse sorts after every
/// parsed one of its name, by name.
fn result_order(key: &str) -> (&str, u64, &str) {
    key.strip_suffix(".jsonl")
        .and_then(|stem| stem.rsplit_once('.'))
        .and_then(|(kind, seq)| seq.parse::<u64>().ok().map(|n| (kind, n, key)))
        .unwrap_or((key, u64::MAX, key))
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

    /// A [`Bucket`] over an in-memory store whose node finishes its run as
    /// the poller reads the marker: on the first marker read the final
    /// result object lands, then the marker with `status` — the node's own
    /// order (marker strictly last). Counts `list_results` calls.
    struct FinishesMidPoll {
        inner: ObjectStoreBucket,
        /// (key, body, status)
        late: std::sync::Mutex<Option<(String, String, String)>>,
        lists: std::sync::atomic::AtomicUsize,
    }

    impl FinishesMidPoll {
        fn new(inner: ObjectStoreBucket, key: &str, body: &str, status: &str) -> Self {
            Self {
                inner,
                late: std::sync::Mutex::new(Some((key.into(), body.into(), status.into()))),
                lists: Default::default(),
            }
        }

        fn lists(&self) -> usize {
            self.lists.load(std::sync::atomic::Ordering::SeqCst)
        }
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
            self.lists.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.list_results(run_id).await
        }
        async fn put_finished(&self, run_id: &str, status: &str) -> Result<(), BucketError> {
            self.inner.put_finished(run_id, status).await
        }
        async fn get_finished(&self, run_id: &str) -> Result<Option<String>, BucketError> {
            let late = self.late.lock().unwrap().take();
            if let Some((key, body, status)) = late {
                self.inner.put_result(run_id, &key, body.as_bytes()).await?;
                self.inner.put_finished(run_id, &status).await?;
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
        async fn artifact_exists(&self, sha256: &str) -> Result<bool, BucketError> {
            self.inner.artifact_exists(sha256).await
        }
        async fn put_artifact_file(
            &self,
            sha256: &str,
            src: &std::path::Path,
        ) -> Result<(), BucketError> {
            self.inner.put_artifact_file(sha256, src).await
        }
        async fn get_artifact_to_file(
            &self,
            sha256: &str,
            dest: &std::path::Path,
            max_bytes: u64,
        ) -> Result<(), BucketError> {
            self.inner
                .get_artifact_to_file(sha256, dest, max_bytes)
                .await
        }
        async fn put_skill(&self, hash: &str, bytes: &[u8]) -> Result<(), BucketError> {
            self.inner.put_skill(hash, bytes).await
        }
        async fn get_skill(&self, hash: &str, max_bytes: u64) -> Result<Vec<u8>, BucketError> {
            self.inner.get_skill(hash, max_bytes).await
        }
    }

    /// The worker's failure reason is a result object an older poller must
    /// skip: `classify_key` knows nothing about it (and `poll_bucket_run`
    /// marks unknown keys consumed without mirroring them).
    #[test]
    fn the_failure_reason_key_is_skipped_by_the_stream_classifier() {
        assert!(classify_key(FAILURE_KEY).is_none());
    }

    /// The node writes its last result object and then the finished marker,
    /// both just before the poll reads the marker. A poll that finished the
    /// run on a listing taken before that would leave the object
    /// terminal-and-never-read for good. The poll lists once, after the
    /// marker, so the late object is mirrored before the run is finished.
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
                    skill_bundle: None,
                    messaging_off: false,
                    kind: crate::node::protocol::RunSpecKind::Workflow,
                    name: "w".into(),
                    inputs: Default::default(),
                    prompt: None,
                    mode: None,
                    target: None,
                    findings_profile: None,
                    engagement_profiles: Vec::new(),
                    skills: Vec::new(),
                    preload_skills: Vec::new(),
                    codename: None,
                },
            )
            .await
            .unwrap();

        let inner = ObjectStoreBucket::new(
            std::sync::Arc::new(object_store::memory::InMemory::new()),
            "p",
        );
        inner
            .put_result(run_id, "coverage.0000.jsonl", b"first\n")
            .await
            .unwrap();
        let bucket = FinishesMidPoll::new(inner, "coverage.0001.jsonl", "last\n", "completed");

        let mut consumed = HashSet::new();
        let done = poll_bucket_run(&bucket, &mirror, host_id, run_id, &mut consumed)
            .await
            .unwrap();
        assert!(done, "the marker was seen, so the run is done");
        assert_eq!(bucket.lists(), 1, "one listing per poll");
        assert_eq!(
            std::fs::read_to_string(mirror.coverage_path(run_id)).unwrap(),
            "first\nlast\n",
            "the object that landed after the list but before the marker is mirrored too"
        );
        assert!(consumed.contains("coverage.0001.jsonl"));
    }

    /// A failed run's `failure.txt` (written before the marker) becomes its
    /// record's `error_message`, taken from the poll's one listing.
    #[tokio::test]
    async fn a_failed_runs_reason_comes_from_the_polls_one_listing() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(dir.path().join("runs")));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        let (run_id, host_id) = ("run_POLLFAIL0000001", "host_FAIL");
        mirror
            .create_run(
                run_id,
                host_id,
                &crate::node::protocol::RunSpec {
                    skill_bundle: None,
                    messaging_off: false,
                    kind: crate::node::protocol::RunSpecKind::Workflow,
                    name: "w".into(),
                    inputs: Default::default(),
                    prompt: None,
                    mode: None,
                    target: None,
                    findings_profile: None,
                    engagement_profiles: Vec::new(),
                    skills: Vec::new(),
                    preload_skills: Vec::new(),
                    codename: None,
                },
            )
            .await
            .unwrap();
        let inner = ObjectStoreBucket::new(
            std::sync::Arc::new(object_store::memory::InMemory::new()),
            "p",
        );
        let bucket =
            FinishesMidPoll::new(inner, FAILURE_KEY, "skill global:alpha: gone\n", "failed");

        let mut consumed = HashSet::new();
        let done = poll_bucket_run(&bucket, &mirror, host_id, run_id, &mut consumed)
            .await
            .unwrap();

        assert!(done);
        assert_eq!(bucket.lists(), 1, "the reason costs no second listing");
        let rec = store.load(run_id).unwrap();
        assert_eq!(rec.status, rupu_orchestrator::RunStatus::Failed);
        assert_eq!(
            rec.error_message.as_deref(),
            Some("host host_FAIL: skill global:alpha: gone")
        );
    }

    /// Result keys are `<kind>.<seq:04>.jsonl`, so past 9,999 chunks the
    /// zero-padding runs out and a listing's lexicographic order puts
    /// `10000` before `9999`. Chunks are mirrored in numeric order per kind.
    #[tokio::test]
    async fn results_past_seq_9999_are_mirrored_in_numeric_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(dir.path().join("runs")));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        let (run_id, host_id) = ("run_POLLSEQ00000001", "host_SEQ");
        mirror
            .create_run(
                run_id,
                host_id,
                &crate::node::protocol::RunSpec {
                    skill_bundle: None,
                    messaging_off: false,
                    kind: crate::node::protocol::RunSpecKind::Workflow,
                    name: "w".into(),
                    inputs: Default::default(),
                    prompt: None,
                    mode: None,
                    target: None,
                    findings_profile: None,
                    engagement_profiles: Vec::new(),
                    skills: Vec::new(),
                    preload_skills: Vec::new(),
                    codename: None,
                },
            )
            .await
            .unwrap();
        let bucket = ObjectStoreBucket::new(
            std::sync::Arc::new(object_store::memory::InMemory::new()),
            "p",
        );
        for (key, body) in [
            ("coverage.9998.jsonl", "c1\n"),
            ("coverage.9999.jsonl", "c2\n"),
            ("coverage.10000.jsonl", "c3\n"),
            ("coverage.10001.jsonl", "c4\n"),
            ("events.9999.jsonl", "{\"e\":1}\n"),
            ("events.10000.jsonl", "{\"e\":2}\n"),
        ] {
            bucket
                .put_result(run_id, key, body.as_bytes())
                .await
                .unwrap();
        }

        let mut consumed = HashSet::new();
        poll_bucket_run(&bucket, &mirror, host_id, run_id, &mut consumed)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(mirror.coverage_path(run_id)).unwrap(),
            "c1\nc2\nc3\nc4\n"
        );
        let events = std::fs::read_to_string(store.events_path(run_id)).unwrap();
        assert_eq!(
            events.lines().collect::<Vec<_>>(),
            vec!["{\"e\":1}", "{\"e\":2}"]
        );
    }

    #[test]
    fn result_order_parses_the_numeric_sequence() {
        assert!(result_order("coverage.9999.jsonl") < result_order("coverage.10000.jsonl"));
        assert!(result_order("events.0002.jsonl") < result_order("events.0010.jsonl"));
        // Kinds stay apart; an unparseable key sorts by its name after them.
        assert!(result_order("coverage.10000.jsonl") < result_order("events.0001.jsonl"));
        assert_eq!(result_order("run.json").1, u64::MAX);
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
        assert!(matches!(
            classify_key("run.json"),
            Some(ArtifactFile::RunJson)
        ));
        assert!(classify_key("finished").is_none());
        assert!(classify_key("unknown.txt").is_none());
        assert!(classify_key("").is_none());
    }
}
