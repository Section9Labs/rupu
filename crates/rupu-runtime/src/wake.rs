use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use thiserror::Error;
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeSource {
    Manual,
    CronPoll,
    Webhook,
    AutoflowDispatch,
    Retry,
    ApprovalResume,
    Repair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeEntityKind {
    Issue,
    Pr,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeEntity {
    pub kind: WakeEntityKind,
    #[serde(rename = "ref")]
    pub ref_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WakeEvent {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub delivery_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dedupe_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WakeRecord {
    pub version: u32,
    pub wake_id: String,
    pub source: WakeSource,
    pub repo_ref: String,
    pub entity: WakeEntity,
    pub event: WakeEvent,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub payload_ref: Option<PathBuf>,
    pub received_at: String,
    pub not_before: String,
}

impl WakeRecord {
    pub const VERSION: u32 = 1;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WakeEnqueueRequest {
    pub source: WakeSource,
    pub repo_ref: String,
    pub entity: WakeEntity,
    pub event: WakeEvent,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub payload: Option<Value>,
    pub received_at: String,
    pub not_before: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DedupeMarker {
    version: u32,
    dedupe_key: String,
    wake_id: String,
    first_seen_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    processed_at: Option<String>,
}

impl DedupeMarker {
    const VERSION: u32 = 1;
}

#[derive(Debug, Error)]
pub enum WakeStoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("wake `{0}` not found")]
    NotFound(String),
    #[error("wake dedupe key already exists: {0}")]
    DuplicateDedupeKey(String),
    #[error("invalid timestamp `{0}`")]
    InvalidTimestamp(String),
}

#[derive(Debug, Clone)]
pub struct WakeStore {
    pub root: PathBuf,
}

impl WakeStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn enqueue(&self, request: WakeEnqueueRequest) -> Result<WakeRecord, WakeStoreError> {
        self.ensure_dirs()?;

        let wake_id = format!("wake_{}", Ulid::new());
        let payload_ref = request
            .payload
            .as_ref()
            .map(|_| self.payload_path(&wake_id));
        let record = WakeRecord {
            version: WakeRecord::VERSION,
            wake_id: wake_id.clone(),
            source: request.source,
            repo_ref: request.repo_ref,
            entity: request.entity,
            event: request.event,
            payload_ref,
            received_at: request.received_at.clone(),
            not_before: request.not_before,
        };

        if let Some(dedupe_key) = record.event.dedupe_key.as_deref() {
            self.create_dedupe_marker(dedupe_key, &wake_id, &request.received_at)?;
        }

        if let Some(payload) = request.payload {
            if let Err(error) = write_atomic_json(&self.payload_path(&wake_id), &payload) {
                self.cleanup_after_enqueue_failure(&record);
                return Err(error.into());
            }
        }

        if let Err(error) = write_atomic_json(&self.queue_path(&wake_id), &record) {
            self.cleanup_after_enqueue_failure(&record);
            return Err(error.into());
        }

        Ok(record)
    }

    pub fn list_due(&self, now: DateTime<Utc>) -> Result<Vec<WakeRecord>, WakeStoreError> {
        self.ensure_dirs()?;
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.queue_dir()) else {
            return Ok(out);
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let record: WakeRecord = serde_json::from_slice(&std::fs::read(&path)?)?;
            let not_before = parse_rfc3339(&record.not_before)?;
            if not_before <= now {
                out.push(record);
            }
        }
        out.sort_by(|left, right| {
            left.not_before
                .cmp(&right.not_before)
                .then_with(|| left.received_at.cmp(&right.received_at))
                .then_with(|| left.wake_id.cmp(&right.wake_id))
        });
        Ok(out)
    }

    pub fn mark_processed(&self, wake_id: &str) -> Result<WakeRecord, WakeStoreError> {
        self.ensure_dirs()?;
        let queue_path = self.queue_path(wake_id);
        if !queue_path.is_file() {
            return Err(WakeStoreError::NotFound(wake_id.to_string()));
        }
        let record: WakeRecord = serde_json::from_slice(&std::fs::read(&queue_path)?)?;
        let processed_path = self.processed_path(wake_id);
        std::fs::rename(&queue_path, &processed_path)?;
        if let Some(dedupe_key) = record.event.dedupe_key.as_deref() {
            self.touch_dedupe_processed_at(dedupe_key)?;
        }
        Ok(record)
    }

    pub fn requeue(
        &self,
        wake_id: &str,
        not_before: DateTime<Utc>,
    ) -> Result<WakeRecord, WakeStoreError> {
        self.ensure_dirs()?;
        let path = self.queue_path(wake_id);
        if !path.is_file() {
            return Err(WakeStoreError::NotFound(wake_id.to_string()));
        }
        let mut record: WakeRecord = serde_json::from_slice(&std::fs::read(&path)?)?;
        record.not_before = not_before.to_rfc3339();
        write_atomic_json(&path, &record)?;
        Ok(record)
    }

    fn ensure_dirs(&self) -> Result<(), WakeStoreError> {
        std::fs::create_dir_all(self.queue_dir())?;
        std::fs::create_dir_all(self.processed_dir())?;
        std::fs::create_dir_all(self.payloads_dir())?;
        std::fs::create_dir_all(self.dedupe_dir())?;
        Ok(())
    }

    fn queue_dir(&self) -> PathBuf {
        self.root.join("queue")
    }

    fn processed_dir(&self) -> PathBuf {
        self.root.join("processed")
    }

    fn payloads_dir(&self) -> PathBuf {
        self.root.join("payloads")
    }

    fn dedupe_dir(&self) -> PathBuf {
        self.root.join("dedupe")
    }

    fn queue_path(&self, wake_id: &str) -> PathBuf {
        self.queue_dir().join(format!("{wake_id}.json"))
    }

    fn processed_path(&self, wake_id: &str) -> PathBuf {
        self.processed_dir().join(format!("{wake_id}.json"))
    }

    fn payload_path(&self, wake_id: &str) -> PathBuf {
        self.payloads_dir().join(format!("{wake_id}.json"))
    }

    fn dedupe_path(&self, dedupe_key: &str) -> PathBuf {
        let digest = Sha256::digest(dedupe_key.as_bytes());
        self.dedupe_dir()
            .join(format!("{}.json", hex::encode(digest)))
    }

    fn create_dedupe_marker(
        &self,
        dedupe_key: &str,
        wake_id: &str,
        first_seen_at: &str,
    ) -> Result<(), WakeStoreError> {
        let path = self.dedupe_path(dedupe_key);
        let marker = DedupeMarker {
            version: DedupeMarker::VERSION,
            dedupe_key: dedupe_key.to_string(),
            wake_id: wake_id.to_string(),
            first_seen_at: first_seen_at.to_string(),
            processed_at: None,
        };
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(WakeStoreError::DuplicateDedupeKey(dedupe_key.to_string()));
            }
            Err(err) => return Err(err.into()),
        };
        serde_json::to_writer_pretty(&mut file, &marker)?;
        Ok(())
    }

    fn touch_dedupe_processed_at(&self, dedupe_key: &str) -> Result<(), WakeStoreError> {
        let path = self.dedupe_path(dedupe_key);
        if !path.is_file() {
            return Ok(());
        }
        let mut marker: DedupeMarker = serde_json::from_slice(&std::fs::read(&path)?)?;
        marker.processed_at = Some(Utc::now().to_rfc3339());
        write_atomic_json(&path, &marker)?;
        Ok(())
    }

    fn cleanup_after_enqueue_failure(&self, record: &WakeRecord) {
        let _ = std::fs::remove_file(self.queue_path(&record.wake_id));
        if let Some(payload_ref) = &record.payload_ref {
            let _ = std::fs::remove_file(payload_ref);
        }
        if let Some(dedupe_key) = record.event.dedupe_key.as_deref() {
            let _ = std::fs::remove_file(self.dedupe_path(dedupe_key));
        }
    }
}

fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<(), std::io::Error> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(value).expect("serialize wake json"),
    )?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>, WakeStoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| WakeStoreError::InvalidTimestamp(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::tempdir;

    fn sample_request() -> WakeEnqueueRequest {
        WakeEnqueueRequest {
            source: WakeSource::Webhook,
            repo_ref: "github:Section9Labs/rupu".into(),
            entity: WakeEntity {
                kind: WakeEntityKind::Issue,
                ref_text: "github:Section9Labs/rupu/issues/42".into(),
            },
            event: WakeEvent {
                id: "github.issue.labeled".into(),
                delivery_id: Some("delivery-123".into()),
                dedupe_key: Some("github:issue:labeled:42:delivery-123".into()),
            },
            payload: Some(serde_json::json!({ "payload": { "issue": { "number": 42 } } })),
            received_at: Utc::now().to_rfc3339(),
            not_before: Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn enqueue_list_due_and_mark_processed_round_trip() {
        let tmp = tempdir().unwrap();
        let store = WakeStore::new(tmp.path().to_path_buf());
        let record = store.enqueue(sample_request()).unwrap();

        let due = store.list_due(Utc::now()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0], record);
        assert!(due[0]
            .payload_ref
            .as_ref()
            .is_some_and(|path| path.is_file()));

        let processed = store.mark_processed(&record.wake_id).unwrap();
        assert_eq!(processed.wake_id, record.wake_id);
        assert!(store.queue_path(&record.wake_id).exists().not());
        assert!(store.processed_path(&record.wake_id).is_file());
    }

    #[test]
    fn enqueue_rejects_duplicate_dedupe_key() {
        let tmp = tempdir().unwrap();
        let store = WakeStore::new(tmp.path().to_path_buf());
        let request = sample_request();
        store.enqueue(request.clone()).unwrap();
        let err = store.enqueue(request).unwrap_err();
        assert!(
            matches!(err, WakeStoreError::DuplicateDedupeKey(key) if key == "github:issue:labeled:42:delivery-123")
        );
    }

    #[test]
    fn requeue_defers_due_visibility() {
        let tmp = tempdir().unwrap();
        let store = WakeStore::new(tmp.path().to_path_buf());
        let record = store.enqueue(sample_request()).unwrap();
        let future = Utc::now() + Duration::minutes(5);
        store.requeue(&record.wake_id, future).unwrap();

        let due_now = store.list_due(Utc::now()).unwrap();
        assert!(due_now.is_empty());
        let due_later = store.list_due(future + Duration::seconds(1)).unwrap();
        assert_eq!(due_later.len(), 1);
        assert_eq!(due_later[0].wake_id, record.wake_id);
    }

    trait BoolExt {
        fn not(self) -> bool;
    }

    impl BoolExt for bool {
        fn not(self) -> bool {
            !self
        }
    }
}
