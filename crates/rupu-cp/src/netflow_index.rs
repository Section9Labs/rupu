//! `cp serve`'s netflow index — every local ledger read goes through here.
//!
//! Spec: docs/superpowers/specs/2026-10-06-rupu-netflow-progressive-loading-design.md.
//!
//! A per-file drop-in for `rupu_netflow::ledger::read_flows_in_range` /
//! `read_capture_states`: callers keep listing ledger files exactly as
//! before and ask [`LedgerReader`] for each one, so every response keeps its
//! iteration order and values. Each file has an entry with two tiers:
//!
//! - **tier 1** ([`LedgerSummary`]) — always resident: counts, dropped
//!   total, capture lines, time bounds, a `(ts, is_error)` point per flow and
//!   the distinct origins / peer IPs. Enough for every whole-history view.
//! - **tier 2** — the file's rows as [`CompactRows`] plus the fold state
//!   needed to apply later completions. Budgeted (Task 5): evicted rows are
//!   re-read on demand.
//!
//! A file is `stat`ed on every call. An unchanged stamp is answered from
//! memory; a changed one is re-checked against the prefix recorded at the
//! last read (ledgers are append-only) and then tailed from the stored
//! offset, or re-read in full if it was rewritten, shrank, or its rows were
//! evicted. Concurrent calls for one file serialize on that file's lock.

use chrono::{DateTime, Utc};
use rupu_netflow::ledger::explorer::{is_error_outcome, origin_key, HistPoint};
use rupu_netflow::ledger::{
    read_capture_states, read_flows_in_range, split_complete_lines, CaptureEntry, CompactRows,
    FoldEvent, LedgerFold, TimeRange,
};
use rupu_netflow::FlowRecord;
use rupu_runtime::file_cache::FileStamp;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_BUDGET_MB: u64 = 256;

/// Bytes of a file's head kept to detect an in-place rewrite.
const PREFIX_LEN: usize = 64;

/// A per-file ledger read. Implemented by the index and by
/// [`DirectReader`] (no caching — the CLI and the equivalence tests).
pub trait LedgerReader: Send + Sync {
    /// Flows with `ts` in `range`, in file order, and the file's WHOLE
    /// dropped total (never window-scoped). A missing or unreadable file
    /// reads as `(vec![], 0)`.
    fn flows_in_range(&self, path: &Path, range: &TimeRange) -> (Vec<FlowRecord>, u64);
    /// Every capture line, in file order; missing/unreadable → empty.
    fn capture_states(&self, path: &Path) -> Vec<CaptureEntry>;
}

/// Reads the file on every call — exactly the pre-index behaviour.
pub struct DirectReader;

impl LedgerReader for DirectReader {
    fn flows_in_range(&self, path: &Path, range: &TimeRange) -> (Vec<FlowRecord>, u64) {
        read_flows_in_range(path, range).unwrap_or_default()
    }

    fn capture_states(&self, path: &Path) -> Vec<CaptureEntry> {
        read_capture_states(path).unwrap_or_default()
    }
}

/// A snapshot of the index's size, for diagnostics.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct IndexStatus {
    pub files: u64,
    pub flows: u64,
    pub tier1_bytes: u64,
    pub tier2_bytes: u64,
    pub budget_bytes: u64,
    pub resident_files: u64,
    pub evictions_total: u64,
}

/// Tier 1: what a whole-history view needs from one ledger.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LedgerSummary {
    pub flow_count: usize,
    pub dropped: u64,
    pub min_ts: Option<DateTime<Utc>>,
    pub max_ts: Option<DateTime<Utc>>,
    /// `(ts, is_error)` per flow, in file order.
    pub points: Vec<HistPoint>,
    /// Distinct origin filter keys (`explorer::origin_key`).
    pub origins: BTreeSet<String>,
    /// Distinct peer IPs; `None` = a flow with no peer IP.
    pub peer_ips: BTreeSet<Option<IpAddr>>,
    pub capture: Vec<CaptureEntry>,
}

