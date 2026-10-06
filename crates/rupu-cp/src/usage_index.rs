//! Incremental, process-cached usage fold — the ONE definition of "a run's
//! usage" (spec 2026-09-29 §4). Every CP endpoint and the CLI go through here
//! (via `crate::usage::{run_usage, transcripts_usage}`).
//!
//! A run's usage is the sum of its usage-ledger rows (`usage.jsonl`) plus, for
//! every *known* transcript (step results ∪ live events ∪ the dispatched
//! sub-run tree) that has no ledger row, a fold of that transcript file. The
//! file is resolved locally, then in the worker mirror cache, then at the
//! agent mirror path; a transcript that should exist but resolves nowhere
//! contributes nothing and marks the result `partial`.
//!
//! The file has two halves:
//! - **The fold** — pure state machines ([`LedgerFold`], [`TranscriptFold`])
//!   fed one JSONL line at a time, and [`build_usage`], which turns them into
//!   a [`RunUsage`]. No IO.
//! - **The index** — [`UsageIndex`], which owns the per-run and per-file
//!   state behind locks, drives the [`JsonlCursor`]s so each call reads only
//!   appended bytes, returns the previous `Arc<RunUsage>` when nothing
//!   changed, and seals terminal runs.
//!
//! Usage accounting never fails a caller: unreadable or garbled input
//! degrades to "partial" or zero, never an error or a panic.

use crate::host::transcript_paths::{agent_mirror_path, cache_path, global_dir_of};
use crate::usage::TurnPoint;
use rupu_orchestrator::runs::{
    known_transcript_from_event_line, known_transcripts_from_step_result_line,
    placed_step_from_event_line, KnownTranscript, RunRecord, RunStore,
};
use rupu_orchestrator::usage_ledger::{LedgerKind, LedgerRow};
use rupu_transcript::{transcript_key, Event, JsonlCursor, UsageRow};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

/// `(provider, model, agent)`.
type ModelKey = (String, String, String);

/// `Usage.purpose` of a context-compaction call: counted in tokens, not turns.
const COMPACTION: &str = "compaction";

/// Most parent hops a dispatched agent's attribution walk takes (a cycle or a
/// hand-edited ledger can never make it spin).
const MAX_PARENT_HOPS: usize = 64;

// ════════════════════════════════════════════════════════════════════════
// Output
// ════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    /// Cache reads — a subset of `input`.
    pub cached: u64,
    /// Cache writes — a subset of `input`, like `cached`.
    pub cache_write: u64,
}

impl Tokens {
    fn add(&mut self, o: Tokens) {
        self.input = self.input.saturating_add(o.input);
        self.output = self.output.saturating_add(o.output);
        self.cached = self.cached.saturating_add(o.cached);
        self.cache_write = self.cache_write.saturating_add(o.cache_write);
    }
}

/// One run's (or one transcript set's) usage. Pricing is applied by
/// consumers (`crate::usage::summarize_run_usage`), never stored here.
#[derive(Debug, Clone, Default)]
pub struct RunUsage {
    /// Keyed by `(provider, model, agent)`; `runs` = contributing transcripts.
    pub rows: Vec<UsageRow>,
    /// The same rows per step label; `""` = unattributed.
    pub by_step: BTreeMap<String, Vec<UsageRow>>,
    /// Ledger tokens per `(step, unit_index)` (fan-out units, dispatch
    /// children folded into their ancestor's unit).
    pub by_unit: BTreeMap<(String, usize), Tokens>,
    /// LLM turns; compaction calls are not turns.
    pub turns: u64,
    /// Longest `RunComplete.duration_ms` among fallback transcripts.
    pub duration_ms: Option<u64>,
    /// Per-call series, append-only in observation order: each refresh
    /// appends the ledger's new rows, then each fallback transcript's new
    /// usage events in known-set order. `turn` is the 1-based index. See
    /// [`Series`] for when it is instead rebuilt canonically (ledger, then
    /// fallback) — the only times `epoch` moves.
    pub points: Vec<TurnPoint>,
    /// A transcript that should exist could not be read anywhere.
    pub partial: bool,
    /// Changes whenever this run's series is rebuilt non-append-only (Plan 2
    /// `epoch`); pure appends never move it.
    pub epoch: u64,
}

// ════════════════════════════════════════════════════════════════════════
// The fold — pure state machines, no IO
// ════════════════════════════════════════════════════════════════════════

/// One agent run's ledger rows (spec §4.2 rule 1).
#[derive(Debug, Default, Clone)]
struct AgentAgg {
    step_id: Option<String>,
    unit_index: Option<usize>,
    parent: Option<String>,
    by_model: BTreeMap<ModelKey, Tokens>,
}

/// Fold state for one run's `usage.jsonl`.
#[derive(Debug, Default)]
struct LedgerFold {
    cursor: JsonlCursor,
    /// Row ids already counted (mirror replays / tailer races count once).
    seen: HashSet<String>,
    agents: HashMap<String, AgentAgg>,
    /// `transcript_key` of every row's transcript.
    keys: HashSet<String>,
    /// Every row's transcript, first sighting per key, in arrival order.
    transcripts: Vec<(String, PathBuf)>,
    /// `(agent_run_id, tokens)` per row, in arrival order.
    points: Vec<(String, Tokens)>,
    turns: u64,
}

impl LedgerFold {
    /// Forget everything folded so far but keep the cursor (which has already
    /// restarted at the top of the shrunk file).
    fn clear_keep_cursor(&mut self) {
        let cursor = std::mem::take(&mut self.cursor);
        *self = LedgerFold {
            cursor,
            ..LedgerFold::default()
        };
    }

    fn apply_line(&mut self, line: &str) {
        let Ok(row) = serde_json::from_str::<LedgerRow>(line) else {
            return; // garbled / foreign line: skipped
        };
        if !self.seen.insert(row.id.clone()) {
            return;
        }
        let t = Tokens {
            input: row.input_tokens,
            output: row.output_tokens,
            cached: row.cached_tokens,
            cache_write: row.cache_write_tokens,
        };
        if let Some(key) = transcript_key(&row.transcript) {
            if self.keys.insert(key.clone()) {
                self.transcripts.push((key, row.transcript.clone()));
            }
        }
        if row.kind == LedgerKind::Turn {
            self.turns += 1;
        }
        let agg = self.agents.entry(row.agent_run_id.clone()).or_default();
        if agg.step_id.is_none() {
            agg.step_id = row.step_id;
        }
        if agg.unit_index.is_none() {
            agg.unit_index = row.unit_index;
        }
        if agg.parent.is_none() {
            agg.parent = row.parent_agent_run_id;
        }
        agg.by_model
            .entry((row.provider, row.model, row.agent))
            .or_default()
            .add(t);
        self.points.push((row.agent_run_id, t));
    }
}

/// Fold state for one transcript file, shared process-wide per resolved path
/// (the same semantics as the old `aggregate_rows_and_metrics`: anchored on
/// `RunStart`, a `Usage` event's own provider/model wins, `RunComplete` sets
/// the duration).
#[derive(Debug, Default)]
struct TranscriptFold {
    cursor: JsonlCursor,
    /// `(provider, model, agent)` of the latest `RunStart`.
    start: Option<ModelKey>,
    /// Tokens after `RunStart`, keyed by the event's own provider/model and
    /// the run-start agent.
    by_model: BTreeMap<ModelKey, Tokens>,
    /// Every non-compaction `Usage`, even one before `RunStart`.
    turns: u64,
    /// The last `RunComplete`.
    duration_ms: Option<u64>,
    /// One per counted `Usage` (after `RunStart`), in file order.
    points: Vec<Tokens>,
    /// Bumped on every change, so a run whose view was built from an older
    /// generation knows to rebuild even when another caller drained this
    /// shared fold.
    generation: u64,
    /// How many times the file shrank or was replaced.
    resets: u64,
    /// The last drain hit an IO error (the file exists but can't be read).
    unreadable: bool,
}

impl TranscriptFold {
    /// Zero the folded accumulators, leaving the cursor, generation and reset
    /// counters untouched. Used by [`drain`](Self::drain) on a file reset,
    /// where the cursor is held outside `self` and must not be disturbed.
    fn clear_fold_state(&mut self) {
        self.start = None;
        self.by_model = BTreeMap::new();
        self.turns = 0;
        self.duration_ms = None;
        self.points = Vec::new();
    }

    fn apply_line(&mut self, line: &str) {
        // Cheap pre-filter: writers emit `{"type":"<tag>",…}`, and the bulk
        // of a transcript (tool results, deltas) is irrelevant here, so skip
        // those without a full JSON parse. Anything not in that shape falls
        // through to the real parser.
        if let Some(rest) = line.strip_prefix("{\"type\":\"") {
            let tag = rest.split('"').next().unwrap_or_default();
            if !matches!(tag, "run_start" | "usage" | "run_complete") {
                return;
            }
        }
        let Ok(ev) = serde_json::from_str::<Event>(line) else {
            return;
        };
        match ev {
            Event::RunStart {
                provider,
                model,
                agent,
                ..
            } => self.start = Some((provider, model, agent)),
            Event::Usage {
                provider,
                model,
                input_tokens,
                output_tokens,
                cached_tokens,
                cache_write_tokens,
                purpose,
                ..
            } => {
                if purpose.as_deref() != Some(COMPACTION) {
                    self.turns += 1;
                }
                let Some((_, _, agent)) = &self.start else {
                    return; // no RunStart yet: an orphan, not counted in tokens
                };
                let t = Tokens {
                    input: u64::from(input_tokens),
                    output: u64::from(output_tokens),
                    cached: u64::from(cached_tokens),
                    cache_write: u64::from(cache_write_tokens),
                };
                self.by_model
                    .entry((provider, model, agent.clone()))
                    .or_default()
                    .add(t);
                self.points.push(t);
            }
            Event::RunComplete { duration_ms, .. } => self.duration_ms = Some(duration_ms),
            _ => {}
        }
    }

    /// Fold every line appended since the last drain. A missing file is "no
    /// data yet"; a shrunk/replaced file restarts the fold.
    ///
    /// Each line is folded as it is read — never collected into a `Vec<String>`
    /// first. A transcript's bulk is tool-result and delta lines (which
    /// [`apply_line`](Self::apply_line) discards without a full parse), so a
    /// multi-GB transcript folded on the aggregate path would otherwise
    /// materialise its entire new content as owned `String`s at once. The
    /// cursor is moved out of `self` for the call so `on_reset` / `on_line` can
    /// borrow the rest of `self`; `on_reset` fires before the first new line
    /// (cursor contract), so a reset clears stale state in the right order.
    fn drain(&mut self, path: &Path) {
        let mut cursor = std::mem::take(&mut self.cursor);
        let did_reset = std::cell::Cell::new(false);
        let mut cleared = false;
        let result = cursor.drain_with(
            path,
            || did_reset.set(true),
            |l| {
                if did_reset.get() && !cleared {
                    self.clear_fold_state();
                    cleared = true;
                }
                self.apply_line(l);
            },
        );
        self.cursor = cursor;
        match result {
            Ok(stats) => {
                self.unreadable = false;
                // A reset with no following lines still clears the fold.
                if did_reset.get() && !cleared {
                    self.clear_fold_state();
                }
                if stats.reset {
                    self.resets += 1;
                    self.generation += 1;
                }
                if stats.bytes > 0 {
                    self.generation += 1;
                }
            }
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "usage index: transcript unreadable");
                self.unreadable = true;
            }
        }
    }
}

