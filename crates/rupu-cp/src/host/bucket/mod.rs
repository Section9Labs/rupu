//! Bucket port — shared dead-drop storage for the multi-host pull transport (Slice 2b).
//!
//! [`Bucket`] is the abstract port; [`ObjectStoreBucket`] is the object_store-backed impl.
//! The key layout is:
//!
//! ```text
//! jobs/<run_id>.json          — job envelope (dispatched by CP)
//! jobs/<run_id>.claim         — claim marker (written atomically by the first node that picks up)
//! control/<run_id>/<seq:020>.json — control messages from CP to node, zero-padded seq
//! runs/<run_id>/<key>         — result objects uploaded by the node
//! runs/<run_id>/finished      — terminal status string written by the node
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub(crate) mod connector;
pub(crate) use connector::BucketHostConnector;
pub(crate) mod object_store_bucket;
pub(crate) use object_store_bucket::ObjectStoreBucket;

// ── control envelope ──────────────────────────────────────────────────────────

/// Control message written by the CP into `control/<run_id>/<seq:020>.json`
/// and consumed by the node agent (Task 6).
///
/// Both the connector (T3) and the node agent (T6) use this SAME type so
/// there is no wire-format drift between the two ends of the protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ControlEnvelope {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ── error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub(crate) enum BucketError {
    #[error("bucket io: {0}")]
    Io(String),
    #[error("not found: {0}")]
    NotFound(String),
}

// ── port trait ────────────────────────────────────────────────────────────────

/// Abstract port for the bucket dead-drop transport.
///
/// Implementations must be `Send + Sync`; all methods are async.
#[async_trait]
pub(crate) trait Bucket: Send + Sync {
    /// Write the job envelope for `run_id` at `jobs/<run_id>.json`.
    async fn put_job(&self, run_id: &str, envelope: &[u8]) -> Result<(), BucketError>;

    /// List run_ids that have a `jobs/<run_id>.json` but NO `jobs/<run_id>.claim`.
    async fn list_jobs(&self) -> Result<Vec<String>, BucketError>;

    /// Atomically claim `run_id` for `worker` by writing `jobs/<run_id>.claim`
    /// with `PutMode::Create`.  Returns `Ok(true)` if this call won the race,
    /// `Ok(false)` if the object already existed (another node claimed it first).
    async fn claim_job(&self, run_id: &str, worker: &str) -> Result<bool, BucketError>;

    /// Fetch the job envelope at `jobs/<run_id>.json`.
    async fn get_job(&self, run_id: &str) -> Result<Vec<u8>, BucketError>;

    /// Write a control message at `control/<run_id>/<seq:020>.json`.
    async fn put_control(&self, run_id: &str, seq: u64, envelope: &[u8]) -> Result<(), BucketError>;

    /// Return all control messages for `run_id`, sorted ascending by seq.
    async fn list_control(&self, run_id: &str) -> Result<Vec<(u64, Vec<u8>)>, BucketError>;

    /// Write a result object at `runs/<run_id>/<key>`.
    async fn put_result(&self, run_id: &str, key: &str, body: &[u8]) -> Result<(), BucketError>;

    /// Return all result objects for `run_id`, sorted ascending by key.
    async fn list_results(&self, run_id: &str) -> Result<Vec<(String, Vec<u8>)>, BucketError>;

    /// Write the terminal status string to `runs/<run_id>/finished`.
    async fn put_finished(&self, run_id: &str, status: &str) -> Result<(), BucketError>;

    /// Return the terminal status string for `run_id`, or `None` if the
    /// finished marker has not been written yet.
    async fn get_finished(&self, run_id: &str) -> Result<Option<String>, BucketError>;

    /// Lightweight connectivity probe — succeeds on a reachable bucket,
    /// returns `Err` on a misconfigured or unreachable one.
    async fn probe(&self) -> Result<(), BucketError>;
}

// ── key-layout helpers ────────────────────────────────────────────────────────

/// `jobs/<run_id>.json`
pub(crate) fn key_job(run_id: &str) -> String {
    format!("jobs/{run_id}.json")
}

/// `jobs/<run_id>.claim`
pub(crate) fn key_claim(run_id: &str) -> String {
    format!("jobs/{run_id}.claim")
}

/// `control/<run_id>/<seq:020>.json`
pub(crate) fn key_control(run_id: &str, seq: u64) -> String {
    format!("control/{run_id}/{seq:020}.json")
}

/// Directory prefix for control messages of a run: `control/<run_id>/`
pub(crate) fn prefix_control(run_id: &str) -> String {
    format!("control/{run_id}/")
}

/// `runs/<run_id>/<key>`
pub(crate) fn key_result(run_id: &str, key: &str) -> String {
    format!("runs/{run_id}/{key}")
}

/// Directory prefix for result objects of a run: `runs/<run_id>/`
pub(crate) fn prefix_results(run_id: &str) -> String {
    format!("runs/{run_id}/")
}

/// `runs/<run_id>/finished`
pub(crate) fn key_finished(run_id: &str) -> String {
    format!("runs/{run_id}/finished")
}