impl LedgerSummary {
    /// Whether any flow could fall in `range` (false for a file with no flows).
    pub fn overlaps(&self, range: &TimeRange) -> bool {
        match (self.min_ts, self.max_ts) {
            (Some(lo), Some(hi)) => {
                range.from.is_none_or(|f| hi >= f) && range.to.is_none_or(|t| lo <= t)
            }
            _ => false,
        }
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.points.capacity() * std::mem::size_of::<HistPoint>()
            + self.origins.iter().map(|o| o.len() + 48).sum::<usize>()
            + self.peer_ips.len() * 48
            + self.capture.len() * 96
    }

    fn apply(&mut self, ev: &FoldEvent) {
        match ev {
            FoldEvent::Flow(f) => {
                self.flow_count += 1;
                self.min_ts = Some(self.min_ts.map_or(f.ts, |m| m.min(f.ts)));
                self.max_ts = Some(self.max_ts.map_or(f.ts, |m| m.max(f.ts)));
                self.points.push((f.ts, is_error_outcome(f.outcome)));
                self.origins.insert(origin_key(&f.ctx.origin));
                self.peer_ips.insert(f.peer_ip);
            }
            FoldEvent::Patch { index, patch } => {
                if let Some(o) = patch.outcome {
                    self.points[*index].1 = is_error_outcome(o);
                }
            }
            FoldEvent::Dropped(n) => self.dropped += n,
            FoldEvent::Capture(c) => self.capture.push(c.clone()),
        }
    }
}

/// Tier 2: one file's rows plus the fold state later completions need.
struct Resident {
    rows: CompactRows,
    fold: LedgerFold,
    /// Bytes currently counted for this entry in `NetflowIndex::resident_bytes`.
    accounted: u64,
}

impl Resident {
    fn bytes(&self) -> u64 {
        (self.rows.heap_bytes() + self.fold.heap_bytes()) as u64
    }
}

#[derive(Default)]
struct Entry {
    stamp: Option<FileStamp>,
    ino: u64,
    offset: u64,
    prefix: Vec<u8>,
    /// The file held non-UTF-8 bytes: it reads as empty, like the direct
    /// reader's `lines()` failure, until it is rewritten.
    unreadable: bool,
    summary: Arc<LedgerSummary>,
    resident: Option<Resident>,
    /// Taken out of the map (set under this entry's lock). A removed entry
    /// never admits rows or records state: a reader that fetched it just
    /// before removal answers directly from the file instead, so no bytes
    /// are ever accounted to an entry `enforce_budget` can no longer reach.
    removed: bool,
}

/// The entry a reader fetched was removed from the map in the meantime.
struct Removed;

pub struct NetflowIndex {
    entries: Mutex<HashMap<PathBuf, Arc<Mutex<Entry>>>>,
    resident_bytes: AtomicU64,
    budget_bytes: AtomicU64,
    evictions: AtomicU64,
    last_sweep: Mutex<Option<Instant>>,
}

/// How often `sweep_missing` may actually stat every indexed path.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