/// Where each `(provider, model, agent)` row and each point accumulates while
/// a [`RunUsage`] is assembled.
#[derive(Default)]
struct Acc {
    rows: BTreeMap<ModelKey, UsageRow>,
    by_step: BTreeMap<String, BTreeMap<ModelKey, UsageRow>>,
    by_unit: BTreeMap<(String, usize), Tokens>,
    points: Vec<TurnPoint>,
    turns: u64,
    duration_ms: Option<u64>,
}

fn add_row(map: &mut BTreeMap<ModelKey, UsageRow>, key: &ModelKey, t: Tokens) {
    let row = map.entry(key.clone()).or_insert_with(|| UsageRow {
        provider: key.0.clone(),
        model: key.1.clone(),
        agent: key.2.clone(),
        ..UsageRow::default()
    });
    row.input_tokens = row.input_tokens.saturating_add(t.input);
    row.output_tokens = row.output_tokens.saturating_add(t.output);
    row.cached_tokens = row.cached_tokens.saturating_add(t.cached);
    row.cache_write_tokens = row.cache_write_tokens.saturating_add(t.cache_write);
    row.runs += 1;
}

impl Acc {
    /// One contributing transcript's tokens for one model key (`runs += 1`).
    fn row(&mut self, label: &str, key: &ModelKey, t: Tokens) {
        add_row(&mut self.rows, key, t);
        add_row(self.by_step.entry(label.to_string()).or_default(), key, t);
    }

    fn point(&mut self, label: &str, t: Tokens) {
        self.points.push(TurnPoint {
            turn: self.points.len() as u64 + 1,
            label: label.to_string(),
            tokens_in: t.input,
            tokens_out: t.output,
            tokens_cached: t.cached,
            tokens_cache_write: t.cache_write,
        });
    }

    /// One fallback transcript's totals (one contributing transcript per model
    /// key; a file with `RunStart` and no usage still counts as a zero row).
    /// Its points reach the output through the [`Series`], not from here.
    fn transcript(&mut self, label: &str, f: &TranscriptFold) {
        for (key, t) in &f.by_model {
            self.row(label, key, *t);
        }
        if f.by_model.is_empty() {
            if let Some(start) = &f.start {
                self.row(label, start, Tokens::default());
            }
        }
        self.turns += f.turns;
        if let Some(d) = f.duration_ms {
            self.duration_ms = Some(self.duration_ms.map_or(d, |cur| cur.max(d)));
        }
    }

    fn finish(self, partial: bool, epoch: u64) -> RunUsage {
        RunUsage {
            rows: self.rows.into_values().collect(),
            by_step: self
                .by_step
                .into_iter()
                .map(|(k, v)| (k, v.into_values().collect()))
                .collect(),
            by_unit: self.by_unit,
            turns: self.turns,
            duration_ms: self.duration_ms,
            points: self.points,
            partial,
            epoch,
        }
    }
}

/// One point of a run's owned per-turn series.
#[derive(Debug, Clone)]
enum SeriesPoint {
    /// `LedgerFold::points[i]` — the raw `(agent_run_id, tokens)`, labelled
    /// through the parent chain at build time (a child's row can arrive before
    /// its ancestor's).
    Ledger(usize),
    /// A fallback transcript's point, labelled when observed.
    Fallback(Arc<str>, Tokens),
}

/// How much of one fallback transcript's fold the series has consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Watermark {
    path: PathBuf,
    resets: u64,
    taken: usize,
}

/// A fallback transcript feeding a build, in known-set order.
struct FallbackSrc {
    key: String,
    label: Arc<str>,
    path: PathBuf,
    fold: Arc<Mutex<TranscriptFold>>,
}

/// A run's per-turn series, append-only in OBSERVATION order (spec §4.3 as
/// amended by the Task 8 fix ruling): each refresh appends the ledger's new
/// points, then each fallback transcript's new points in known-set order, so
/// Plan 2's `since=N` client only ever sees the series grow. It is rebuilt
/// canonically — every ledger point in ledger order, then every fallback
/// point in known-set order — only when that is impossible:
/// - the ledger reset after some of its points were emitted;
/// - an emitted fallback transcript reset (shrank / was replaced) or now
///   resolves to a different file;
/// - an emitted fallback key left the fallback set (its ledger rows arrived,
///   or it no longer resolves), so its points must go.
///
/// A fallback key none of whose points were emitted never forces a rebuild.
#[derive(Debug, Default)]
struct Series {
    points: Vec<SeriesPoint>,
    /// `LedgerFold::points` already appended.
    ledger_taken: usize,
    /// Per fallback key.
    marks: HashMap<String, Watermark>,
}

impl Series {
    /// Append every point not yet observed. Returns `true` when the series had
    /// to be rebuilt canonically instead (the caller moves the epoch).
    fn advance(
        &mut self,
        ledger: &LedgerFold,
        ledger_reset: bool,
        fallback: &[FallbackSrc],
    ) -> bool {
        let live: HashSet<&str> = fallback.iter().map(|f| f.key.as_str()).collect();
        let mut rebuild = (ledger_reset && self.ledger_taken > 0)
            || self.ledger_taken > ledger.points.len()
            || self
                .marks
                .iter()
                .any(|(k, m)| m.taken > 0 && !live.contains(k.as_str()));
        let mut rebuilt = false;
        loop {
            if rebuild {
                *self = Series::default();
                rebuilt = true;
                rebuild = false;
            }
            self.points
                .extend((self.ledger_taken..ledger.points.len()).map(SeriesPoint::Ledger));
            self.ledger_taken = ledger.points.len();
            for src in fallback {
                let f = lock_fold(&src.fold);
                let mark = self
                    .marks
                    .entry(src.key.clone())
                    .or_insert_with(|| Watermark {
                        path: src.path.clone(),
                        resets: f.resets,
                        taken: 0,
                    });
                if mark.path != src.path || mark.resets != f.resets || mark.taken > f.points.len() {
                    if mark.taken > 0 {
                        rebuild = true; // its emitted points are no longer true
                        break;
                    }
                    // Nothing of it emitted yet: just re-anchor.
                    *mark = Watermark {
                        path: src.path.clone(),
                        resets: f.resets,
                        taken: 0,
                    };
                }
                let fresh = f.points.get(mark.taken..).unwrap_or_default();
                self.points.extend(
                    fresh
                        .iter()
                        .map(|t| SeriesPoint::Fallback(Arc::clone(&src.label), *t)),
                );
                mark.taken = f.points.len();
            }
            if !rebuild {
                break;
            }
        }
        self.marks.retain(|k, _| live.contains(k.as_str()));
        rebuilt
    }
}

/// The step (and unit) a ledger agent run is attributed to: its own tag, or
/// the nearest ancestor's, walking `parent` links at most
/// [`MAX_PARENT_HOPS`]. An ancestor with no ledger rows of its own falls back
/// to its known-set step label. Unresolved → `""`.
fn attribute(
    agent_run_id: &str,
    agents: &HashMap<String, AgentAgg>,
    known_step: &dyn Fn(&str) -> Option<String>,
) -> (String, Option<usize>) {
    let mut cur = agent_run_id;
    for _ in 0..=MAX_PARENT_HOPS {
        let Some(agg) = agents.get(cur) else {
            return (known_step(cur).unwrap_or_default(), None);
        };
        if let Some(step) = &agg.step_id {
            return (step.clone(), agg.unit_index);
        }
        match &agg.parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    (String::new(), None)
}

/// Spec §4.2 rule 4: ledger agents (attributed through their parent chain)
/// first, then each fallback transcript in known-set order; `points` are the
/// run's [`Series`], labelled here.
fn build_usage(
    ledger: &LedgerFold,
    known_step: &dyn Fn(&str) -> Option<String>,
    fallback: &[FallbackSrc],
    series: &[SeriesPoint],
    partial: bool,
    epoch: u64,
) -> RunUsage {
    let mut acc = Acc::default();
    let mut ids: Vec<&String> = ledger.agents.keys().collect();
    ids.sort();
    let mut attributed: HashMap<&str, String> = HashMap::with_capacity(ids.len());
    for id in ids {
        let agg = &ledger.agents[id];
        let (label, unit) = attribute(id, &ledger.agents, known_step);
        for (key, t) in &agg.by_model {
            acc.row(&label, key, *t);
            if let Some(u) = unit {
                acc.by_unit.entry((label.clone(), u)).or_default().add(*t);
            }
        }
        attributed.insert(id.as_str(), label);
    }
    acc.turns += ledger.turns;
    for src in fallback {
        acc.transcript(&src.label, &lock_fold(&src.fold));
    }
    for p in series {
        match p {
            SeriesPoint::Ledger(i) => {
                let Some((id, t)) = ledger.points.get(*i) else {
                    continue; // unreachable: a ledger reset rebuilds the series
                };
                let label = attributed.get(id.as_str()).map_or("", String::as_str);
                acc.point(label, *t);
            }
            SeriesPoint::Fallback(label, t) => acc.point(label, *t),
        }
    }
    acc.finish(partial, epoch)
}

// ════════════════════════════════════════════════════════════════════════
// The index — caching, locking, IO
// ════════════════════════════════════════════════════════════════════════

/// A known transcript plus whether a `step_results` row (a finished step or
/// item) named it — i.e. whether it SHOULD exist.
#[derive(Debug)]
struct Known {
    kt: KnownTranscript,
    from_step_results: bool,
}

/// `(len, mtime, inode)`; a missing file is [`ABSENT`]. The inode (unix; `0`
/// elsewhere) is what catches an authoritative replace by rename — a mirror's
/// terminal ledger pull — that happens to leave the length unchanged and lands
/// within the filesystem's mtime granularity (or carries the old mtime):
/// without it a sealed terminal run would never see the new content.
type Stat = (u64, Option<SystemTime>, u64);

fn stat(path: &Path) -> Stat {
    match std::fs::metadata(path) {
        Ok(m) => (m.len(), m.modified().ok(), inode(&m)),
        Err(_) => ABSENT,
    }
}

#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    m.ino()
}

#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> u64 {
    0
}

/// What a terminal run's cached result was computed against.
///
/// Known edge: on a LOCAL run, a fallback transcript of a LOCAL unit or step
/// is not statted (only the run files are), so one more `Usage` line after
/// `run.json` turned terminal stays frozen at the sealed total until a run
/// file changes. That covers a legacy (no-ledger) run's own transcripts,
/// which are written before the run finishes; turns the coordinator ledgers
/// change `usage.jsonl`. A PLACED unit or step (its `unit_started` /
/// `step_started` / step result names a host) writes no coordinator ledger
/// rows, and its transcript — recorded, since #646, AS the coordinator's
/// agent-mirror path — is filled by a host pull that may still grow it after
/// the run turned terminal. So placement decides: every placed fallback is
/// statted in `mirrors`, as is any fallback that resolved away from its
/// recorded path and every folded file of a mirrored run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seal {
    /// `run.json`, `usage.jsonl`, `events.jsonl`, `step_results.jsonl`.
    files: [Stat; 4],
    /// Extra files the result depends on: every folded fallback file of a
    /// mirrored run; on any run, each folded fallback of a placed unit/step
    /// and each that resolved to a worker-cache / agent-mirror file instead
    /// of its recorded path (a host pull may still fill them after the run
    /// turned terminal); plus each resolution candidate of a transcript that
    /// resolved nowhere, at its absent stat — so a late pull that creates any
    /// of them breaks the seal.
    mirrors: Vec<(PathBuf, Stat)>,
}

/// The stat of a path [`resolve`] found absent (`is_file()` false), and of any
/// missing file.
const ABSENT: Stat = (0, None, 0);

/// The fallback transcript versions a build used, per key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FallbackView {
    key: String,
    path: PathBuf,
    generation: u64,
    resets: u64,
}

