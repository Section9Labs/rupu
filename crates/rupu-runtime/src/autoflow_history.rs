use crate::file_cache::{FileCache, FileCacheError};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
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
    IssueCommented,
    IssueStateChanged,
    PullRequestOpened,
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
pub struct AutoflowHistoryEventRecord {
    pub version: u32,
    pub event_id: String,
    pub cycle_id: String,
    pub mode: AutoflowCycleMode,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_filter: Option<String>,
    pub at: String,
    #[serde(flatten)]
    pub event: AutoflowCycleEvent,
}

impl AutoflowHistoryEventRecord {
    pub const VERSION: u32 = 1;

    pub fn from_cycle_event(
        cycle: &AutoflowCycleRecord,
        event: AutoflowCycleEvent,
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            version: Self::VERSION,
            event_id: format!("afe_{}", Ulid::new()),
            cycle_id: cycle.cycle_id.clone(),
            mode: cycle.mode,
            worker_id: cycle.worker_id.clone(),
            worker_name: cycle.worker_name.clone(),
            repo_filter: cycle.repo_filter.clone(),
            at: at.to_rfc3339(),
            event,
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

    pub fn append_event(
        &self,
        record: &AutoflowHistoryEventRecord,
    ) -> Result<(), AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        let at = parse_rfc3339(&record.at)?;
        let day_dir = self.events_dir().join(at.format("%Y-%m-%d").to_string());
        std::fs::create_dir_all(&day_dir)?;
        write_atomic_json(&day_dir.join(format!("{}.json", record.event_id)), record)?;
        Ok(())
    }

    pub fn append_cycle_event(
        &self,
        cycle: &AutoflowCycleRecord,
        event: AutoflowCycleEvent,
        at: DateTime<Utc>,
    ) -> Result<AutoflowHistoryEventRecord, AutoflowHistoryStoreError> {
        let record = AutoflowHistoryEventRecord::from_cycle_event(cycle, event, at);
        self.append_event(&record)?;
        Ok(record)
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

    /// The newest `limit` cycles, newest first (see [`newest_records`]).
    pub fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<AutoflowCycleRecord>, AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        static CACHE: OnceLock<FileCache<AutoflowCycleRecord>> = OnceLock::new();
        newest_records(
            &self.cycles_dir(),
            self.day_dirs()?,
            limit,
            CACHE.get_or_init(FileCache::default),
            |r| (&r.started_at, &r.cycle_id),
        )
    }