#[cfg(unix)]
fn inode(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

#[cfg(not(unix))]
fn inode(_meta: &std::fs::Metadata) -> u64 {
    0
}

impl NetflowIndex {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            resident_bytes: AtomicU64::new(0),
            budget_bytes: AtomicU64::new(budget_bytes),
            evictions: AtomicU64::new(0),
            last_sweep: Mutex::new(None),
        }
    }

    /// Tier 1 for one ledger, refreshed; `None` when the file is gone.
    pub fn summary(&self, path: &Path) -> Option<Arc<LedgerSummary>> {
        // An entry removed between fetching it and locking it is replaced in
        // the map by a fresh one: re-fetch once, then give up (None).
        for _ in 0..2 {
            let entry = self.entry(path);
            if let Ok(out) = self.summary_in(&entry, path) {
                return out;
            }
        }
        None
    }

    fn summary_in(
        &self,
        entry: &Arc<Mutex<Entry>>,
        path: &Path,
    ) -> Result<Option<Arc<LedgerSummary>>, Removed> {
        let out = {
            let mut e = entry.lock().unwrap_or_else(|p| p.into_inner());
            if e.removed {
                return Err(Removed);
            }
            match self.refresh(&mut e, path) {
                Refresh::Gone => None,
                // Unreadable right now: serve the last consistent tier 1
                // if the entry ever loaded; the next call retries.
                Refresh::Failed => e.stamp.map(|_| Arc::clone(&e.summary)),
                Refresh::Current => Some(Arc::clone(&e.summary)),
            }
        };
        if out.is_none() {
            self.remove(path);
        }
        self.enforce_budget();
        Ok(out)
    }

    fn entry(&self, path: &Path) -> Arc<Mutex<Entry>> {
        let mut map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        Arc::clone(map.entry(path.to_path_buf()).or_default())
    }

    fn remove(&self, path: &Path) {
        let removed = self
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(path);
        if let Some(entry) = removed {
            self.retire(&entry);
        }
    }

    /// Mark a map-removed entry and release its rows (blocking on its lock;
    /// call without the map lock held).
    fn retire(&self, entry: &Arc<Mutex<Entry>>) {
        let mut e = entry.lock().unwrap_or_else(|p| p.into_inner());
        e.removed = true;
        self.drop_resident(&mut e);
    }

    fn drop_resident(&self, e: &mut Entry) {
        if let Some(r) = e.resident.take() {
            self.resident_bytes
                .fetch_sub(r.accounted, Ordering::Relaxed);
        }
    }

    fn account(&self, e: &mut Entry) {
        if let Some(r) = e.resident.as_mut() {
            let now = r.bytes();
            if now >= r.accounted {
                self.resident_bytes
                    .fetch_add(now - r.accounted, Ordering::Relaxed);
            } else {
                self.resident_bytes
                    .fetch_sub(r.accounted - now, Ordering::Relaxed);
            }
            r.accounted = now;
        }
    }

    /// Bring `e` up to date with the file. A failed read leaves the entry
    /// exactly as it was, with its old stamp, so the next call retries.
    fn refresh(&self, e: &mut Entry, path: &Path) -> Refresh {
        let Ok(meta) = std::fs::metadata(path) else {
            return Refresh::Gone;
        };
        let stamp = FileStamp::of(&meta);
        if e.stamp == Some(stamp) {
            // Unchanged file: tier 1 is current; tier 2 may be evicted,
            // which `ensure_rows` handles.
            return Refresh::Current;
        }
        let ino = inode(&meta);
        let can_tail = e.stamp.is_some()
            && !e.unreadable
            && e.resident.is_some()
            && ino == e.ino
            && meta.len() >= e.offset
            && self.prefix_matches(path, &e.prefix);
        let read = if can_tail {
            self.tail(e, path)
        } else {
            self.full_read(e, path)
        };
        if read.is_err() {
            return Refresh::Failed;
        }
        e.stamp = Some(stamp);
        e.ino = ino;
        Refresh::Current
    }

    fn prefix_matches(&self, path: &Path, prefix: &[u8]) -> bool {
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        let mut buf = vec![0u8; prefix.len()];
        f.read_exact(&mut buf).is_ok() && buf == prefix
    }

    /// Read the file from byte `offset` to its end.
    fn read_from(path: &Path, offset: u64) -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        let mut f = std::fs::File::open(path)?;
        f.seek(SeekFrom::Start(offset))?;
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Rebuild the entry from the whole file. The file is read BEFORE the
    /// entry is touched: on a read error the entry is left exactly as it was.
    fn full_read(&self, e: &mut Entry, path: &Path) -> std::io::Result<()> {
        let buf = Self::read_from(path, 0)?;
        self.drop_resident(e);
        e.offset = 0;
        e.prefix.clear();
        e.unreadable = false;
        e.summary = Arc::new(LedgerSummary::default());
        e.resident = Some(Resident {
            rows: CompactRows::default(),
            fold: LedgerFold::default(),
            accounted: 0,
        });
        self.ingest(e, &buf);
        Ok(())
    }

    /// Feed everything after `e.offset` that ends in a newline. The read
    /// happens before any mutation: on error the entry is untouched.
    fn tail(&self, e: &mut Entry, path: &Path) -> std::io::Result<()> {
        let buf = Self::read_from(path, e.offset)?;
        self.ingest(e, &buf);
        Ok(())
    }

    /// Fold `buf` (the bytes from `e.offset`) into the entry.
    fn ingest(&self, e: &mut Entry, buf: &[u8]) {
        let (complete, used) = split_complete_lines(buf);
        let Ok(text) = std::str::from_utf8(complete) else {
            self.drop_resident(e);
            e.unreadable = true;
            e.summary = Arc::new(LedgerSummary::default());
            return;
        };
        if e.prefix.len() < PREFIX_LEN && e.offset == 0 {
            e.prefix = buf[..buf.len().min(PREFIX_LEN)].to_vec();
        }
        let summary = Arc::make_mut(&mut e.summary);
        let resident = e.resident.as_mut().expect("ingest runs with rows resident");
        for line in text.lines() {
            let Some(ev) = resident.fold.feed_line(line) else {
                continue;
            };
            summary.apply(&ev);
            match ev {
                FoldEvent::Flow(f) => resident.rows.push(*f),
                FoldEvent::Patch { index, patch } => resident.rows.patch(index, &patch),
                FoldEvent::Dropped(_) | FoldEvent::Capture(_) => {}
            }
        }
        e.offset += used as u64;
        self.account(e);
    }

    /// Make sure tier 2 is resident, re-reading the file if it was evicted.
    /// `false` = the re-read failed; tier 1 is left intact.
    fn ensure_rows(&self, e: &mut Entry, path: &Path) -> bool {
        if e.resident.is_none() && !e.unreadable {
            return self.full_read(e, path).is_ok();
        }
        true
    }

    pub fn set_budget_bytes(&self, bytes: u64) {
        self.budget_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Drop tier 2 of the files with the oldest newest-flow first until the
    /// resident rows fit the budget. Entries locked by an in-flight read are
    /// skipped this pass (they are re-checked on the next call). Only
    /// `try_lock` is used while the map lock is held, so this never blocks
    /// behind a read.
    fn enforce_budget(&self) {
        let budget = self.budget_bytes.load(Ordering::Relaxed);
        if self.resident_bytes.load(Ordering::Relaxed) <= budget {
            return;
        }
        let mut candidates: Vec<_> = {
            let map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
            map.values()
                .filter_map(|entry| {
                    let e = entry.try_lock().ok()?;
                    e.resident.as_ref()?;
                    Some((e.summary.max_ts, Arc::clone(entry)))
                })
                .collect()
        };
        candidates.sort_by_key(|(max_ts, _)| *max_ts);
        let mut evicted = 0u64;
        let mut freed = 0u64;
        for (_, entry) in candidates {
            if self.resident_bytes.load(Ordering::Relaxed) <= budget {
                break;
            }
            if let Ok(mut e) = entry.try_lock() {
                if let Some(r) = e.resident.take() {
                    self.resident_bytes
                        .fetch_sub(r.accounted, Ordering::Relaxed);
                    freed += r.accounted;
                    evicted += 1;
                }
            }
        }
        if evicted > 0 {
            self.evictions.fetch_add(evicted, Ordering::Relaxed);
            tracing::debug!(
                evicted,
                freed_bytes = freed,
                budget_bytes = budget,
                "netflow index evicted rows"
            );
        }
    }

    /// Sizes for diagnostics. An entry locked by an in-flight read is not
    /// counted in the per-file figures (`files` and `tier2_bytes` still are).
    pub fn status(&self) -> IndexStatus {
        let map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let mut st = IndexStatus {
            files: map.len() as u64,
            flows: 0,
            tier1_bytes: 0,
            tier2_bytes: self.resident_bytes.load(Ordering::Relaxed),
            budget_bytes: self.budget_bytes.load(Ordering::Relaxed),
            resident_files: 0,
            evictions_total: self.evictions.load(Ordering::Relaxed),
        };
        for entry in map.values() {
            if let Ok(e) = entry.try_lock() {
                st.flows += e.summary.flow_count as u64;
                st.tier1_bytes += e.summary.heap_bytes() as u64;
                st.resident_files += u64::from(e.resident.is_some());
            }
        }
        st
    }

    /// Forget entries whose ledger no longer exists, at most once per
    /// [`SWEEP_EVERY`] (cheap to call on every request).
    pub fn sweep_missing(&self) {
        {
            let mut last = self.last_sweep.lock().unwrap_or_else(|p| p.into_inner());
            if last.is_some_and(|t| t.elapsed() < SWEEP_EVERY) {
                return;
            }
            *last = Some(Instant::now());
        }
        self.sweep_missing_now();
    }

    /// [`sweep_missing`](Self::sweep_missing) without the throttle.
    pub fn sweep_missing_now(&self) {
        // Stat without the map lock: a slow filesystem must not stall readers.
        let paths: Vec<PathBuf> = {
            let map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
            map.keys().cloned().collect()
        };
        let missing: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| std::fs::symlink_metadata(p).is_err())
            .collect();
        if missing.is_empty() {
            return;
        }
        let dropped: Vec<Arc<Mutex<Entry>>> = {
            let mut map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
            missing.iter().filter_map(|p| map.remove(p)).collect()
        };
        // Outside the map lock: waiting on an in-flight read is fine here.
        for entry in dropped {
            self.retire(&entry);
        }
    }
}