#[derive(Debug, Default)]
struct RunState {
    ledger: LedgerFold,
    events: JsonlCursor,
    step_results: JsonlCursor,
    /// Insertion-ordered known set.
    known: Vec<Known>,
    known_idx: HashMap<String, usize>,
    /// Steps a `step_started` placed on a fleet host (additive, like the
    /// known set): every transcript of theirs ran there.
    placed_steps: HashSet<String>,
    /// Fallback transcripts the last build folded.
    fallback: Vec<FallbackView>,
    /// The per-turn series, append-only in observation order.
    series: Series,
    /// `finished_at - started_at` of the last loaded run record (still valid
    /// while sealed: `run.json` is part of the seal).
    wall_clock_ms: Option<u64>,
    seal: Option<Seal>,
    last: Option<Arc<RunUsage>>,
    epoch: u64,
    /// [`UsageIndex::claimed_paths`] of a sealed run; cleared whenever the
    /// seal breaks.
    claimed: Option<Arc<Vec<PathBuf>>>,
}

impl RunState {
    fn new(epoch: u64) -> Self {
        RunState {
            epoch,
            ..RunState::default()
        }
    }

    /// Add to the known set; `true` when it changed. A `step_results` sighting
    /// of a key first seen in events replaces it (durable label wins, as in
    /// `RunStore::known_transcripts`) and marks it should-exist. Placement
    /// (`host`) is kept from whichever sighting stated it; it only affects
    /// what a seal stats, never the usage, so it is not a change.
    fn add_known(&mut self, kt: KnownTranscript, from_step_results: bool) -> bool {
        match self.known_idx.get(&kt.key) {
            None => {
                self.known_idx.insert(kt.key.clone(), self.known.len());
                self.known.push(Known {
                    kt,
                    from_step_results,
                });
                true
            }
            Some(&i) => {
                let k = &mut self.known[i];
                let host = k.kt.host.take().or_else(|| kt.host.clone());
                let changed = if from_step_results && !k.from_step_results {
                    k.kt = kt;
                    k.from_step_results = true;
                    true
                } else {
                    false
                };
                k.kt.host = host;
                changed
            }
        }
    }

    /// The transcript ran on a fleet host: its own sighting said so, or its
    /// step was placed by a `step_started`.
    fn placed(&self, k: &Known) -> bool {
        k.kt.host.is_some()
            || k.kt
                .step_id
                .as_ref()
                .is_some_and(|s| self.placed_steps.contains(s))
    }

    fn known_step(&self, key: &str) -> Option<String> {
        let i = *self.known_idx.get(key)?;
        self.known.get(i)?.kt.step_id.clone()
    }
}

/// Drain `path`'s new lines into a Vec (so the reset can be handled before
/// they are applied). `Err` = the file exists but can't be read.
fn drain_lines(cursor: &mut JsonlCursor, path: &Path) -> std::io::Result<(bool, u64, Vec<String>)> {
    let mut lines = Vec::new();
    let stats = cursor.drain_with(path, || {}, |l| lines.push(l.to_owned()))?;
    Ok((stats.reset, stats.bytes, lines))
}

/// A transcript key we are willing to turn into a mirror file name.
fn safe_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Spec §4.2 rule 3's candidates, in resolution order: the recorded path,
/// then (mirrored runs) the worker mirror cache, then the agent mirror path.
fn candidates(path: &Path, key: &str, worker: Option<&str>, global: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    if let Some(c) = worker.and_then(|w| cache_path(global, w, path)) {
        out.push(c);
    }
    if safe_key(key) {
        out.push(agent_mirror_path(global, key));
    }
    out
}

/// Spec §4.2 rule 3's resolution: the first [`candidates`] entry that is a
/// file. `None` = nowhere.
fn resolve(path: &Path, key: &str, worker: Option<&str>, global: &Path) -> Option<PathBuf> {
    candidates(path, key, worker, global)
        .into_iter()
        .find(|c| c.is_file())
}

/// A finished run's wall-clock duration.
fn wall_clock_ms(rec: &RunRecord) -> Option<u64> {
    let finished = rec.finished_at?;
    Some(u64::try_from((finished - rec.started_at).num_milliseconds()).unwrap_or(0))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A fold whose previous holder panicked mid-update is restarted rather than
/// trusted.
fn lock_fold(m: &Mutex<TranscriptFold>) -> MutexGuard<'_, TranscriptFold> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => {
            let mut g = p.into_inner();
            let (generation, resets) = (g.generation + 1, g.resets + 1);
            *g = TranscriptFold {
                generation,
                resets,
                ..TranscriptFold::default()
            };
            m.clear_poison();
            g
        }
    }
}

/// Per-run state, keyed by `(store root, run_id)`.
type RunMap = HashMap<(PathBuf, String), Arc<Mutex<RunState>>>;
/// Per-file transcript folds, keyed by resolved path (shared across runs).
type FileMap = HashMap<PathBuf, Arc<Mutex<TranscriptFold>>>;

pub struct UsageIndex {
    runs: Mutex<RunMap>,
    files: Mutex<FileMap>,
    /// UNIX nanos at construction — every RunState's epoch starts here so an
    /// epoch never repeats across CP restarts (Plan 2's `since`/`epoch`).
    base_epoch: u64,
}

impl Default for UsageIndex {
    fn default() -> Self {
        let base_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Self {
            runs: Mutex::default(),
            files: Mutex::default(),
            base_epoch,
        }
    }
}

