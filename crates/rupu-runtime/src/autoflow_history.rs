use crate::file_cache::{FileCache, FileCacheError};
use chrono::{DateTime, NaiveDate, Utc};
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

    /// True when the cycle did nothing worth keeping: it launched, failed and
    /// cleaned up nothing, consumed no polled or webhook events, and recorded
    /// no event — including no `cycle_failed`, which is how a tick records an
    /// error it continued past. `workflow_count` / `skipped_cycles` don't
    /// count: "evaluated N issues, none due" is the steady state of an idle
    /// tick loop. The writer does not persist idle cycles.
    pub fn is_idle(&self) -> bool {
        self.ran_cycles == 0
            && self.failed_cycles == 0
            && self.cleaned_claims == 0
            && self.polled_event_count == 0
            && self.webhook_event_count == 0
            && self.events.is_empty()
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

    /// The newest `limit` cycles, newest first (see [`records_in`]).
    pub fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<AutoflowCycleRecord>, AutoflowHistoryStoreError> {
        self.list_cycles(HistoryWindow {
            limit: Some(limit),
            ..HistoryWindow::default()
        })
    }

    /// The cycles that started inside `window`, newest first (see
    /// [`records_in`]).
    pub fn list_cycles(
        &self,
        window: HistoryWindow,
    ) -> Result<Vec<AutoflowCycleRecord>, AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        static CACHE: OnceLock<FileCache<AutoflowCycleRecord>> = OnceLock::new();
        records_in(
            &self.cycles_dir(),
            self.day_dirs()?,
            window,
            CACHE.get_or_init(FileCache::default),
            |r| (&r.started_at, &r.cycle_id),
            |_| true,
        )
    }

    /// The newest `limit` history events, newest first (see
    /// [`records_in`]).
    pub fn list_recent_events(
        &self,
        limit: usize,
    ) -> Result<Vec<AutoflowHistoryEventRecord>, AutoflowHistoryStoreError> {
        self.list_events(
            HistoryWindow {
                limit: Some(limit),
                ..HistoryWindow::default()
            },
            |_| true,
        )
    }

    /// The history events inside `window` that `keep` accepts, newest first
    /// (see [`records_in`]). `window.limit` counts only accepted events.
    pub fn list_events(
        &self,
        window: HistoryWindow,
        keep: impl Fn(&AutoflowHistoryEventRecord) -> bool,
    ) -> Result<Vec<AutoflowHistoryEventRecord>, AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        static CACHE: OnceLock<FileCache<AutoflowHistoryEventRecord>> = OnceLock::new();
        records_in(
            &self.events_dir(),
            self.event_day_dirs()?,
            window,
            CACHE.get_or_init(FileCache::default),
            |r| (&r.at, &r.event_id),
            keep,
        )
    }

    /// Delete every day of cycles and events filed before `cutoff` (a UTC
    /// date): the retention policy behind `[autoflow].history_retention_days`.
    ///
    /// Whole day directories go at once — `save` / `append_event` file each
    /// record under the UTC day of its own timestamp, so no record is ever
    /// opened to decide. Only a directory whose name is a `YYYY-MM-DD` date
    /// is touched; anything else under `cycles/` or `events/` is left alone.
    /// A day another pruner already removed is not a failure. A day that
    /// cannot be removed is reported in [`HistoryPruneReport::failures`] and
    /// does not stop the rest from being pruned.
    pub fn prune_before(
        &self,
        cutoff: NaiveDate,
    ) -> Result<HistoryPruneReport, AutoflowHistoryStoreError> {
        self.ensure_dirs()?;
        let mut report = HistoryPruneReport::default();
        for (dirs, removed) in [
            (self.day_dirs()?, &mut report.cycle_days),
            (self.event_day_dirs()?, &mut report.event_days),
        ] {
            for day in dirs {
                if !day_of(&day).is_some_and(|date| date < cutoff) {
                    continue;
                }
                match std::fs::remove_dir_all(&day) {
                    Ok(()) => *removed += 1,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => report.failures.push((day, e)),
                }
            }
        }
        Ok(report)
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

/// Which records a listing returns: those stamped inside `[since, until]`
/// (both ends inclusive; an absent bound imposes nothing on its side), at
/// most `limit` of them, newest first. `limit: None` is every record in
/// range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryWindow {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

impl HistoryWindow {
    fn is_bounded(&self) -> bool {
        self.since.is_some() || self.until.is_some()
    }

    fn contains(&self, at: DateTime<Utc>) -> bool {
        self.since.is_none_or(|since| at >= since) && self.until.is_none_or(|until| at <= until)
    }
}

/// What [`AutoflowHistoryStore::prune_before`] removed: whole days of cycles
/// and of events, plus any day it could not remove.
#[derive(Debug, Default)]
pub struct HistoryPruneReport {
    pub cycle_days: usize,
    pub event_days: usize,
    pub failures: Vec<(PathBuf, std::io::Error)>,
}

/// The UTC date a day directory holds, or `None` for a name that is not one.
fn day_of(dir: &Path) -> Option<NaiveDate> {
    let name = dir.file_name()?.to_str()?;
    NaiveDate::parse_from_str(name, "%Y-%m-%d").ok()
}

/// The records under `day_dirs` (given newest day first) that fall inside
/// `window` and that `keep` accepts, ordered by their timestamp, then id,
/// both descending. `key` yields a record's `(rfc3339 timestamp, id)`. An
/// unparsable timestamp sorts last, and is excluded once `window` has a
/// bound — there is no honest basis to place it inside one.
///
/// [`AutoflowHistoryStore::save`] / [`AutoflowHistoryStore::append_event`]
/// file every record under the UTC day of its own timestamp, so every record
/// in an older day sorts after every record in a newer one. Days entirely
/// outside `window` are never opened, and once whole days have yielded
/// `window.limit` accepted records nothing older can place, so the walk
/// stops there instead of reading the store's entire history. Only `*.json`
/// files are records — an in-flight atomic write's `.tmp` is not.
///
/// Parses come from `cache` while a file is unchanged on disk. A file deleted
/// mid-walk is skipped, and so is a whole day that [`prune_before`] removed
/// between the listing and the walk — never an error, which callers would
/// otherwise read as an empty store. The cache drops only the parses of days
/// that no longer exist, not every day this walk did not reach: listings over
/// different windows (the newest page vs a 30-day count) would otherwise keep
/// evicting each other's parses.
///
/// [`prune_before`]: AutoflowHistoryStore::prune_before
fn records_in<T: DeserializeOwned + Clone>(
    root: &Path,
    day_dirs: Vec<PathBuf>,
    window: HistoryWindow,
    cache: &FileCache<T>,
    key: impl Fn(&T) -> (&String, &String),
    keep: impl Fn(&T) -> bool,
) -> Result<Vec<T>, AutoflowHistoryStoreError> {
    let timestamp = |r: &T| {
        DateTime::parse_from_rfc3339(key(r).0)
            .ok()
            .map(|at| at.with_timezone(&Utc))
    };
    let first_day = window.since.map(|since| since.date_naive());
    let last_day = window.until.map(|until| until.date_naive());
    let mut out: Vec<Arc<T>> = Vec::new();
    for day in &day_dirs {
        if window.limit.is_some_and(|limit| out.len() >= limit) {
            break;
        }
        if let Some(date) = day_of(day) {
            if last_day.is_some_and(|last| date > last) {
                continue;
            }
            if first_day.is_some_and(|first| date < first) {
                break;
            }
        }
        let entries = match std::fs::read_dir(day) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
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
            let in_window = match timestamp(&record) {
                Some(at) => window.contains(at),
                None => !window.is_bounded(),
            };
            if in_window && keep(&record) {
                out.push(record);
            }
        }
    }
    let live_days: HashSet<&Path> = day_dirs.iter().map(PathBuf::as_path).collect();
    cache.retain(|p| !p.starts_with(root) || p.parent().is_some_and(|d| live_days.contains(d)));

    let mut keyed: Vec<_> = out
        .into_iter()
        .map(|r| ((timestamp(&r), key(&r).1.clone()), r))
        .collect();
    keyed.sort_by(|(left, _), (right, _)| right.cmp(left));
    if let Some(limit) = window.limit {
        keyed.truncate(limit);
    }
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

    fn utc(at: &str) -> DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn cycle_ids(records: Vec<AutoflowCycleRecord>) -> Vec<String> {
        records.into_iter().map(|r| r.cycle_id).collect()
    }

    fn garble(store: &AutoflowHistoryStore, kind: &str, day: &str) {
        let dir = store.root.join(kind).join(day);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("garbled.json"), "not json").unwrap();
    }

    #[test]
    fn a_cycle_is_idle_only_when_it_did_and_recorded_nothing() {
        let mut evaluated = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, Utc::now());
        assert!(evaluated.is_idle());
        // "Evaluated these issues, none due" is the idle steady state.
        evaluated.workflow_count = 3;
        evaluated.skipped_cycles = 9;
        assert!(evaluated.is_idle());

        let busy: [fn(&mut AutoflowCycleRecord); 6] = [
            |r| r.ran_cycles = 1,
            |r| r.failed_cycles = 1,
            |r| r.cleaned_claims = 1,
            |r| r.polled_event_count = 1,
            |r| r.webhook_event_count = 1,
            |r| {
                r.events.push(AutoflowCycleEvent {
                    kind: AutoflowCycleEventKind::CycleFailed,
                    detail: Some("failed to poll autoflow wake events".into()),
                    ..Default::default()
                })
            },
        ];
        for mark in busy {
            let mut record = evaluated.clone();
            mark(&mut record);
            assert!(!record.is_idle(), "{record:?}");
        }
    }

    #[test]
    fn list_cycles_returns_exactly_the_window_without_opening_days_outside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        cycle_at(&store, "2026-05-01T23:59:59Z");
        cycle_at(&store, "2026-05-02T05:59:59Z");
        let at_since = cycle_at(&store, "2026-05-02T06:00:00Z");
        let middle = cycle_at(&store, "2026-05-03T12:00:00Z");
        let at_until = cycle_at(&store, "2026-05-04T18:00:00Z");
        cycle_at(&store, "2026-05-04T18:00:01Z");
        // Days wholly outside the window: a walk that opened either fails.
        garble(&store, "cycles", "2026-04-30");
        garble(&store, "cycles", "2026-05-05");

        let window = HistoryWindow {
            since: Some(utc("2026-05-02T06:00:00Z")),
            until: Some(utc("2026-05-04T18:00:00Z")),
            limit: None,
        };
        assert_eq!(
            cycle_ids(store.list_cycles(window).unwrap()),
            vec![at_until.cycle_id, middle.cycle_id, at_since.cycle_id]
        );
    }

    #[test]
    fn list_cycles_limit_stops_at_a_whole_day_inside_the_window() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        garble(&store, "cycles", "2026-05-01");
        let older = cycle_at(&store, "2026-05-02T08:00:00Z");
        let newer = cycle_at(&store, "2026-05-02T09:00:00Z");
        garble(&store, "cycles", "2026-05-03");

        let window = HistoryWindow {
            until: Some(utc("2026-05-02T23:59:59Z")),
            limit: Some(1),
            ..HistoryWindow::default()
        };
        assert_eq!(
            cycle_ids(store.list_cycles(window).unwrap()),
            vec![newer.cycle_id.clone()]
        );
        let window = HistoryWindow {
            limit: Some(2),
            ..window
        };
        assert_eq!(
            cycle_ids(store.list_cycles(window).unwrap()),
            vec![newer.cycle_id, older.cycle_id]
        );
    }

    #[test]
    fn list_events_limit_counts_only_kept_events() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, Utc::now());
        let launched = store
            .append_cycle_event(
                &cycle,
                AutoflowCycleEvent {
                    kind: AutoflowCycleEventKind::RunLaunched,
                    run_id: Some("run_1".into()),
                    ..Default::default()
                },
                utc("2026-05-01T10:00:00Z"),
            )
            .unwrap();
        for hour in 10..13 {
            store
                .append_cycle_event(
                    &cycle,
                    AutoflowCycleEvent {
                        kind: AutoflowCycleEventKind::WakeConsumed,
                        ..Default::default()
                    },
                    utc(&format!("2026-05-02T{hour}:00:00Z")),
                )
                .unwrap();
        }

        // Newest-N-then-filter would find no launch among the newest 1.
        let window = HistoryWindow {
            limit: Some(1),
            ..HistoryWindow::default()
        };
        let got = store
            .list_events(window, |r| {
                r.event.kind == AutoflowCycleEventKind::RunLaunched
            })
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event_id, launched.event_id);
    }

    #[test]
    fn a_bounded_window_excludes_a_record_whose_timestamp_does_not_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        let good = cycle_at(&store, "2026-05-02T08:00:00Z");
        let mut bad = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, Utc::now());
        bad.started_at = "not a time".into();
        let day = tmp.path().join("cycles").join("2026-05-02");
        std::fs::write(
            day.join(format!("{}.json", bad.cycle_id)),
            serde_json::to_vec(&bad).unwrap(),
        )
        .unwrap();

        assert_eq!(
            cycle_ids(store.list_cycles(HistoryWindow::default()).unwrap()),
            vec![good.cycle_id.clone(), bad.cycle_id]
        );
        let window = HistoryWindow {
            since: Some(utc("2026-05-01T00:00:00Z")),
            ..HistoryWindow::default()
        };
        assert_eq!(
            cycle_ids(store.list_cycles(window).unwrap()),
            vec![good.cycle_id]
        );
    }

    fn walk(
        store: &AutoflowHistoryStore,
        day_dirs: Vec<PathBuf>,
        limit: Option<usize>,
        cache: &FileCache<AutoflowCycleRecord>,
    ) -> Vec<String> {
        let window = HistoryWindow {
            limit,
            ..HistoryWindow::default()
        };
        cycle_ids(
            records_in(
                &store.cycles_dir(),
                day_dirs,
                window,
                cache,
                |r| (&r.started_at, &r.cycle_id),
                |_| true,
            )
            .unwrap(),
        )
    }

    #[test]
    fn a_day_pruned_between_listing_and_walk_is_skipped_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        let kept = cycle_at(&store, "2026-05-02T08:00:00Z");
        let day_dirs = vec![
            store.cycles_dir().join("2026-05-03"),
            store.cycles_dir().join("2026-05-02"),
        ];
        let cache = FileCache::with_settle(std::time::Duration::ZERO);
        assert_eq!(walk(&store, day_dirs, None, &cache), vec![kept.cycle_id]);
    }

    #[test]
    fn a_narrow_walk_keeps_other_days_parses_and_a_pruned_day_drops_them() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        for at in [
            "2026-05-01T08:00:00Z",
            "2026-05-02T08:00:00Z",
            "2026-05-03T08:00:00Z",
        ] {
            cycle_at(&store, at);
        }
        let cache = FileCache::with_settle(std::time::Duration::ZERO);
        assert_eq!(
            walk(&store, store.day_dirs().unwrap(), None, &cache).len(),
            3
        );
        assert_eq!(cache.len(), 3);
        assert_eq!(
            walk(&store, store.day_dirs().unwrap(), Some(1), &cache).len(),
            1
        );
        assert_eq!(cache.len(), 3, "the newest page evicted the older days");

        store
            .prune_before(NaiveDate::from_ymd_opt(2026, 5, 2).unwrap())
            .unwrap();
        assert_eq!(
            walk(&store, store.day_dirs().unwrap(), Some(1), &cache).len(),
            1
        );
        assert_eq!(cache.len(), 2, "a pruned day's parses outlived it");
    }

    #[test]
    fn prune_before_removes_only_dated_days_older_than_the_cutoff() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
        for at in [
            "2026-05-01T23:00:00Z",
            "2026-05-02T00:00:00Z",
            "2026-05-03T08:00:00Z",
        ] {
            let cycle = cycle_at(&store, at);
            store
                .append_cycle_event(&cycle, AutoflowCycleEvent::default(), utc(at))
                .unwrap();
        }
        let notes = tmp.path().join("cycles").join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("keep.json"), "{}").unwrap();
        std::fs::write(tmp.path().join("cycles").join("README"), "keep").unwrap();

        let cutoff = NaiveDate::from_ymd_opt(2026, 5, 2).unwrap();
        let report = store.prune_before(cutoff).unwrap();
        assert_eq!((report.cycle_days, report.event_days), (1, 1));
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        for kind in ["cycles", "events"] {
            assert!(!tmp.path().join(kind).join("2026-05-01").exists());
            assert!(tmp.path().join(kind).join("2026-05-02").is_dir());
            assert!(tmp.path().join(kind).join("2026-05-03").is_dir());
        }
        assert!(notes.join("keep.json").is_file());
        assert!(tmp.path().join("cycles").join("README").is_file());

        let again = store.prune_before(cutoff).unwrap();
        assert_eq!((again.cycle_days, again.event_days), (0, 0));
    }
}