/// What `refresh` found.
enum Refresh {
    /// The entry now reflects the file.
    Current,
    /// The file is gone.
    Gone,
    /// The file exists but could not be read; the entry is unchanged.
    Failed,
}

impl NetflowIndex {
    fn flows_in(
        &self,
        entry: &Arc<Mutex<Entry>>,
        path: &Path,
        range: &TimeRange,
    ) -> (Vec<FlowRecord>, u64) {
        let out = {
            let mut e = entry.lock().unwrap_or_else(|p| p.into_inner());
            if e.removed {
                // Not in the map any more: answer from the file, cache nothing.
                return DirectReader.flows_in_range(path, range);
            }
            match self.refresh(&mut e, path) {
                Refresh::Gone => None,
                Refresh::Failed => Some(DirectReader.flows_in_range(path, range)),
                Refresh::Current => {
                    if e.unreadable {
                        Some((Vec::new(), 0))
                    } else if !e.summary.overlaps(range) {
                        Some((Vec::new(), e.summary.dropped))
                    } else if !self.ensure_rows(&mut e, path) {
                        Some(DirectReader.flows_in_range(path, range))
                    } else {
                        let dropped = e.summary.dropped;
                        let rows = &e.resident.as_ref().expect("rows ensured").rows;
                        Some((rows.records_in_range(range).collect(), dropped))
                    }
                }
            }
        };
        let Some(out) = out else {
            self.remove(path);
            return (Vec::new(), 0);
        };
        self.enforce_budget();
        out
    }

