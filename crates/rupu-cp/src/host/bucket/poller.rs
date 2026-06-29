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

    // Check whether the node has written the finished marker.
    let status = bucket
        .get_finished(run_id)
        .await
        .with_context(|| format!("get_finished for run {run_id}"))?;

    if let Some(status) = status {
        mirror
            .finish(run_id, host_id, &status)
            .with_context(|| format!("mirror.finish for run {run_id}"))?;
        return Ok(true);
    }

    Ok(false)
}

/// Map a result-object filename (not the full path) to the matching
/// [`ArtifactFile`] variant.
///
/// Patterns (filename only, key layout: `runs/<run_id>/<key>`):
/// - `events*.jsonl`            → [`ArtifactFile::Events`]
/// - `step_results*.jsonl`      → [`ArtifactFile::StepResults`]
/// - `unit_checkpoints*.jsonl`  → [`ArtifactFile::UnitCheckpoints`]
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
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(matches!(classify_key("run.json"), Some(ArtifactFile::RunJson)));
        assert!(classify_key("finished").is_none());
        assert!(classify_key("unknown.txt").is_none());
        assert!(classify_key("").is_none());
    }
}