    /// The newest `limit` history events, newest first (see
    /// [`newest_records`]).
    pub fn list_recent_events(
        &self,
        limit: usize,
    ) -> Result<Vec<AutoflowHistoryEventRecord>, AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        static CACHE: OnceLock<FileCache<AutoflowHistoryEventRecord>> = OnceLock::new();
        newest_records(
            &self.events_dir(),
            self.event_day_dirs()?,
            limit,
            CACHE.get_or_init(FileCache::default),
            |r| (&r.at, &r.event_id),
        )
    }

    fn ensure_dirs(&self) -> Result<(), AutoflowHistoryStoreError> {
        std::fs::create_dir_all(self.cycles_dir())?;
        std::fs::create_dir_all(self.events_dir())?;
        Ok(())
    }

    fn cycles_dir(&self) -> PathBuf {
        self.root.join("cycles")
    }

    fn events_dir(&self) -> PathBuf {
        self.root.join("events")
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

    fn event_day_dirs(&self) -> Result<Vec<PathBuf>, AutoflowHistoryStoreError> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(self.events_dir())? {
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

/// The newest `limit` records under `day_dirs` (given newest day first),
/// ordered by their timestamp, then id, both descending. `key` yields a
/// record's `(rfc3339 timestamp, id)`; an unparsable timestamp sorts last.
///
/// [`AutoflowHistoryStore::save`] / [`AutoflowHistoryStore::append_event`]
/// file every record under the UTC day of its own timestamp, so every record
/// in an older day sorts after every record in a newer one. Once whole days
/// have yielded `limit` records nothing older can place, and the walk stops
/// there instead of reading the store's entire history (tens of thousands of
/// files for a long-lived tick loop). Only `*.json` files are records — an
/// in-flight atomic write's `.tmp` is not. Parses come from `cache` while a
/// file is unchanged on disk; a file deleted mid-walk is skipped.
fn newest_records<T: DeserializeOwned + Clone>(
    root: &Path,
    day_dirs: Vec<PathBuf>,
    limit: usize,
    cache: &FileCache<T>,
    key: impl Fn(&T) -> (&String, &String),
) -> Result<Vec<T>, AutoflowHistoryStoreError> {
    let mut out: Vec<Arc<T>> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for day in day_dirs {
        if out.len() >= limit {
            break;
        }
        for entry in std::fs::read_dir(day)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file()
                || path.extension().and_then(|e| e.to_str()) != Some("json")
            {
                continue;
            }
            let record = match cache.read(&path, |body| serde_json::from_slice::<T>(body)) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(FileCacheError::Io(e)) => return Err(e.into()),
                Err(FileCacheError::Parse(e)) => return Err(e.into()),
            };
            seen.insert(path);
            out.push(record);
        }
    }
    cache.retain(|p| !p.starts_with(root) || seen.contains(p));

    let sort_key = |r: &T| {
        let (at, id) = key(r);
        (DateTime::parse_from_rfc3339(at).ok(), id.clone())
    };
    let mut keyed: Vec<_> = out.into_iter().map(|r| (sort_key(&r), r)).collect();
    keyed.sort_by(|(left, _), (right, _)| right.cmp(left));
    keyed.truncate(limit);
    Ok(keyed.into_iter().map(|(_, r)| (*r).clone()).collect())
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

    #[test]
    fn appends_and_lists_recent_events() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());

        let cycle = AutoflowCycleRecord::new(
            AutoflowCycleMode::Serve,
            chrono::DateTime::parse_from_rfc3339("2026-05-11T10:05:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        let first = store
            .append_cycle_event(
                &cycle,
                AutoflowCycleEvent {
                    kind: AutoflowCycleEventKind::ClaimAcquired,
                    issue_ref: Some("github:Section9Labs/rupu/issues/1".into()),
                    ..Default::default()
                },
                chrono::DateTime::parse_from_rfc3339("2026-05-11T10:05:01Z")
                    .unwrap()
                    .with_timezone(&Utc),
            )
            .unwrap();
        let second = store
            .append_cycle_event(
                &cycle,
                AutoflowCycleEvent {
                    kind: AutoflowCycleEventKind::IssueCommented,
                    issue_ref: Some("github:Section9Labs/rupu/issues/1".into()),
                    ..Default::default()
                },
                chrono::DateTime::parse_from_rfc3339("2026-05-11T10:05:02Z")
                    .unwrap()
                    .with_timezone(&Utc),
            )
            .unwrap();

        let recent = store.list_recent_events(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].event_id, second.event_id);
        assert_eq!(recent[1].event_id, first.event_id);
        assert_eq!(recent[0].cycle_id, cycle.cycle_id);
        assert_eq!(recent[0].event.kind, AutoflowCycleEventKind::IssueCommented);
    }

    fn cycle_at(store: &AutoflowHistoryStore, at: &str) -> AutoflowCycleRecord {
        let record = AutoflowCycleRecord::new(
            AutoflowCycleMode::Tick,
            chrono::DateTime::parse_from_rfc3339(at)
                .unwrap()
                .with_timezone(&Utc),
        );
        store.save(&record).unwrap();
        record
    }

    #[test]
    fn list_recent_spans_days_newest_first_and_stops_at_a_whole_day() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        let d1_a = cycle_at(&store, "2026-05-01T09:00:00Z");
        let d2_a = cycle_at(&store, "2026-05-02T08:00:00Z");
        let d2_b = cycle_at(&store, "2026-05-02T23:59:59.5Z");
        let d3_a = cycle_at(&store, "2026-05-03T00:00:00Z");
        // A garbled record in the oldest day: a full scan of history would
        // fail on it, so listing the newest records proves the walk never
        // opened that day.
        let oldest = tmp.path().join("cycles").join("2026-04-30");
        std::fs::create_dir_all(&oldest).unwrap();
        std::fs::write(oldest.join("afc_garbled.json"), "not json").unwrap();

        let ids = |v: Vec<AutoflowCycleRecord>| -> Vec<String> {
            v.into_iter().map(|r| r.cycle_id).collect()
        };
        assert_eq!(
            ids(store.list_recent(1).unwrap()),
            vec![d3_a.cycle_id.clone()]
        );
        assert_eq!(
            ids(store.list_recent(2).unwrap()),
            vec![d3_a.cycle_id.clone(), d2_b.cycle_id.clone()]
        );
        assert_eq!(
            ids(store.list_recent(4).unwrap()),
            vec![
                d3_a.cycle_id.clone(),
                d2_b.cycle_id.clone(),
                d2_a.cycle_id.clone(),
                d1_a.cycle_id.clone()
            ]
        );
        assert!(store.list_recent(0).unwrap().is_empty());
        // Asking for more than the clean days hold must reach the bad day,
        // and a bad record is an error, as before.
        assert!(store.list_recent(5).is_err());
    }

    #[test]
    fn list_recent_ignores_in_flight_tmp_files_and_ties_break_on_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        let a = cycle_at(&store, "2026-05-02T08:00:00Z");
        let b = cycle_at(&store, "2026-05-02T08:00:00Z");
        // A concurrent `save` caught between its write and its rename.
        let day = tmp.path().join("cycles").join("2026-05-02");
        std::fs::write(day.join("afc_inflight.tmp"), "{\"half\":").unwrap();

        let got: Vec<String> = store
            .list_recent(10)
            .unwrap()
            .into_iter()
            .map(|r| r.cycle_id)
            .collect();
        let mut want = vec![a.cycle_id, b.cycle_id];
        want.sort();
        want.reverse();
        assert_eq!(got, want);
    }

    #[test]
    fn a_rewritten_cycle_lists_its_new_content() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        let mut record = cycle_at(&store, "2026-05-02T08:00:00Z");
        assert_eq!(store.list_recent(1).unwrap()[0].ran_cycles, 0);
        record.ran_cycles = 7;
        store.save(&record).unwrap();
        assert_eq!(store.list_recent(1).unwrap()[0].ran_cycles, 7);
    }

    #[test]
    fn list_recent_events_stops_at_a_whole_day() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Serve, Utc::now());
        let at = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&Utc)
        };
        let old = store
            .append_cycle_event(
                &cycle,
                AutoflowCycleEvent::default(),
                at("2026-05-01T10:00:00Z"),
            )
            .unwrap();
        let new = store
            .append_cycle_event(
                &cycle,
                AutoflowCycleEvent::default(),
                at("2026-05-02T10:00:00Z"),
            )
            .unwrap();
        let oldest = tmp.path().join("events").join("2026-04-30");
        std::fs::create_dir_all(&oldest).unwrap();
        std::fs::write(oldest.join("afe_garbled.json"), "not json").unwrap();

        let got: Vec<String> = store
            .list_recent_events(2)
            .unwrap()
            .into_iter()
            .map(|r| r.event_id)
            .collect();
        assert_eq!(got, vec![new.event_id, old.event_id]);
    }
}