    fn captures_in(&self, entry: &Arc<Mutex<Entry>>, path: &Path) -> Vec<CaptureEntry> {
        let out = {
            let mut e = entry.lock().unwrap_or_else(|p| p.into_inner());
            if e.removed {
                return DirectReader.capture_states(path);
            }
            match self.refresh(&mut e, path) {
                Refresh::Gone => None,
                Refresh::Failed => Some(DirectReader.capture_states(path)),
                Refresh::Current => Some(e.summary.capture.clone()),
            }
        };
        let Some(out) = out else {
            self.remove(path);
            return Vec::new();
        };
        self.enforce_budget();
        out
    }
}

impl LedgerReader for NetflowIndex {
    /// If the file cannot be read (or its rows re-loaded), this call answers
    /// with the direct reader's result and caches nothing new.
    fn flows_in_range(&self, path: &Path, range: &TimeRange) -> (Vec<FlowRecord>, u64) {
        self.flows_in(&self.entry(path), path, range)
    }

    fn capture_states(&self, path: &Path) -> Vec<CaptureEntry> {
        self.captures_in(&self.entry(path), path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::record::{CaptureState, LedgerLine, SocketCompletion};
    use rupu_netflow::{Fidelity, FlowCtx, FlowId, Origin, Outcome};
    use std::io::Write;

    fn flow(n: u64, secs: i64) -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(n, u128::from(n)),
            ts: chrono::DateTime::from_timestamp(secs, 0).unwrap(),
            ctx: FlowCtx::system(Origin::Provider("anthropic".into())),
            fidelity: Fidelity::Socket,
            method: "POST".into(),
            scheme: "https".into(),
            host: "api.anthropic.com".into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: Some("1.0.0.1".parse().unwrap()),
            resolved_ips: vec![],
            process: None,
            local_addr: None,
            direction: None,
            http_version: None,
            status: None,
            outcome: Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: None,
            body_complete: false,
            ttfb_ms: None,
            duration_ms: None,
        }
    }

