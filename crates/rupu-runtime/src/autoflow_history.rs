use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoflowCycleMode {
    Tick,
    Serve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoflowCycleEventKind {
    WakeConsumed,
    WakeSkipped,
    ClaimAcquired,
    ClaimReleased,
    ClaimTakeover,
    RunLaunched,
    AwaitingHuman,
    AwaitingExternal,
    RetryScheduled,
    DispatchQueued,
    CleanupPerformed,
    CycleSkipped,
    CycleFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoflowCycleEvent {
    pub kind: AutoflowCycleEventKind,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_display_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub wake_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub wake_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

impl Default for AutoflowCycleEvent {
    fn default() -> Self {
        Self {
            kind: AutoflowCycleEventKind::CycleSkipped,
            issue_ref: None,
            issue_display_ref: None,
            repo_ref: None,
            source_ref: None,
            workflow: None,
            run_id: None,
            wake_id: None,
            wake_event_id: None,
            status: None,
            detail: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoflowCycleRecord {
    pub version: u32,
    pub cycle_id: String,
    pub mode: AutoflowCycleMode,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_filter: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub workflow_count: usize,
    pub polled_event_count: usize,
    pub webhook_event_count: usize,
    pub ran_cycles: usize,
    pub skipped_cycles: usize,
    pub failed_cycles: usize,
    pub cleaned_claims: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<AutoflowCycleEvent>,
}

impl AutoflowCycleRecord {
    pub const VERSION: u32 = 1;

    pub fn new(mode: AutoflowCycleMode, started_at: DateTime<Utc>) -> Self {
        Self {
            version: Self::VERSION,
            cycle_id: format!("afc_{}", Ulid::new()),
            mode,
            worker_id: None,
            worker_name: None,
            repo_filter: None,
            started_at: started_at.to_rfc3339(),
            finished_at: started_at.to_rfc3339(),
            workflow_count: 0,
            polled_event_count: 0,
            webhook_event_count: 0,
            ran_cycles: 0,
            skipped_cycles: 0,
            failed_cycles: 0,
            cleaned_claims: 0,
            events: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum AutoflowHistoryStoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct AutoflowHistoryStore {
    pub root: PathBuf,
}

impl AutoflowHistoryStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn save(&self, record: &AutoflowCycleRecord) -> Result<(), AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        let started_at = parse_rfc3339(&record.started_at)?;
        let day_dir = self
            .cycles_dir()
            .join(started_at.format("%Y-%m-%d").to_string());
        std::fs::create_dir_all(&day_dir)?;
        write_atomic_json(&day_dir.join(format!("{}.json", record.cycle_id)), record)?;
        Ok(())
    }

    pub fn load(
        &self,
        cycle_id: &str,
    ) -> Result<Option<AutoflowCycleRecord>, AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        for day in self.day_dirs()? {
            let path = day.join(format!("{cycle_id}.json"));
            if path.is_file() {
                let body = std::fs::read(path)?;
                return Ok(Some(serde_json::from_slice(&body)?));
            }
        }
        Ok(None)
    }

    pub fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<AutoflowCycleRecord>, AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        let mut out = Vec::new();
        for day in self.day_dirs()? {
            for entry in std::fs::read_dir(day)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let body = std::fs::read(entry.path())?;
                let record: AutoflowCycleRecord = serde_json::from_slice(&body)?;
                out.push(record);
            }
        }
        out.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| right.cycle_id.cmp(&left.cycle_id))
        });
        if out.len() > limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    fn ensure_dirs(&self) -> Result<(), AutoflowHistoryStoreError> {
        std::fs::create_dir_all(self.cycles_dir())?;
        Ok(())
    }

    fn cycles_dir(&self) -> PathBuf {
        self.root.join("cycles")
    }

    fn day_dirs(&self) -> Result<Vec<PathBuf>, AutoflowHistoryStoreError> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(self.cycles_dir())? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                out.push(entry.path());
            }
        }
        out.sort();
        out.reverse();
        Ok(out)
    }
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>, AutoflowHistoryStoreError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(std::io::Error::other)?;
    Ok(parsed.with_timezone(&Utc))
}

fn write_atomic_json<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), AutoflowHistoryStoreError> {
    let tmp = path.with_extension("tmp");
    let body = serde_json::to_vec_pretty(value)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_lists_recent_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());

        let mut older = AutoflowCycleRecord::new(
            AutoflowCycleMode::Tick,
            chrono::DateTime::parse_from_rfc3339("2026-05-11T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        older.finished_at = "2026-05-11T10:00:01Z".into();
        older.workflow_count = 1;
        store.save(&older).unwrap();

        let mut newer = AutoflowCycleRecord::new(
            AutoflowCycleMode::Serve,
            chrono::DateTime::parse_from_rfc3339("2026-05-11T10:05:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        newer.finished_at = "2026-05-11T10:05:02Z".into();
        newer.workflow_count = 2;
        newer.events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::RunLaunched,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            run_id: Some("run_123".into()),
            ..Default::default()
        });
        store.save(&newer).unwrap();

        let recent = store.list_recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].cycle_id, newer.cycle_id);
        assert_eq!(recent[1].cycle_id, older.cycle_id);

        let loaded = store.load(&newer.cycle_id).unwrap().unwrap();
        assert_eq!(loaded.events.len(), 1);
        assert_eq!(loaded.events[0].run_id.as_deref(), Some("run_123"));
    }
}