impl UsageIndex {
    /// The process-wide index every CP endpoint and the CLI share.
    pub fn global() -> &'static UsageIndex {
        static G: OnceLock<UsageIndex> = OnceLock::new();
        G.get_or_init(UsageIndex::default)
    }

    fn run_state(&self, store: &RunStore, run_id: &str) -> Arc<Mutex<RunState>> {
        let mut map = lock(&self.runs);
        Arc::clone(
            map.entry((store.root.clone(), run_id.to_string()))
                .or_insert_with(|| Arc::new(Mutex::new(RunState::new(self.base_epoch)))),
        )
    }

    fn file_fold(&self, path: &Path) -> Arc<Mutex<TranscriptFold>> {
        let mut map = lock(&self.files);
        Arc::clone(map.entry(path.to_path_buf()).or_default())
    }

    /// Lock a run's state; one whose previous holder panicked mid-update
    /// restarts from scratch (with a fresh epoch) rather than being trusted.
    fn lock_run<'a>(&self, m: &'a Mutex<RunState>) -> MutexGuard<'a, RunState> {
        match m.lock() {
            Ok(g) => g,
            Err(p) => {
                let mut g = p.into_inner();
                let epoch = g.epoch.saturating_add(1);
                *g = RunState::new(epoch);
                m.clear_poison();
                g
            }
        }
    }

    /// A run's usage (spec §4.2 rules 1–5). Reads only bytes appended since
    /// the previous call for the same `(store root, run_id)`.
    pub fn run_usage(&self, store: &RunStore, run_id: &str) -> Arc<RunUsage> {
        let state = self.run_state(store, run_id);
        let mut st = self.lock_run(&state);
        self.refresh(&mut st, store, run_id)
    }

    /// [`Self::run_usage`] plus the run record's wall-clock duration once it
    /// has finished, from the same record load (none at all while sealed).
    pub(crate) fn run_usage_and_wall_clock(
        &self,
        store: &RunStore,
        run_id: &str,
    ) -> (Arc<RunUsage>, Option<u64>) {
        let state = self.run_state(store, run_id);
        let mut st = self.lock_run(&state);
        let usage = self.refresh(&mut st, store, run_id);
        (usage, st.wall_clock_ms)
    }

    fn refresh(&self, st: &mut RunState, store: &RunStore, run_id: &str) -> Arc<RunUsage> {
        let ledger_path = store.usage_ledger_path(run_id);
        let events_path = store.events_path(run_id);
        let step_results_path = store.step_results_path(run_id);
        // Stat BEFORE reading: a write racing this call changes the stat the
        // next call compares against, so it is re-validated, never lost.
        let files = [
            stat(&store.run_json_path(run_id)),
            stat(&ledger_path),
            stat(&events_path),
            stat(&step_results_path),
        ];
        if let (Some(seal), Some(last)) = (&st.seal, &st.last) {
            if seal.files == files && seal.mirrors.iter().all(|(p, s)| stat(p) == *s) {
                return Arc::clone(last);
            }
        }
        st.seal = None;
        st.claimed = None;

        let rec = store.load(run_id).ok();
        st.wall_clock_ms = rec.as_ref().and_then(wall_clock_ms);
        let worker = rec.as_ref().and_then(|r| r.worker_id.clone());
        let terminal = rec.as_ref().is_some_and(|r| r.status.is_terminal());
        let mut changed = st.last.is_none();
        let mut ledger_reset = false;
        let mut partial = false;
        // A file that exists but could not be read: partial AND never sealed
        // (becoming readable again need not change any stat we compare).
        let mut unreadable = false;

        // Rule 1: the ledger.
        match drain_lines(&mut st.ledger.cursor, &ledger_path) {
            Ok((reset, bytes, lines)) => {
                if reset {
                    st.ledger.clear_keep_cursor();
                    ledger_reset = true;
                    changed = true;
                }
                changed |= bytes > 0;
                for l in &lines {
                    st.ledger.apply_line(l);
                }
            }
            Err(e) => {
                tracing::debug!(run_id, error = %e, "usage index: ledger unreadable");
                partial = true;
                unreadable = true;
            }
        }

        // Rule 2: the known set, step_results first (its label is durable).
        // The set is additive: a reset re-delivers lines that dedup by key.
        match drain_lines(&mut st.step_results, &step_results_path) {
            Ok((_, _, lines)) => {
                for l in &lines {
                    for kt in known_transcripts_from_step_result_line(l) {
                        changed |= st.add_known(kt, true);
                    }
                }
            }
            Err(e) => {
                tracing::debug!(run_id, error = %e, "usage index: step_results unreadable");
                partial = true;
                unreadable = true;
            }
        }
        match drain_lines(&mut st.events, &events_path) {
            Ok((_, _, lines)) => {
                for l in &lines {
                    if let Some(kt) = known_transcript_from_event_line(l) {
                        changed |= st.add_known(kt, false);
                    } else if let Some((step, _host)) = placed_step_from_event_line(l) {
                        st.placed_steps.insert(step);
                    }
                }
            }
            Err(e) => {
                tracing::debug!(run_id, error = %e, "usage index: events unreadable");
                partial = true;
                unreadable = true;
            }
        }
        // The sub-run walk: only for a run with no ledger (legacy / mirrored:
        // dispatch children have no rows) or a terminal run not yet sealed
        // (we only get here unsealed). A live ledger run learns its children
        // from ledger rows and `DispatchStarted` events instead.
        if !ledger_path.exists() || terminal {
            let mut ledger_roots: Vec<&String> = st.ledger.agents.keys().collect();
            ledger_roots.sort();
            let roots: Vec<String> = std::iter::once(run_id.to_string())
                .chain(st.known.iter().map(|k| k.kt.key.clone()))
                .chain(ledger_roots.into_iter().cloned())
                .collect();
            let mut walked = store.dispatched_transcripts(roots.iter().map(String::as_str));
            walked.sort_by(|a, b| a.key.cmp(&b.key));
            for kt in walked {
                changed |= st.add_known(kt, false);
            }
        }

        // Rule 3: fallback for every known key without ledger rows.
        let global = global_dir_of(store);
        let mut fallback: Vec<FallbackSrc> = Vec::new();
        let mut views: Vec<FallbackView> = Vec::new();
        let mut mirrors: Vec<(PathBuf, Stat)> = Vec::new();
        for k in &st.known {
            if st.ledger.keys.contains(&k.kt.key) {
                continue;
            }
            let Some(path) = resolve(&k.kt.path, &k.kt.key, worker.as_deref(), &global) else {
                // Missing data only when it should exist: a finished step's
                // transcript, or anything of a mirrored run. A local unit that
                // just started simply hasn't created its file yet.
                if k.from_step_results || worker.is_some() {
                    partial = true;
                }
                // Seal against every place it could still appear.
                for c in candidates(&k.kt.path, &k.kt.key, worker.as_deref(), &global) {
                    mirrors.push((c, ABSENT));
                }
                continue;
            };
            // Seal over every folded file a host pull may still grow: all of a
            // mirrored run's; on a local run, every transcript of a unit or
            // step that ran on a host (since #646 its recorded path IS the
            // agent mirror, so placement — not the path — says so), and any
            // that resolved away from its recorded path. A local run's own
            // on-disk transcripts are not (the legacy edge on `Seal`).
            if worker.is_some() || st.placed(k) || path != k.kt.path {
                mirrors.push((path.clone(), stat(&path)));
            }
            let fold = self.file_fold(&path);
            let view = {
                let mut f = lock_fold(&fold);
                f.drain(&path);
                partial |= f.unreadable;
                unreadable |= f.unreadable;
                FallbackView {
                    key: k.kt.key.clone(),
                    path: path.clone(),
                    generation: f.generation,
                    resets: f.resets,
                }
            };
            fallback.push(FallbackSrc {
                key: k.kt.key.clone(),
                label: Arc::from(k.kt.step_id.as_deref().unwrap_or_default()),
                path,
                fold,
            });
            views.push(view);
        }
        changed |= views != st.fallback;
        if let Some(last) = &st.last {
            changed |= last.partial != partial;
        }

        let reuse = if changed { None } else { st.last.clone() };
        let usage = match reuse {
            Some(last) => last,
            None => {
                // Rule 5 (as amended): the epoch moves only when the series
                // could not stay append-only and was rebuilt canonically.
                let rebuilt = st.series.advance(&st.ledger, ledger_reset, &fallback);
                if rebuilt {
                    st.epoch = st.epoch.saturating_add(1);
                }
                let known_step = |key: &str| st.known_step(key);
                let built = build_usage(
                    &st.ledger,
                    &known_step,
                    &fallback,
                    &st.series.points,
                    partial,
                    st.epoch,
                );
                if cfg!(debug_assertions) && !rebuilt {
                    if let Some(last) = &st.last {
                        // Tokens only: a ledger point's label may legitimately
                        // resolve later (a child row before its ancestor's).
                        let tok = |p: &TurnPoint| {
                            (
                                p.tokens_in,
                                p.tokens_out,
                                p.tokens_cached,
                                p.tokens_cache_write,
                            )
                        };
                        debug_assert!(
                            last.points.len() <= built.points.len()
                                && last
                                    .points
                                    .iter()
                                    .zip(&built.points)
                                    .all(|(a, b)| tok(a) == tok(b)),
                            "usage series changed without an epoch bump"
                        );
                    }
                }
                st.fallback = views;
                let built = Arc::new(built);
                st.last = Some(Arc::clone(&built));
                built
            }
        };

        // Rule 5: seal a terminal run after this full computation (walk
        // included). A result that is partial only because transcripts are
        // missing is sealed too — the seal stats each missing transcript's
        // candidates at ABSENT, so a host pull that delivers one re-validates
        // — rather than re-walking a finished run on every request. An
        // unreadable file is never sealed over.
        if terminal && !unreadable {
            st.seal = Some(Seal { files, mirrors });
        }
        usage
    }

    /// Usage of explicit transcript files (standalone runs, session turns):
    /// the fallback fold alone, each file labelled by the caller, points in the
    /// given order. A path listed twice counts once; a missing or unreadable
    /// file sets `partial`. Only the per-file folds are cached.
    pub fn transcripts_usage(&self, labeled: &[(String, PathBuf)]) -> Arc<RunUsage> {
        let mut seen: HashSet<&Path> = HashSet::new();
        let mut fallback: Vec<FallbackSrc> = Vec::new();
        let mut partial = false;
        let mut resets = 0u64;
        for (label, path) in labeled {
            if !seen.insert(path.as_path()) {
                continue;
            }
            if !path.is_file() {
                partial = true;
                continue;
            }
            let fold = self.file_fold(path);
            {
                let mut f = lock_fold(&fold);
                f.drain(path);
                partial |= f.unreadable;
                resets = resets.saturating_add(f.resets);
            }
            fallback.push(FallbackSrc {
                key: path.to_string_lossy().into_owned(),
                label: Arc::from(label.as_str()),
                path: path.clone(),
                fold,
            });
        }
        let no_ledger = LedgerFold::default();
        let mut series = Series::default();
        series.advance(&no_ledger, false, &fallback);
        let epoch = self.base_epoch.saturating_add(resets);
        let no_labels = |_: &str| None;
        Arc::new(build_usage(
            &no_ledger,
            &no_labels,
            &fallback,
            &series.points,
            partial,
            epoch,
        ))
    }

    /// Every transcript of a run, resolved to the file that serves it on this
    /// coordinator: the known set in order (a transcript that resolves nowhere
    /// keeps its recorded path), then each ledger row's transcript that is not
    /// already listed and exists. Used for aggregate dedupe and netflow.
    pub fn resolved_transcripts(&self, store: &RunStore, run_id: &str) -> Vec<PathBuf> {
        let state = self.run_state(store, run_id);
        let mut st = self.lock_run(&state);
        self.refresh(&mut st, store, run_id);
        Self::resolved_in(&st, store, run_id)
    }

    /// Every path a workflow run claims for the aggregate usage surfaces
    /// (`crate::usage_sources::claimed_transcripts`): each
    /// [`Self::resolved_transcripts`] path, preceded by its canonical form
    /// when it exists. Memoized on a sealed run and dropped whenever the seal
    /// breaks, so a finished run costs the seal's `stat`s, not a resolve and a
    /// `canonicalize` per transcript, on every aggregate request.
    pub fn claimed_paths(&self, store: &RunStore, run_id: &str) -> Arc<Vec<PathBuf>> {
        let state = self.run_state(store, run_id);
        let mut st = self.lock_run(&state);
        self.refresh(&mut st, store, run_id);
        if st.seal.is_some() {
            if let Some(claimed) = &st.claimed {
                return Arc::clone(claimed);
            }
        }
        let mut claimed = Vec::new();
        for p in Self::resolved_in(&st, store, run_id) {
            if p.exists() {
                claimed.push(std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone()));
            }
            claimed.push(p);
        }
        let claimed = Arc::new(claimed);
        if st.seal.is_some() {
            st.claimed = Some(Arc::clone(&claimed));
        }
        claimed
    }

    /// [`Self::resolved_transcripts`] over an already-refreshed state.
    fn resolved_in(st: &RunState, store: &RunStore, run_id: &str) -> Vec<PathBuf> {
        let worker = store.load(run_id).ok().and_then(|r| r.worker_id);
        let global = global_dir_of(store);
        let mut out: Vec<PathBuf> = st
            .known
            .iter()
            .map(|k| {
                resolve(&k.kt.path, &k.kt.key, worker.as_deref(), &global)
                    .unwrap_or_else(|| k.kt.path.clone())
            })
            .collect();
        for (key, path) in &st.ledger.transcripts {
            if st.known_idx.contains_key(key) {
                continue;
            }
            if let Some(p) = resolve(path, key, worker.as_deref(), &global) {
                out.push(p);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_orchestrator::executor::Event as RunEvent;
    use rupu_orchestrator::runs::{RunRecord, RunStatus, StepResultRecord};
    use rupu_orchestrator::usage_ledger::{LedgerKind, LedgerRow, LEDGER_VERSION};
    use rupu_transcript::Event;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    const PROVIDER: &str = "anthropic";
    const MODEL: &str = "claude-test";

    fn record(id: &str, status: RunStatus, worker: Option<&str>) -> RunRecord {
        RunRecord {
            codename: None,
            id: id.into(),
            workflow_name: "wf".into(),
            status,
            inputs: Default::default(),
            event: None,
            workspace_id: String::new(),
            workspace_path: PathBuf::from("."),
            transcript_dir: PathBuf::from("."),
            started_at: Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: worker.map(str::to_string),
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            resume_rerequested_at: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
            gate_decisions: Vec::new(),
            cause: None,
        }
    }

    fn run_store(tmp: &Path) -> RunStore {
        RunStore::new(tmp.join("runs"))
    }

    fn create_run(store: &RunStore, id: &str, status: RunStatus) {
        store.create(record(id, status, None), "").unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn ledger_line(
        id: &str,
        step: Option<&str>,
        unit: Option<usize>,
        agent_run: &str,
        parent: Option<&str>,
        transcript: &Path,
        input: u64,
        output: u64,
        kind: LedgerKind,
    ) -> String {
        let row = LedgerRow {
            v: LEDGER_VERSION,
            id: id.into(),
            at: Utc::now(),
            kind,
            step_id: step.map(str::to_string),
            unit_index: unit,
            unit_key: None,
            agent_run_id: agent_run.into(),
            parent_agent_run_id: parent.map(str::to_string),
            transcript: transcript.to_path_buf(),
            agent: "coder".into(),
            provider: PROVIDER.into(),
            model: MODEL.into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            cache_write_tokens: 0,
        };
        serde_json::to_string(&row).unwrap() + "\n"
    }

    fn event_json(ev: &Event) -> String {
        serde_json::to_string(ev).unwrap() + "\n"
    }

    fn usage_line(
        provider: &str,
        model: &str,
        input: u32,
        output: u32,
        purpose: Option<&str>,
    ) -> String {
        event_json(&Event::Usage {
            provider: provider.into(),
            model: model.into(),
            served_model: None,
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            cache_write_tokens: 0,
            purpose: purpose.map(str::to_string),
        })
    }

    /// A `run_start` event then one `usage` event per `(input, output)`, in
    /// the exact serde shape of `rupu_transcript::Event`.
    fn transcript_lines(agent: &str, provider: &str, model: &str, turns: &[(u32, u32)]) -> String {
        let mut out = event_json(&Event::RunStart {
            codename: None,
            run_id: "run_T".into(),
            workspace_id: "ws".into(),
            agent: agent.into(),
            provider: provider.into(),
            model: model.into(),
            started_at: Utc::now(),
            mode: rupu_transcript::RunMode::Ask,
            schema: None,
            system_prompt: None,
        });
        for &(i, o) in turns {
            out += &usage_line(provider, model, i, o, None);
        }
        out
    }

    fn append(path: &Path, text: &str) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn transcript_fold_streams_lines_skips_bulk_and_clears_on_reset() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jsonl");

        // RunStart + 2 usage turns, interleaved with bulk lines that
        // `apply_line` must skip (a tool_result event and a non-event line) —
        // the kind of content whose per-line owned-String materialisation was
        // the memory spike this fold now avoids.
        append(&p, &event_json(&Event::RunStart {
            codename: None,
            run_id: "run_T".into(),
            workspace_id: "ws".into(),
            agent: "a".into(),
            provider: "anthropic".into(),
            model: "m".into(),
            started_at: Utc::now(),
            mode: rupu_transcript::RunMode::Ask,
            schema: None,
            system_prompt: None,
        }));
        append(&p, "{\"type\":\"tool_result\",\"blob\":\"xxxxxxxxxxxxxxxxxxxx\"}\n");
        append(&p, &usage_line("anthropic", "m", 10, 5, None));
        append(&p, "not even json, a raw delta line\n");
        append(&p, &usage_line("anthropic", "m", 20, 7, None));

        let mut fold = TranscriptFold::default();
        fold.drain(&p);
        assert_eq!(fold.points.len(), 2, "two usage turns folded");
        assert_eq!(fold.turns, 2, "bulk lines did not count as turns");

        // Truncating and rewriting the file (a reset) must clear the stale
        // fold before the new content is applied — not accumulate onto it.
        std::fs::write(&p, "").unwrap();
        append(&p, &transcript_lines("a", "anthropic", "m", &[(1, 1)]));
        fold.drain(&p);
        assert_eq!(fold.points.len(), 1, "reset cleared stale points before applying");
        assert_eq!(fold.resets, 1, "the shrink was counted as a reset");
    }

    fn step_result(step: &str, transcript: &Path) -> StepResultRecord {
        StepResultRecord {
            codename: None,
            step_id: step.into(),
            run_id: rupu_transcript::transcript_key(transcript).unwrap_or_default(),
            transcript_path: transcript.to_path_buf(),
            output: String::new(),
            success: true,
            skipped: false,
            rendered_prompt: String::new(),
            kind: Default::default(),
            items: vec![],
            findings: vec![],
            iterations: 0,
            resolved: true,
            finished_at: Utc::now(),
            loop_iteration: None,
            run_outcome: None,
            host: None,
            cause: None,
            error: None,
        }
    }

    fn unit_started(run_id: &str, step: &str, index: usize, transcript: &Path) -> String {
        let ev = RunEvent::UnitStarted {
            codename: None,
            run_id: run_id.into(),
            step_id: step.into(),
            index,
            unit_key: format!("unit{index}"),
            agent: Some("fanner".into()),
            transcript_path: transcript.to_path_buf(),
            host: None,
        };
        serde_json::to_string(&ev).unwrap() + "\n"
    }

    fn sum(rows: &[rupu_transcript::UsageRow]) -> Tokens {
        let mut t = Tokens::default();
        for r in rows {
            t.input += r.input_tokens;
            t.output += r.output_tokens;
            t.cached += r.cached_tokens;
            t.cache_write += r.cache_write_tokens;
        }
        t
    }

    fn total(u: &RunUsage) -> Tokens {
        sum(&u.rows)
    }

    fn step_total(u: &RunUsage, step: &str) -> Tokens {
        sum(u.by_step.get(step).map(Vec::as_slice).unwrap_or_default())
    }

    fn tok(input: u64, output: u64) -> Tokens {
        Tokens {
            input,
            output,
            cached: 0,
            cache_write: 0,
        }
    }

    /// Everything but `epoch` (two indexes have different base epochs).
    /// `exact_order: false` compares `points` as a multiset: for a run fed by
    /// more than one source, an incremental series is in observation order
    /// while a one-shot fold is in canonical order.
    fn assert_same_usage(a: &RunUsage, b: &RunUsage, exact_order: bool) {
        assert_eq!(a.rows, b.rows, "rows");
        assert_eq!(a.by_step, b.by_step, "by_step");
        assert_eq!(a.by_unit, b.by_unit, "by_unit");
        assert_eq!(a.turns, b.turns, "turns");
        assert_eq!(a.duration_ms, b.duration_ms, "duration_ms");
        assert_eq!(a.partial, b.partial, "partial");
        if exact_order {
            assert_eq!(a.points, b.points, "points");
        } else {
            let key = |p: &TurnPoint| {
                (
                    p.label.clone(),
                    p.tokens_in,
                    p.tokens_out,
                    p.tokens_cached,
                    p.tokens_cache_write,
                )
            };
            let mut pa: Vec<_> = a.points.iter().map(key).collect();
            let mut pb: Vec<_> = b.points.iter().map(key).collect();
            pa.sort();
            pb.sort();
            assert_eq!(pa, pb, "points (as a multiset)");
            for u in [a, b] {
                let turns: Vec<u64> = u.points.iter().map(|p| p.turn).collect();
                let want: Vec<u64> = (1..=u.points.len() as u64).collect();
                assert_eq!(turns, want, "turn is the 1-based index");
            }
        }
    }

    /// `next` only extends `prev`: same epoch, `prev.points` a prefix.
    fn assert_extends(prev: &RunUsage, next: &RunUsage) {
        assert_eq!(next.epoch, prev.epoch, "pure growth keeps the epoch");
        assert!(
            next.points.starts_with(&prev.points),
            "the series must only extend:\n prev {:?}\n next {:?}",
            prev.points,
            next.points
        );
    }

    #[test]
    fn ledger_rows_are_summed_and_deduped_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_LEDGER1", RunStatus::Running);
        let t = tmp.path().join("transcripts/run_A.jsonl");
        let l1 = ledger_line(
            "01ROW1",
            Some("s"),
            None,
            "run_A",
            None,
            &t,
            10,
            5,
            LedgerKind::Turn,
        );
        let l2 = ledger_line(
            "01ROW2",
            Some("s"),
            None,
            "run_A",
            None,
            &t,
            7,
            3,
            LedgerKind::Turn,
        );
        // The same row twice (a mirror replay) plus a garbled line.
        append(
            &store.usage_ledger_path("run_LEDGER1"),
            &format!("{l1}{l1}{{not json\n{l2}"),
        );

        let u = UsageIndex::default().run_usage(&store, "run_LEDGER1");
        assert_eq!(total(&u), tok(17, 8));
        assert_eq!(u.turns, 2);
        assert_eq!(u.points.len(), 2);
        assert_eq!(u.rows.len(), 1);
        assert_eq!(
            u.rows[0].runs, 1,
            "one agent run = one contributing transcript"
        );
        assert_eq!(step_total(&u, "s"), tok(17, 8));
        assert!(!u.partial);
    }

    #[test]
    fn transcript_with_ledger_rows_is_not_double_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_DOUBLE", RunStatus::Running);
        let t = tmp.path().join("transcripts/run_A.jsonl");
        append(
            &t,
            &transcript_lines("coder", PROVIDER, MODEL, &[(100, 10), (50, 5)]),
        );
        append(
            &store.usage_ledger_path("run_DOUBLE"),
            &[
                ledger_line(
                    "01D1",
                    Some("s"),
                    None,
                    "run_A",
                    None,
                    &t,
                    100,
                    10,
                    LedgerKind::Turn,
                ),
                ledger_line(
                    "01D2",
                    Some("s"),
                    None,
                    "run_A",
                    None,
                    &t,
                    50,
                    5,
                    LedgerKind::Turn,
                ),
            ]
            .concat(),
        );
        store
            .append_step_result("run_DOUBLE", &step_result("s", &t))
            .unwrap();

        let u = UsageIndex::default().run_usage(&store, "run_DOUBLE");
        assert_eq!(total(&u), tok(150, 15), "ledger total only");
        assert_eq!(u.turns, 2);
        assert_eq!(u.points.len(), 2);
        assert_eq!(step_total(&u, "s"), tok(150, 15));
        assert!(!u.partial);
    }

    #[test]
    fn known_transcript_without_ledger_rows_falls_back_to_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_LEGACY", RunStatus::Running);
        let a = tmp.path().join("transcripts/run_A.jsonl");
        let b = tmp.path().join("transcripts/run_B.jsonl");
        append(
            &a,
            &transcript_lines("builder", PROVIDER, MODEL, &[(100, 10), (200, 20)]),
        );
        append(&b, &transcript_lines("fanner", PROVIDER, MODEL, &[(7, 1)]));
        // A: a finished step (step_results). B: an in-flight unit (events only).
        store
            .append_step_result("run_LEGACY", &step_result("build", &a))
            .unwrap();
        append(
            &store.events_path("run_LEGACY"),
            &unit_started("run_LEGACY", "fan", 0, &b),
        );

        let u = UsageIndex::default().run_usage(&store, "run_LEGACY");
        assert_eq!(total(&u), tok(307, 31));
        assert_eq!(u.turns, 3, "one turn per usage event");
        assert_eq!(step_total(&u, "build"), tok(300, 30));
        assert_eq!(step_total(&u, "fan"), tok(7, 1));
        let labels: Vec<&str> = u.points.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["build", "build", "fan"]);
        let turns: Vec<u64> = u.points.iter().map(|p| p.turn).collect();
        assert_eq!(turns, [1, 2, 3]);
        assert_eq!(u.rows.len(), 2, "two agents");
        assert!(u.rows.iter().all(|r| r.runs == 1));
        assert!(!u.partial);
    }

    /// `line` (one [`ledger_line`]) with `cache_write_tokens` set to `n`.
    fn with_cache_write(line: String, n: u64) -> String {
        let mut row: LedgerRow = serde_json::from_str(line.trim_end()).unwrap();
        row.cache_write_tokens = n;
        serde_json::to_string(&row).unwrap() + "\n"
    }

    #[test]
    fn cache_write_tokens_fold_from_ledger_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_CWLEDGER", RunStatus::Running);
        let t = tmp.path().join("transcripts/run_A.jsonl");
        append(
            &store.usage_ledger_path("run_CWLEDGER"),
            &[
                with_cache_write(
                    ledger_line(
                        "01CW1",
                        Some("s"),
                        Some(0),
                        "run_A",
                        None,
                        &t,
                        100,
                        10,
                        LedgerKind::Turn,
                    ),
                    30,
                ),
                with_cache_write(
                    ledger_line(
                        "01CW2",
                        Some("s"),
                        Some(0),
                        "run_A",
                        None,
                        &t,
                        50,
                        5,
                        LedgerKind::Turn,
                    ),
                    5,
                ),
            ]
            .concat(),
        );

        let u = UsageIndex::default().run_usage(&store, "run_CWLEDGER");
        assert_eq!(u.rows.len(), 1);
        assert_eq!(u.rows[0].cache_write_tokens, 35);
        assert_eq!(u.rows[0].input_tokens, 150);
        assert_eq!(u.by_step["s"][0].cache_write_tokens, 35);
        assert_eq!(u.by_unit[&("s".to_string(), 0)].cache_write, 35);
        let points: Vec<u64> = u.points.iter().map(|p| p.tokens_cache_write).collect();
        assert_eq!(points, [30, 5]);
    }

    #[test]
    fn cache_write_tokens_fold_from_a_fallback_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_CWFALLBACK", RunStatus::Running);
        let a = tmp.path().join("transcripts/run_A.jsonl");
        let mut text = transcript_lines("builder", PROVIDER, MODEL, &[]);
        text += &event_json(&Event::Usage {
            provider: PROVIDER.into(),
            model: MODEL.into(),
            served_model: None,
            input_tokens: 90,
            output_tokens: 9,
            cached_tokens: 20,
            cache_write_tokens: 7,
            purpose: None,
        });
        append(&a, &text);
        store
            .append_step_result("run_CWFALLBACK", &step_result("build", &a))
            .unwrap();

        let u = UsageIndex::default().run_usage(&store, "run_CWFALLBACK");
        assert_eq!(u.rows.len(), 1);
        assert_eq!(u.rows[0].cache_write_tokens, 7);
        assert_eq!(u.rows[0].cached_tokens, 20);
        assert_eq!(u.by_step["build"][0].cache_write_tokens, 7);
        assert_eq!(u.points.len(), 1);
        assert_eq!(u.points[0].tokens_cache_write, 7);

        // The same file through the session/standalone entry point.
        let s = UsageIndex::default().transcripts_usage(&[("turn".into(), a.clone())]);
        assert_eq!(s.rows[0].cache_write_tokens, 7);
    }

    #[test]
    fn dispatch_child_is_attributed_to_ancestor_step() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_DISPATCH", RunStatus::Running);
        let p = tmp.path().join("transcripts/run_P.jsonl");
        let c = store.sub_run_transcript_path("run_P", "sub_C");
        let g = store.sub_run_transcript_path("sub_C", "sub_G");
        // Grandchild first: attribution is resolved at finish, not on arrival.
        append(
            &store.usage_ledger_path("run_DISPATCH"),
            &[
                ledger_line(
                    "01G",
                    None,
                    None,
                    "sub_G",
                    Some("sub_C"),
                    &g,
                    30,
                    3,
                    LedgerKind::Turn,
                ),
                ledger_line(
                    "01C",
                    None,
                    None,
                    "sub_C",
                    Some("run_P"),
                    &c,
                    20,
                    2,
                    LedgerKind::Turn,
                ),
                ledger_line(
                    "01P",
                    Some("s"),
                    Some(1),
                    "run_P",
                    None,
                    &p,
                    10,
                    1,
                    LedgerKind::Turn,
                ),
            ]
            .concat(),
        );

        let u = UsageIndex::default().run_usage(&store, "run_DISPATCH");
        assert_eq!(step_total(&u, "s"), tok(60, 6));
        assert!(!u.by_step.contains_key(""), "{:?}", u.by_step.keys());
        assert_eq!(u.by_unit.get(&("s".to_string(), 1)), Some(&tok(60, 6)));
        assert_eq!(u.rows.len(), 1);
        assert_eq!(u.rows[0].runs, 3, "three agent runs contributed");
        assert!(u.points.iter().all(|p| p.label == "s"));
    }

    #[test]
    fn compaction_rows_count_tokens_not_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_COMPACT", RunStatus::Running);
        let t = tmp.path().join("transcripts/run_A.jsonl");
        append(
            &store.usage_ledger_path("run_COMPACT"),
            &[
                ledger_line(
                    "01T",
                    Some("s"),
                    None,
                    "run_A",
                    None,
                    &t,
                    10,
                    1,
                    LedgerKind::Turn,
                ),
                ledger_line(
                    "01K",
                    Some("s"),
                    None,
                    "run_A",
                    None,
                    &t,
                    1000,
                    100,
                    LedgerKind::Compaction,
                ),
            ]
            .concat(),
        );
        // The fallback side follows the same rule for `purpose: compaction`.
        let f = tmp.path().join("transcripts/run_F.jsonl");
        append(
            &f,
            &(transcript_lines("legacy", PROVIDER, MODEL, &[(5, 1)])
                + &usage_line(PROVIDER, MODEL, 500, 50, Some("compaction"))),
        );
        store
            .append_step_result("run_COMPACT", &step_result("old", &f))
            .unwrap();

        let u = UsageIndex::default().run_usage(&store, "run_COMPACT");
        assert_eq!(u.turns, 2, "one ledger turn + one transcript turn");
        assert_eq!(
            total(&u),
            tok(1515, 152),
            "compaction spend is still counted"
        );
        assert_eq!(u.points.len(), 4, "compaction spend still plots");
    }

    #[test]
    fn unreadable_known_transcript_sets_partial() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        let idx = UsageIndex::default();

        // A completed step whose transcript is gone.
        create_run(&store, "run_GONE", RunStatus::Completed);
        store
            .append_step_result(
                "run_GONE",
                &step_result("s", Path::new("/nope/run_X.jsonl")),
            )
            .unwrap();
        assert!(idx.run_usage(&store, "run_GONE").partial);

        // A mirrored remote run whose events name a transcript that resolves nowhere.
        store
            .create(
                record("run_REMOTEGONE", RunStatus::Running, Some("host_zz")),
                "",
            )
            .unwrap();
        append(
            &store.events_path("run_REMOTEGONE"),
            &unit_started("run_REMOTEGONE", "fan", 0, Path::new("/nope/run_Y.jsonl")),
        );
        assert!(idx.run_usage(&store, "run_REMOTEGONE").partial);
    }

    fn is_sealed(idx: &UsageIndex, store: &RunStore, run_id: &str) -> bool {
        let state = idx.run_state(store, run_id);
        let st = idx.lock_run(&state);
        st.seal.is_some()
    }

    #[test]
    fn terminal_partial_run_is_sealed_until_the_missing_transcript_lands() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        let idx = UsageIndex::default();
        create_run(&store, "run_HALF", RunStatus::Completed);
        let present = tmp.path().join("transcripts/run_P.jsonl");
        let missing = tmp.path().join("transcripts/run_M.jsonl");
        append(
            &present,
            &transcript_lines("builder", PROVIDER, MODEL, &[(10, 1)]),
        );
        store
            .append_step_result("run_HALF", &step_result("a", &present))
            .unwrap();
        store
            .append_step_result("run_HALF", &step_result("b", &missing))
            .unwrap();

        let first = idx.run_usage(&store, "run_HALF");
        assert!(first.partial);
        assert_eq!(total(&first), tok(10, 1));
        assert!(
            is_sealed(&idx, &store, "run_HALF"),
            "a finished run with a permanently missing transcript must not re-walk every call"
        );
        let second = idx.run_usage(&store, "run_HALF");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(is_sealed(&idx, &store, "run_HALF"));

        // The missing transcript appears (a late host pull): the seal breaks,
        // it is counted, and the run is no longer partial.
        append(
            &missing,
            &transcript_lines("builder", PROVIDER, MODEL, &[(20, 2)]),
        );
        let third = idx.run_usage(&store, "run_HALF");
        assert!(!third.partial);
        assert_eq!(total(&third), tok(30, 3));
        assert_eq!(step_total(&third, "b"), tok(20, 2));
        assert!(is_sealed(&idx, &store, "run_HALF"));
    }

    /// The layouts a placed unit/step leaves on a LOCAL run (no `worker_id`)
    /// since #646: its transcript is recorded AS the coordinator's agent-mirror
    /// path, so `resolve` returns the recorded path itself — only placement
    /// says a host pull may still grow it.
    fn placed_layouts(tmp: &Path, store: &RunStore) -> Vec<(&'static str, PathBuf)> {
        let mut out = Vec::new();

        // A `distribute:` unit: `unit_started` carries the host.
        let unit = agent_mirror_path(tmp, "run_UNIT7");
        create_run(store, "run_PLACEDUNIT", RunStatus::Completed);
        let ev = RunEvent::UnitStarted {
            codename: None,
            run_id: "run_PLACEDUNIT".into(),
            step_id: "fan".into(),
            index: 0,
            unit_key: "unit0".into(),
            agent: Some("fanner".into()),
            transcript_path: unit.clone(),
            host: Some("mini".into()),
        };
        append(
            &store.events_path("run_PLACEDUNIT"),
            &(serde_json::to_string(&ev).unwrap() + "\n"),
        );
        out.push(("run_PLACEDUNIT", unit));

        // A `host:` linear step: the host is on `step_started`; the transcript
        // arrives via `step_working`, which names none.
        let step = agent_mirror_path(tmp, "run_STEP8");
        create_run(store, "run_PLACEDSTEP", RunStatus::Completed);
        for ev in [
            RunEvent::StepStarted {
                codename: None,
                run_id: "run_PLACEDSTEP".into(),
                step_id: "fan".into(),
                kind: rupu_orchestrator::runs::StepKind::Linear,
                agent: Some("fanner".into()),
                host: Some("mini".into()),
            },
            RunEvent::StepWorking {
                run_id: "run_PLACEDSTEP".into(),
                step_id: "fan".into(),
                note: None,
                transcript_path: Some(step.clone()),
            },
        ] {
            append(
                &store.events_path("run_PLACEDSTEP"),
                &(serde_json::to_string(&ev).unwrap() + "\n"),
            );
        }
        out.push(("run_PLACEDSTEP", step));

        // Only the durable step result survives (events rotated/absent): its
        // `host` says where the step ran.
        let durable = agent_mirror_path(tmp, "run_STEP9");
        create_run(store, "run_PLACEDRESULT", RunStatus::Completed);
        let mut sr = step_result("fan", &durable);
        sr.host = Some("mini".into());
        store.append_step_result("run_PLACEDRESULT", &sr).unwrap();
        out.push(("run_PLACEDRESULT", durable));

        out
    }

    #[test]
    fn sealed_local_run_revalidates_a_growing_placed_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        let idx = UsageIndex::default();
        for (run_id, transcript) in placed_layouts(tmp.path(), &store) {
            append(
                &transcript,
                &transcript_lines("fanner", PROVIDER, MODEL, &[(10, 1)]),
            );
            let first = idx.run_usage(&store, run_id);
            assert_eq!(total(&first), tok(10, 1), "{run_id}");
            assert!(!first.partial, "{run_id}");
            assert!(is_sealed(&idx, &store, run_id), "{run_id}");

            // A late host pull/tail grows the mirror — no run file changes. The
            // seal re-validates and the new turn counts.
            append(&transcript, &usage_line(PROVIDER, MODEL, 20, 2, None));
            let second = idx.run_usage(&store, run_id);
            assert_eq!(total(&second), tok(30, 3), "{run_id}");
            assert_eq!(step_total(&second, "fan"), tok(30, 3), "{run_id}");
            assert_extends(&first, &second);
            assert!(is_sealed(&idx, &store, run_id), "{run_id}");
        }
    }

    #[tokio::test]
    async fn terminal_partial_mirrored_run_unseals_when_the_cache_fills() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _recorded) = crate::usage::tests::seed_remote_run(tmp.path()).await;
        let mut rec = store.load("run_01USAGE").unwrap();
        rec.status = RunStatus::Completed;
        store.update(&rec).unwrap();

        let idx = UsageIndex::default();
        let first = idx.run_usage(&store, "run_01USAGE");
        assert!(first.partial, "nothing mirrored yet");
        assert!(is_sealed(&idx, &store, "run_01USAGE"));

        // The terminal pull lands the worker-cache copy.
        let cache = tmp
            .path()
            .join("mirror")
            .join("host_abc")
            .join("transcripts")
            .join("run_01A.jsonl");
        append(
            &cache,
            &transcript_lines("remote", PROVIDER, MODEL, &[(40, 4)]),
        );
        let second = idx.run_usage(&store, "run_01USAGE");
        assert!(!second.partial);
        assert_eq!(total(&second), tok(40, 4));
    }

    #[test]
    fn events_only_missing_transcript_on_a_local_run_is_not_partial() {
        // A unit that just started hasn't created its transcript yet: that is
        // "no usage yet", not missing data.
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_JUSTSTARTED", RunStatus::Running);
        let t = tmp.path().join("transcripts/run_NEW.jsonl");
        append(
            &store.events_path("run_JUSTSTARTED"),
            &unit_started("run_JUSTSTARTED", "fan", 0, &t),
        );
        let idx = UsageIndex::default();
        let u = idx.run_usage(&store, "run_JUSTSTARTED");
        assert!(!u.partial);
        assert_eq!(total(&u), Tokens::default());

        // ...and once the file lands, its usage shows up.
        append(&t, &transcript_lines("fanner", PROVIDER, MODEL, &[(3, 2)]));
        assert_eq!(total(&idx.run_usage(&store, "run_JUSTSTARTED")), tok(3, 2));
    }

    #[test]
    fn incremental_append_equals_one_shot() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_INCR", RunStatus::Running);
        let ledger = store.usage_ledger_path("run_INCR");
        let ta = tmp.path().join("transcripts/run_A.jsonl");
        let l = |n: u64| {
            ledger_line(
                &format!("01I{n}"),
                Some("s"),
                Some(0),
                "run_A",
                None,
                &ta,
                n,
                1,
                LedgerKind::Turn,
            )
        };
        append(&ledger, &[l(1), l(2)].concat());
        // A fallback transcript (no ledger rows) that grows alongside.
        let tf = tmp.path().join("transcripts/run_F.jsonl");
        append(
            &tf,
            &transcript_lines("legacy", PROVIDER, MODEL, &[(1000, 100)]),
        );
        store
            .append_step_result("run_INCR", &step_result("old", &tf))
            .unwrap();

        let idx = UsageIndex::default();
        let first = idx.run_usage(&store, "run_INCR");
        assert_eq!(total(&first).input, 1 + 2 + 1000);

        // Three whole ledger rows + half of a fourth; one whole usage event +
        // half of another.
        let sixth = l(6);
        let (head, tail) = sixth.split_at(sixth.len() / 2);
        append(&ledger, &format!("{}{}{}{head}", l(3), l(4), l(5)));
        let late = usage_line(PROVIDER, MODEL, 2000, 200, None);
        let (fhead, ftail) = late.split_at(late.len() / 2);
        append(
            &tf,
            &format!("{}{fhead}", usage_line(PROVIDER, MODEL, 3000, 300, None)),
        );

        let second = idx.run_usage(&store, "run_INCR");
        assert_eq!(
            total(&second).input,
            1 + 2 + 3 + 4 + 5 + 1000 + 3000,
            "half-written lines are not counted yet"
        );
        assert_eq!(second.turns, 5 + 2);
        assert_extends(&first, &second);

        append(&ledger, tail);
        append(&tf, ftail);
        let third = idx.run_usage(&store, "run_INCR");
        assert_extends(&second, &third);
        let fresh = UsageIndex::default().run_usage(&store, "run_INCR");
        assert_same_usage(&third, &fresh, false);
        assert_eq!(total(&third).input, 21 + 6000);
        assert_eq!(third.turns, 6 + 3);
    }

    #[test]
    fn unchanged_run_returns_same_arc() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        let idx = UsageIndex::default();
        let t = tmp.path().join("transcripts/run_A.jsonl");
        let f = tmp.path().join("transcripts/run_F.jsonl");
        append(&f, &transcript_lines("legacy", PROVIDER, MODEL, &[(9, 9)]));

        // A live run: ledger + a fallback transcript.
        create_run(&store, "run_LIVE", RunStatus::Running);
        append(
            &store.usage_ledger_path("run_LIVE"),
            &ledger_line(
                "01L1",
                Some("s"),
                None,
                "run_A",
                None,
                &t,
                1,
                1,
                LedgerKind::Turn,
            ),
        );
        store
            .append_step_result("run_LIVE", &step_result("old", &f))
            .unwrap();
        let a = idx.run_usage(&store, "run_LIVE");
        let b = idx.run_usage(&store, "run_LIVE");
        assert!(Arc::ptr_eq(&a, &b));

        // A terminal run is sealed; a ledger append still re-validates it.
        create_run(&store, "run_DONE", RunStatus::Completed);
        let done_ledger = store.usage_ledger_path("run_DONE");
        append(
            &done_ledger,
            &ledger_line(
                "01S1",
                Some("s"),
                None,
                "run_A",
                None,
                &t,
                5,
                5,
                LedgerKind::Turn,
            ),
        );
        let c1 = idx.run_usage(&store, "run_DONE");
        let c2 = idx.run_usage(&store, "run_DONE");
        assert!(Arc::ptr_eq(&c1, &c2));
        append(
            &done_ledger,
            &ledger_line(
                "01S2",
                Some("s"),
                None,
                "run_A",
                None,
                &t,
                6,
                6,
                LedgerKind::Turn,
            ),
        );
        let c3 = idx.run_usage(&store, "run_DONE");
        assert!(!Arc::ptr_eq(&c1, &c3));
        assert_eq!(total(&c3), tok(11, 11));
    }

    #[test]
    fn epoch_is_stable_for_appends_and_bumps_on_a_ledger_reset() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_EPOCH", RunStatus::Running);
        let ledger = store.usage_ledger_path("run_EPOCH");
        let t = tmp.path().join("transcripts/run_A.jsonl");
        let row = |id: &str, n: u64| {
            ledger_line(
                id,
                Some("s"),
                None,
                "run_A",
                None,
                &t,
                n,
                1,
                LedgerKind::Turn,
            )
        };
        append(&ledger, &[row("01E1", 100), row("01E2", 200)].concat());

        let idx = UsageIndex::default();
        let e1 = idx.run_usage(&store, "run_EPOCH").epoch;
        assert_eq!(
            e1, idx.base_epoch,
            "a new run state starts at the index's base epoch"
        );
        assert!(
            e1 > 1,
            "base epoch is wall-clock nanos, so it differs across restarts"
        );

        append(&ledger, &row("01E3", 300));
        let appended = idx.run_usage(&store, "run_EPOCH");
        assert_eq!(appended.epoch, e1, "pure appends keep the epoch");
        assert_eq!(total(&appended).input, 600);
        // A single-source run: incremental order IS canonical order.
        let fresh = UsageIndex::default().run_usage(&store, "run_EPOCH");
        assert_same_usage(&appended, &fresh, true);

        // The file shrank: the ledger fold restarts and the epoch moves on.
        std::fs::write(&ledger, row("01E9", 7)).unwrap();
        let reset = idx.run_usage(&store, "run_EPOCH");
        assert_eq!(reset.epoch, e1 + 1);
        assert_eq!(total(&reset), tok(7, 1));
        assert_eq!(reset.points.len(), 1);

        // Replaced by a LONGER file (new inode, e.g. a mirror's atomic
        // rename): still a reset, so still a rebuild.
        let replacement = ledger.with_extension("jsonl.tmp");
        std::fs::write(
            &replacement,
            [row("01F1", 1), row("01F2", 2), row("01F3", 3)].concat(),
        )
        .unwrap();
        std::fs::rename(&replacement, &ledger).unwrap();
        let replaced = idx.run_usage(&store, "run_EPOCH");
        assert_eq!(replaced.epoch, e1 + 2);
        assert_eq!(total(&replaced), tok(6, 3));
        let fresh = UsageIndex::default().run_usage(&store, "run_EPOCH");
        assert_same_usage(&replaced, &fresh, true);
    }

    /// A sealed terminal run whose ledger is REPLACED by rename with content of
    /// the same length and the same mtime (a mirror's authoritative terminal
    /// pull landing inside the filesystem's mtime granularity) still
    /// re-validates: the seal compares the inode too.
    #[cfg(unix)]
    #[test]
    fn sealed_terminal_run_revalidates_an_equal_length_ledger_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_SWAP", RunStatus::Completed);
        let ledger = store.usage_ledger_path("run_SWAP");
        let t = tmp.path().join("transcripts/run_A.jsonl");
        let original = ledger_line(
            "01W1",
            Some("s"),
            None,
            "run_A",
            None,
            &t,
            100,
            1,
            LedgerKind::Turn,
        );
        append(&ledger, &original);

        let idx = UsageIndex::default();
        let first = idx.run_usage(&store, "run_SWAP");
        assert_eq!(total(&first), tok(100, 1));
        assert!(is_sealed(&idx, &store, "run_SWAP"));

        // Same length, different tokens, same mtime — only the inode moves.
        let replaced = original.replace("\"input_tokens\":100", "\"input_tokens\":200");
        assert_ne!(replaced, original);
        assert_eq!(replaced.len(), original.len());
        let mtime = std::fs::metadata(&ledger).unwrap().modified().unwrap();
        let staged = ledger.with_extension("jsonl.swap");
        std::fs::write(&staged, &replaced).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&staged)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        std::fs::rename(&staged, &ledger).unwrap();
        let m = std::fs::metadata(&ledger).unwrap();
        assert_eq!(
            (m.len(), m.modified().unwrap()),
            (original.len() as u64, mtime),
            "precondition: length and mtime are unchanged"
        );

        let second = idx.run_usage(&store, "run_SWAP");
        assert!(
            !Arc::ptr_eq(&first, &second),
            "a replaced ledger must break the seal"
        );
        assert_eq!(total(&second), tok(200, 1));
        assert!(is_sealed(&idx, &store, "run_SWAP"));
    }

    #[test]
    fn a_zero_point_fallback_key_never_bumps_the_epoch() {
        // An all-local ledger run: a new unit's transcript is announced
        // (RunStart written, no usage yet) before its first ledger row.
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_ZERO", RunStatus::Running);
        let a = tmp.path().join("transcripts/run_A.jsonl");
        let b = tmp.path().join("transcripts/run_B.jsonl");
        let ledger = store.usage_ledger_path("run_ZERO");
        append(
            &ledger,
            &ledger_line(
                "01Z1",
                Some("s"),
                Some(0),
                "run_A",
                None,
                &a,
                10,
                1,
                LedgerKind::Turn,
            ),
        );
        let idx = UsageIndex::default();
        let u0 = idx.run_usage(&store, "run_ZERO");

        append(&b, &transcript_lines("fanner", PROVIDER, MODEL, &[]));
        append(
            &store.events_path("run_ZERO"),
            &unit_started("run_ZERO", "s", 1, &b),
        );
        let u1 = idx.run_usage(&store, "run_ZERO");
        assert_extends(&u0, &u1);
        assert_eq!(
            u1.rows.iter().map(|r| r.runs).sum::<u64>(),
            2,
            "the new unit is a zero row"
        );

        // Its first call lands in the ledger: the key leaves the fallback set
        // with nothing of it emitted.
        append(
            &ledger,
            &ledger_line(
                "01Z2",
                Some("s"),
                Some(1),
                "run_B",
                None,
                &b,
                20,
                2,
                LedgerKind::Turn,
            ),
        );
        let u2 = idx.run_usage(&store, "run_ZERO");
        assert_extends(&u1, &u2);
        assert_eq!(total(&u2), tok(30, 3));
    }

    #[test]
    fn a_mixed_run_series_only_extends_as_both_sources_grow() {
        // Ledger rows for a local step (run_A) + a fallback transcript (run_B,
        // e.g. a placed unit's mirror), appended to alternately.
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_MIX", RunStatus::Running);
        let a = tmp.path().join("transcripts/run_A.jsonl");
        let b = tmp.path().join("transcripts/run_B.jsonl");
        let ledger = store.usage_ledger_path("run_MIX");
        append(
            &ledger,
            &ledger_line(
                "01X0",
                Some("s"),
                None,
                "run_A",
                None,
                &a,
                1,
                1,
                LedgerKind::Turn,
            ),
        );
        append(
            &b,
            &transcript_lines("remote", PROVIDER, MODEL, &[(100, 10)]),
        );
        store
            .append_step_result("run_MIX", &step_result("placed", &b))
            .unwrap();

        let idx = UsageIndex::default();
        let mut prev = idx.run_usage(&store, "run_MIX");
        let e0 = prev.epoch;
        for i in 1..=6u32 {
            if i % 2 == 1 {
                let id = format!("01X{i}");
                append(
                    &ledger,
                    &ledger_line(
                        &id,
                        Some("s"),
                        None,
                        "run_A",
                        None,
                        &a,
                        u64::from(i),
                        1,
                        LedgerKind::Turn,
                    ),
                );
            } else {
                append(&b, &usage_line(PROVIDER, MODEL, 100 * i, 10, None));
            }
            let next = idx.run_usage(&store, "run_MIX");
            assert_extends(&prev, &next);
            assert_eq!(next.points.len(), prev.points.len() + 1);
            prev = next;
        }
        assert_eq!(prev.epoch, e0);
        let labels: Vec<&str> = prev.points.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            labels,
            ["s", "placed", "s", "placed", "s", "placed", "s", "placed"]
        );
        // Totals equal a one-shot fold; only the point order differs.
        let fresh = UsageIndex::default().run_usage(&store, "run_MIX");
        assert_same_usage(&prev, &fresh, false);
    }

    #[test]
    fn growing_fallback_transcripts_keep_the_epoch() {
        // A legacy (no-ledger) run whose two transcripts grow alternately:
        // growth of the non-last one must not reorder the series.
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_TWO", RunStatus::Running);
        let f1 = tmp.path().join("transcripts/run_F1.jsonl");
        let f2 = tmp.path().join("transcripts/run_F2.jsonl");
        append(&f1, &transcript_lines("one", PROVIDER, MODEL, &[(1, 1)]));
        append(&f2, &transcript_lines("two", PROVIDER, MODEL, &[(2, 2)]));
        store
            .append_step_result("run_TWO", &step_result("one", &f1))
            .unwrap();
        append(
            &store.events_path("run_TWO"),
            &unit_started("run_TWO", "two", 0, &f2),
        );

        let idx = UsageIndex::default();
        let mut prev = idx.run_usage(&store, "run_TWO");
        for i in 1..=4u32 {
            let f = if i % 2 == 1 { &f1 } else { &f2 };
            append(f, &usage_line(PROVIDER, MODEL, 10 * i, 1, None));
            let next = idx.run_usage(&store, "run_TWO");
            assert_extends(&prev, &next);
            assert_eq!(next.points.len(), prev.points.len() + 1);
            prev = next;
        }
        let fresh = UsageIndex::default().run_usage(&store, "run_TWO");
        assert_same_usage(&prev, &fresh, false);
    }

    #[test]
    fn a_fallback_file_shrink_bumps_the_epoch() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_SHRINK", RunStatus::Running);
        let f = tmp.path().join("transcripts/run_F.jsonl");
        append(
            &f,
            &transcript_lines("legacy", PROVIDER, MODEL, &[(10, 1), (20, 2)]),
        );
        store
            .append_step_result("run_SHRINK", &step_result("old", &f))
            .unwrap();
        let idx = UsageIndex::default();
        let u0 = idx.run_usage(&store, "run_SHRINK");
        assert_eq!(u0.points.len(), 2);

        std::fs::write(&f, transcript_lines("legacy", PROVIDER, MODEL, &[(5, 5)])).unwrap();
        let u1 = idx.run_usage(&store, "run_SHRINK");
        assert_eq!(u1.epoch, u0.epoch + 1);
        assert_eq!(total(&u1), tok(5, 5));
        assert_eq!(u1.points.len(), 1);
        let fresh = UsageIndex::default().run_usage(&store, "run_SHRINK");
        assert_same_usage(&u1, &fresh, true);
    }

    #[test]
    fn a_key_moving_from_fallback_to_ledger_after_emitting_bumps_and_goes_canonical() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_MOVE", RunStatus::Running);
        let a = tmp.path().join("transcripts/run_A.jsonl");
        let b = tmp.path().join("transcripts/run_B.jsonl");
        let ledger = store.usage_ledger_path("run_MOVE");
        // run_B is first seen as a fallback transcript; its point is emitted.
        append(&b, &transcript_lines("coder", PROVIDER, MODEL, &[(5, 5)]));
        append(
            &store.events_path("run_MOVE"),
            &unit_started("run_MOVE", "fan", 0, &b),
        );
        let idx = UsageIndex::default();
        let u0 = idx.run_usage(&store, "run_MOVE");

        // Another agent's ledger row appends in observation order.
        append(
            &ledger,
            &ledger_line(
                "01V1",
                Some("s"),
                None,
                "run_A",
                None,
                &a,
                1,
                1,
                LedgerKind::Turn,
            ),
        );
        let u1 = idx.run_usage(&store, "run_MOVE");
        assert_extends(&u0, &u1);
        let labels: Vec<&str> = u1.points.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["fan", "s"]);

        // run_B's own ledger row lands: its emitted fallback point must go, so
        // the series is rebuilt canonically and the epoch moves.
        append(
            &ledger,
            &ledger_line(
                "01V2",
                Some("fan"),
                Some(0),
                "run_B",
                None,
                &b,
                5,
                5,
                LedgerKind::Turn,
            ),
        );
        let u2 = idx.run_usage(&store, "run_MOVE");
        assert_eq!(u2.epoch, u0.epoch + 1);
        let labels: Vec<&str> = u2.points.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            labels,
            ["s", "fan"],
            "canonical: ledger points in ledger order"
        );
        assert_eq!(total(&u2), tok(6, 6));
        let fresh = UsageIndex::default().run_usage(&store, "run_MOVE");
        assert_same_usage(&u2, &fresh, true);
    }

    #[test]
    fn legacy_run_walk_finds_dispatch_children() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_WALK", RunStatus::Completed);
        let step = tmp.path().join("transcripts/run_STEPAGENT.jsonl");
        append(
            &step,
            &transcript_lines("builder", PROVIDER, MODEL, &[(10, 1)]),
        );
        store
            .append_step_result("run_WALK", &step_result("build", &step))
            .unwrap();
        // A legacy dispatch child: no ledger, no DispatchStarted event.
        let (_child, child_path) = store.create_sub_run("run_STEPAGENT", "helper").unwrap();
        append(
            &child_path,
            &transcript_lines("helper", PROVIDER, MODEL, &[(5, 5)]),
        );

        let u = UsageIndex::default().run_usage(&store, "run_WALK");
        assert_eq!(total(&u), tok(15, 6));
        assert_eq!(step_total(&u, "build"), tok(10, 1));
        assert_eq!(
            step_total(&u, ""),
            tok(5, 5),
            "a walk-found child is unattributed"
        );
        assert!(!u.partial);
    }

    #[tokio::test]
    async fn remote_worker_run_resolves_mirror_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, recorded) = crate::usage::tests::seed_remote_run(tmp.path()).await;
        assert!(!recorded.exists());
        let cache = tmp
            .path()
            .join("mirror")
            .join("host_abc")
            .join("transcripts")
            .join("run_01A.jsonl");
        append(
            &cache,
            &transcript_lines("remote", PROVIDER, MODEL, &[(40, 4), (60, 6)]),
        );

        let idx = UsageIndex::default();
        let u = idx.run_usage(&store, "run_01USAGE");
        assert_eq!(total(&u), tok(100, 10));
        assert_eq!(step_total(&u, "a"), tok(100, 10));
        assert!(!u.partial);
        assert_eq!(idx.resolved_transcripts(&store, "run_01USAGE"), vec![cache]);
    }

    #[test]
    fn resolved_transcripts_cover_known_and_existing_ledger_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_RESOLVE", RunStatus::Running);
        let s = tmp.path().join("transcripts/run_S.jsonl");
        let l = tmp.path().join("transcripts/run_L.jsonl");
        let m = tmp.path().join("transcripts/run_M.jsonl");
        append(&s, &transcript_lines("a", PROVIDER, MODEL, &[(1, 1)]));
        append(&l, &transcript_lines("a", PROVIDER, MODEL, &[(1, 1)]));
        store
            .append_step_result("run_RESOLVE", &step_result("s", &s))
            .unwrap();
        append(
            &store.usage_ledger_path("run_RESOLVE"),
            &[
                ledger_line(
                    "01R1",
                    Some("l"),
                    None,
                    "run_L",
                    None,
                    &l,
                    1,
                    1,
                    LedgerKind::Turn,
                ),
                ledger_line(
                    "01R2",
                    Some("m"),
                    None,
                    "run_M",
                    None,
                    &m,
                    1,
                    1,
                    LedgerKind::Turn,
                ),
            ]
            .concat(),
        );

        let got = UsageIndex::default().resolved_transcripts(&store, "run_RESOLVE");
        assert_eq!(
            got,
            vec![s, l],
            "known first, then ledger transcripts that exist"
        );
    }

    #[test]
    fn claimed_paths_are_memoized_while_sealed_and_follow_a_broken_seal() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        create_run(&store, "run_CLAIM", RunStatus::Completed);
        let a = tmp.path().join("transcripts/run_A.jsonl");
        append(&a, &transcript_lines("a", PROVIDER, MODEL, &[(1, 1)]));
        store
            .append_step_result("run_CLAIM", &step_result("a", &a))
            .unwrap();
        let idx = UsageIndex::default();

        let first = idx.claimed_paths(&store, "run_CLAIM");
        assert!(is_sealed(&idx, &store, "run_CLAIM"));
        assert!(first.contains(&a));
        assert!(first.contains(&std::fs::canonicalize(&a).unwrap()));
        assert!(
            Arc::ptr_eq(&first, &idx.claimed_paths(&store, "run_CLAIM")),
            "a sealed run's claim is not recomputed"
        );

        // A late step result breaks the seal; its transcript is claimed.
        let b = tmp.path().join("transcripts/run_B.jsonl");
        append(&b, &transcript_lines("b", PROVIDER, MODEL, &[(1, 1)]));
        store
            .append_step_result("run_CLAIM", &step_result("b", &b))
            .unwrap();
        let after = idx.claimed_paths(&store, "run_CLAIM");
        assert!(after.contains(&a) && after.contains(&b));

        // A live run is never memoized: what it claims is re-resolved.
        create_run(&store, "run_LIVE", RunStatus::Running);
        let live = idx.claimed_paths(&store, "run_LIVE");
        assert!(!Arc::ptr_eq(&live, &idx.claimed_paths(&store, "run_LIVE")));
    }

    #[test]
    fn run_metrics_prefers_the_finished_records_wall_clock() {
        let tmp = tempfile::tempdir().unwrap();
        let store = run_store(tmp.path());
        let f = tmp.path().join("transcripts/run_F.jsonl");
        let complete = event_json(&Event::RunComplete {
            run_id: "run_F".into(),
            status: rupu_transcript::RunStatus::Ok,
            total_tokens: 2,
            duration_ms: 4200,
            error: None,
            outcome: None,
        });
        append(
            &f,
            &(transcript_lines("a", PROVIDER, MODEL, &[(1, 1)]) + &complete),
        );
        let pricing = rupu_config::PricingConfig::default();

        // Unfinished record: the transcript's RunComplete duration.
        create_run(&store, "run_WALLA", RunStatus::Running);
        store
            .append_step_result("run_WALLA", &step_result("s", &f))
            .unwrap();
        let m = crate::usage::run_metrics(&store, "run_WALLA", &pricing);
        assert_eq!(m.duration_ms, Some(4200));
        assert_eq!(m.turns, 1);

        // Finished record: its wall clock, stable across sealed re-reads.
        let mut rec = record("run_WALLB", RunStatus::Completed, None);
        rec.finished_at = Some(rec.started_at + chrono::Duration::seconds(90));
        store.create(rec, "").unwrap();
        store
            .append_step_result("run_WALLB", &step_result("s", &f))
            .unwrap();
        for _ in 0..2 {
            let m = crate::usage::run_metrics(&store, "run_WALLB", &pricing);
            assert_eq!(m.duration_ms, Some(90_000));
            assert_eq!(m.usage.total_tokens, 2);
        }
    }

    #[test]
    fn transcripts_usage_folds_labeled_paths_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("run_A.jsonl");
        let b = tmp.path().join("run_B.jsonl");
        append(
            &a,
            &transcript_lines("one", PROVIDER, MODEL, &[(1, 1), (2, 2)]),
        );
        append(&b, &transcript_lines("two", PROVIDER, MODEL, &[(3, 3)]));
        let idx = UsageIndex::default();

        let u = idx.transcripts_usage(&[
            ("x".into(), a.clone()),
            ("y".into(), b.clone()),
            ("x".into(), a.clone()),
        ]);
        assert_eq!(total(&u), tok(6, 6), "a path listed twice counts once");
        assert_eq!(u.turns, 3);
        let labels: Vec<&str> = u.points.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["x", "x", "y"]);
        assert!(!u.partial);

        let missing = idx.transcripts_usage(&[("z".into(), tmp.path().join("run_Z.jsonl"))]);
        assert!(missing.partial);
        assert_eq!(total(&missing), Tokens::default());
    }
}