    fn line(l: &LedgerLine) -> String {
        format!("{}\n", serde_json::to_string(l).unwrap())
    }

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn all() -> TimeRange {
        TimeRange::unbounded()
    }

    /// The index must answer exactly what the direct reader answers.
    fn assert_same(index: &NetflowIndex, path: &Path, range: &TimeRange) {
        assert_eq!(
            index.flows_in_range(path, range),
            DirectReader.flows_in_range(path, range)
        );
        assert_eq!(
            index.capture_states(path),
            DirectReader.capture_states(path)
        );
    }

    #[test]
    fn a_new_file_reads_like_the_direct_reader() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        append(
            &p,
            &line(&LedgerLine::Dropped {
                count: 2,
                ts: chrono::DateTime::from_timestamp(150, 0).unwrap(),
            }),
        );
        append(
            &p,
            &line(&LedgerLine::Capture {
                ts: chrono::DateTime::from_timestamp(160, 0).unwrap(),
                state: CaptureState::Unavailable {
                    reason: "no ntstat".into(),
                },
                tool_call_id: None,
                note: None,
            }),
        );
        let index = NetflowIndex::new(u64::MAX);
        assert_same(&index, &p, &all());
        let bounded = TimeRange {
            from: Some(chrono::DateTime::from_timestamp(150, 0).unwrap()),
            to: None,
        };
        assert_same(&index, &p, &bounded);
    }

    #[test]
    fn appended_lines_are_tailed_and_completions_patch_earlier_flows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        let index = NetflowIndex::new(u64::MAX);
        assert_same(&index, &p, &all());
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(2, 200)))));
        append(
            &p,
            &line(&LedgerLine::SocketComplete(SocketCompletion {
                id: FlowId::from_parts(1, 1u128),
                duration_ms: 5,
                bytes_in: Some(1),
                bytes_out: None,
                outcome: Some(Outcome::Timeout),
                error: None,
            })),
        );
        assert_same(&index, &p, &all());
        let s = index.summary(&p).unwrap();
        assert_eq!(s.flow_count, 2);
        assert!(
            s.points[0].1,
            "the patched outcome must flip the histogram point"
        );
    }

    #[test]
    fn a_partial_trailing_line_waits_for_its_newline() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        let full = line(&LedgerLine::Flow(Box::new(flow(1, 100))));
        let second = line(&LedgerLine::Flow(Box::new(flow(2, 200))));
        let (head, tail) = second.split_at(10);
        append(&p, &full);
        append(&p, head);
        let index = NetflowIndex::new(u64::MAX);
        assert_eq!(index.flows_in_range(&p, &all()).0.len(), 1);
        append(&p, tail);
        assert_same(&index, &p, &all());
    }

    #[test]
    fn a_rewritten_file_is_reread_not_tailed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        let index = NetflowIndex::new(u64::MAX);
        index.flows_in_range(&p, &all());
        // Same path, different content and longer: in-place rewrite.
        std::fs::write(
            &p,
            format!(
                "{}{}",
                line(&LedgerLine::Flow(Box::new(flow(7, 700)))),
                line(&LedgerLine::Flow(Box::new(flow(8, 800))))
            ),
        )
        .unwrap();
        assert_same(&index, &p, &all());
    }

    #[test]
    fn a_truncated_file_is_reread() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(2, 200)))));
        let index = NetflowIndex::new(u64::MAX);
        index.flows_in_range(&p, &all());
        std::fs::write(&p, line(&LedgerLine::Flow(Box::new(flow(3, 300))))).unwrap();
        assert_same(&index, &p, &all());
    }

    #[test]
    fn a_deleted_file_reads_empty_and_leaves_no_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        let index = NetflowIndex::new(u64::MAX);
        index.flows_in_range(&p, &all());
        std::fs::remove_file(&p).unwrap();
        assert_eq!(index.flows_in_range(&p, &all()), (vec![], 0));
        assert!(index.summary(&p).is_none());
        assert_eq!(index.status().files, 0);
    }

    #[test]
    fn non_utf8_content_reads_empty_like_the_direct_reader() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(&[0xff, 0xfe, b'\n']).unwrap();
        let index = NetflowIndex::new(u64::MAX);
        assert_same(&index, &p, &all());
        assert_eq!(index.flows_in_range(&p, &all()), (vec![], 0));
    }

    #[test]
    fn a_window_outside_the_file_skips_its_rows_but_keeps_its_dropped_count() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        append(
            &p,
            &line(&LedgerLine::Dropped {
                count: 4,
                ts: chrono::DateTime::from_timestamp(100, 0).unwrap(),
            }),
        );
        let index = NetflowIndex::new(u64::MAX);
        let later = TimeRange {
            from: Some(chrono::DateTime::from_timestamp(500, 0).unwrap()),
            to: None,
        };
        assert_eq!(index.flows_in_range(&p, &later), (vec![], 4));
        assert_same(&index, &p, &later);
    }

    #[test]
    fn a_failed_read_neither_wipes_nor_caches_and_the_next_call_recovers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(2, 200)))));
        let index = NetflowIndex::new(u64::MAX);
        assert_same(&index, &p, &all());
        let before = index.summary(&p).unwrap();
        assert_eq!(before.flow_count, 2);

        // Metadata succeeds, the read fails (portable, root-safe seam: a
        // directory under the ledger's name).
        std::fs::remove_file(&p).unwrap();
        std::fs::create_dir(&p).unwrap();
        assert_same(&index, &p, &all());
        // tier 1 is still the last consistent one, not wiped.
        assert_eq!(index.summary(&p).unwrap().flow_count, 2);

        // The file comes back with different content: the index recovers
        // and answers exactly like the direct reader.
        std::fs::remove_dir(&p).unwrap();
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(7, 700)))));
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(8, 800)))));
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(9, 900)))));
        assert_same(&index, &p, &all());
        assert_eq!(index.summary(&p).unwrap().flow_count, 3);
    }

    fn ledger_with(dir: &Path, name: &str, base: u64, secs: i64, n: u64) -> PathBuf {
        let p = dir.join(name);
        for i in 0..n {
            append(
                &p,
                &line(&LedgerLine::Flow(Box::new(flow(base + i, secs + i as i64)))),
            );
        }
        p
    }

    fn is_resident(index: &NetflowIndex, path: &Path) -> bool {
        let entry = Arc::clone(index.entries.lock().unwrap().get(path).unwrap());
        let e = entry.lock().unwrap();
        e.resident.is_some()
    }

    #[test]
    fn over_budget_evicts_the_oldest_files_rows_and_rereads_them_correctly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let old = ledger_with(tmp.path(), "run_old.jsonl", 0, 100, 200);
        let new = ledger_with(tmp.path(), "run_new.jsonl", 1000, 10_000, 200);
        let index = NetflowIndex::new(u64::MAX);
        index.flows_in_range(&old, &all());
        index.flows_in_range(&new, &all());
        let both = index.status().tier2_bytes;
        // Room for roughly one file's rows.
        index.set_budget_bytes(both * 2 / 3);
        index.flows_in_range(&new, &all());
        let st = index.status();
        assert!(st.tier2_bytes <= st.budget_bytes, "{st:?}");
        assert_eq!(st.resident_files, 1);
        assert!(st.evictions_total >= 1);
        // Oldest first: the file with the older newest flow lost its rows.
        assert!(is_resident(&index, &new));
        assert!(!is_resident(&index, &old));
        // Evicted rows are re-read on demand and the answer is unchanged...
        assert_same(&index, &old, &all());
        // ...and re-admitting the old file evicts it again, not the newer one.
        assert!(is_resident(&index, &new));
        assert!(!is_resident(&index, &old));
        let st = index.status();
        assert!(st.tier2_bytes <= st.budget_bytes, "{st:?}");
    }

    #[test]
    fn a_zero_budget_still_answers_every_read_exactly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = ledger_with(tmp.path(), "run_a.jsonl", 0, 100, 50);
        let index = NetflowIndex::new(0);
        assert_same(&index, &p, &all());
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(999, 999)))));
        assert_same(&index, &p, &all());
        assert_eq!(index.status().tier2_bytes, 0);
        assert_eq!(index.summary(&p).unwrap().flow_count, 51);
    }

    #[test]
    fn status_counts_files_flows_and_bytes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = ledger_with(tmp.path(), "run_a.jsonl", 0, 100, 3);
        let b = ledger_with(tmp.path(), "run_b.jsonl", 10, 100, 4);
        let index = NetflowIndex::new(1 << 30);
        index.summary(&a);
        index.summary(&b);
        let st = index.status();
        assert_eq!((st.files, st.flows, st.resident_files), (2, 7, 2));
        assert!(st.tier1_bytes > 0 && st.tier2_bytes > 0);
        assert_eq!(st.budget_bytes, 1 << 30);
    }

    #[test]
    fn sweep_missing_drops_entries_whose_file_is_gone() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = ledger_with(tmp.path(), "run_a.jsonl", 0, 100, 3);
        let b = ledger_with(tmp.path(), "run_b.jsonl", 10, 100, 4);
        let index = NetflowIndex::new(u64::MAX);
        index.summary(&a);
        index.summary(&b);
        std::fs::remove_file(&b).unwrap();
        index.sweep_missing_now();
        let st = index.status();
        assert_eq!((st.files, st.flows), (1, 3));
        assert!(st.tier2_bytes > 0);
        std::fs::remove_file(&a).unwrap();
        index.sweep_missing_now();
        let st = index.status();
        assert_eq!((st.files, st.flows, st.tier2_bytes), (0, 0, 0));
    }

    #[test]
    fn sweep_missing_is_throttled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = ledger_with(tmp.path(), "run_a.jsonl", 0, 100, 3);
        let b = ledger_with(tmp.path(), "run_b.jsonl", 10, 100, 4);
        let index = NetflowIndex::new(u64::MAX);
        index.summary(&a);
        index.summary(&b);
        index.sweep_missing(); // runs (first call), nothing missing
        assert_eq!(index.status().files, 2);
        std::fs::remove_file(&b).unwrap();
        index.sweep_missing(); // within 60s of the last sweep: skipped
        assert_eq!(index.status().files, 2);
        index.sweep_missing_now();
        assert_eq!(index.status().files, 1);
    }

    #[test]
    fn a_reader_holding_a_removed_entry_answers_directly_and_leaks_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = ledger_with(tmp.path(), "run_a.jsonl", 0, 100, 5);
        let index = NetflowIndex::new(u64::MAX);

        // Via `remove`: the reader fetched the entry first, then it is removed.
        let held = index.entry(&p);
        index.remove(&p);
        assert_eq!(
            index.flows_in(&held, &p, &all()),
            DirectReader.flows_in_range(&p, &all())
        );
        assert_eq!(
            index.captures_in(&held, &p),
            DirectReader.capture_states(&p)
        );
        assert!(index.summary_in(&held, &p).is_err());
        assert_eq!(index.status().tier2_bytes, 0);
        assert_eq!(index.resident_bytes.load(Ordering::Relaxed), 0);

        // Via the sweep.
        index.summary(&p);
        let held = Arc::clone(index.entries.lock().unwrap().get(&p).unwrap());
        let gone = tmp.path().join("run_gone.jsonl");
        let held_gone = index.entry(&gone);
        append(&gone, &line(&LedgerLine::Flow(Box::new(flow(1, 1)))));
        std::fs::remove_file(&gone).unwrap();
        index.sweep_missing_now();
        // The file reappears before the stale reader locks its entry.
        append(&gone, &line(&LedgerLine::Flow(Box::new(flow(1, 1)))));
        assert_eq!(
            index.flows_in(&held_gone, &gone, &all()),
            DirectReader.flows_in_range(&gone, &all())
        );
        assert_eq!(
            index.resident_bytes.load(Ordering::Relaxed),
            index.status().tier2_bytes
        );
        // The live entry is untouched.
        assert!(!held.lock().unwrap().removed);

        // A public summary() after removal rebuilds a fresh entry.
        index.remove(&p);
        assert_eq!(index.summary(&p).unwrap().flow_count, 5);
        assert_eq!(index.status().files, 1);
    }
}
