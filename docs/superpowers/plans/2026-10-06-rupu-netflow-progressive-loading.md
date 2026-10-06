# Netflow Progressive Loading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing Network pages (global, project, run) load fast on large histories without changing anything they render.

**Architecture:** A two-tier, stat-validated, incrementally-tailed `NetflowIndex` in `rupu-cp` replaces every per-file ledger read behind a `LedgerReader` trait (callers keep their listing and iteration order, so responses are byte-identical). The global/project explorer builds its whole-history views from the always-resident tier-1 summaries and reads rows only for files overlapping the window. On the web, `NetflowExplorer` loads its two requests independently, global/project default to the existing `24h` preset, and `SortableTable` gains opt-in window virtualization used only by `NetflowTable`.

**Tech Stack:** Rust 2021 (tokio, axum, serde, chrono), React + TypeScript + vitest/jsdom.

**Spec:** `docs/superpowers/specs/2026-10-06-rupu-netflow-progressive-loading-design.md`

## Global Constraints

- What is rendered stays the same: no new UI elements, states, copy, controls, paging affordances, or changed control semantics (operator rule, 2026-10-06).
- Same endpoints, same response shapes, same values; the index must not change any number on screen (spec §3.6).
- Config key: `[netflow].cp_index_budget_mb`, default `256`.
- Status endpoint: `GET /api/netflow/index` → `{ files, flows, tier1_bytes, tier2_bytes, budget_bytes, resident_files, evictions_total }`.
- Virtualization threshold: `500` rows, `NetflowTable` only.
- Workspace deps only — no new crates, no new npm packages.
- Integration tests: ONE binary per crate (`crates/<c>/tests/it/`); never a new top-level `tests/*.rs`.
- Never run package-wide `cargo fmt`; format only the files you touched (`rustfmt --edition 2021 <file>`).
- Run targeted tests only (`cargo test -p <crate> <filter>`), never a cold full-workspace `cargo test`.
- Never use bare `git stash` / `git stash pop` (shared stash across worktrees).
- `cp serve` is a daemon: live verification needs a restart of the installed binary.

## Spec refinements made while planning (already folded into the spec)

- The index is a per-file drop-in (`LedgerReader::flows_in_range`), not a global walk: callers keep listing files as today, which preserves iteration order and makes old-vs-new equivalence exact.
- Change detection uses a stored file prefix instead of `FileCache`'s settle window (ledgers are append-only).
- An unterminated final line is not read until its newline arrives (documented deviation).
- Row heights are measured with ref callbacks on every render rather than a `ResizeObserver` (same effect for single-line rows; simpler).

## File structure

| File | Responsibility |
|---|---|
| `crates/rupu-netflow/src/ledger/fold.rs` (new) | `LedgerFold` / `FoldEvent` / `FlowPatch` / `split_complete_lines` — the one place ledger lines become flows |
| `crates/rupu-netflow/src/ledger/compact.rs` (new) | `CompactRows` — per-file interned flow rows |
| `crates/rupu-netflow/src/ledger/views.rs` | readers rewired onto `LedgerFold` |
| `crates/rupu-netflow/src/ledger/explorer.rs` | `HistPoint`, `histogram_from_points`, `SankeyUniverse`, `sankey_view_with_universe`, `is_error_outcome` |
| `crates/rupu-netflow/src/ledger/mod.rs` | re-exports |
| `crates/rupu-config/src/netflow_config.rs` | `cp_index_budget_mb` |
| `crates/rupu-cp/src/netflow_index.rs` (new) | `LedgerReader`, `DirectReader`, `NetflowIndex`, `LedgerSummary`, `IndexStatus` |
| `crates/rupu-cp/src/api/netflow.rs` | file listing split from reading; all reads via `LedgerReader`; `indexed_explorer`; status route; prewarm helper |
| `crates/rupu-cp/src/state.rs`, `lib.rs` | `AppState.netflow_index`; prewarm |
| `crates/rupu-cli/src/cmd/netflow.rs` | passes `DirectReader` |
| `crates/rupu-cp/web/src/components/netflow/TimeRangePicker.tsx` | exports `presetValue` |
| `crates/rupu-cp/web/src/components/netflow/explorer/NetflowExplorer.tsx` | independent section loading, 24h default |
| `crates/rupu-cp/web/src/components/lists/useWindowVirtualRows.ts` (new) | viewport math + row measurement |
| `crates/rupu-cp/web/src/components/lists/SortableTable.tsx` | opt-in `virtualize` |
| `crates/rupu-cp/web/src/components/netflow/NetflowTable.tsx` | passes `virtualize={{ threshold: 500 }}` |
| `docs/configuration.md`, `CLAUDE.md` | docs |

---

### Task 1: `LedgerFold` — one resumable fold for every ledger reader

**Files:**
- Create: `crates/rupu-netflow/src/ledger/fold.rs`
- Modify: `crates/rupu-netflow/src/ledger/views.rs` (`read_flows_and_dropped`, `read_capture_states`)
- Modify: `crates/rupu-netflow/src/ledger/mod.rs`

**Interfaces:**
- Produces:
  - `pub struct FlowPatch { pub bytes_in: Option<u64>, pub bytes_out: Option<u64>, pub outcome: Option<Outcome>, pub error: Option<String>, pub duration_ms: u64 }` with `pub fn apply(&self, f: &mut FlowRecord)`
  - `pub enum FoldEvent { Flow(Box<FlowRecord>), Patch { index: usize, patch: FlowPatch }, Dropped(u64), Capture(CaptureEntry) }`
  - `pub struct LedgerFold` (`Default`, `Clone`) with `pub fn feed_line(&mut self, line: &str) -> Option<FoldEvent>`, `pub fn flow_count(&self) -> usize`, `pub fn heap_bytes(&self) -> usize`
  - `pub fn split_complete_lines(buf: &[u8]) -> (&[u8], usize)` — the prefix up to and including the last `\n`, and its length
  - re-exported from `rupu_netflow::ledger`: `FlowPatch, FoldEvent, LedgerFold, split_complete_lines`

- [ ] **Step 1: Write the failing tests** — append to `fold.rs` (create the file with only the test module and `use` lines first so it compiles to a failure):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::views::read_flows_and_dropped;
    use crate::record::{Fidelity, FlowRecord, LedgerLine, Outcome, SocketCompletion};
    use crate::{FlowCtx, Origin};

    fn flow(n: u64, secs: i64) -> FlowRecord {
        FlowRecord {
            id: crate::FlowId::from_parts(n, n),
            ts: chrono::DateTime::from_timestamp(secs, 0).unwrap(),
            ctx: FlowCtx::system(Origin::Provider("anthropic".into())),
            fidelity: Fidelity::Socket,
            method: "POST".into(),
            scheme: "https".into(),
            host: "api.anthropic.com".into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: None,
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

    /// A ledger exercising every line kind, including completions that
    /// rewrite an earlier flow's outcome.
    fn ledger_text() -> String {
        let lines = vec![
            LedgerLine::Flow(Box::new(flow(1, 100))),
            LedgerLine::Flow(Box::new(flow(2, 200))),
            LedgerLine::Complete { id: crate::FlowId::from_parts(1, 1), bytes_in: 7, duration_ms: 9 },
            LedgerLine::Dropped { count: 3, ts: chrono::DateTime::from_timestamp(250, 0).unwrap() },
            LedgerLine::SocketComplete(SocketCompletion {
                id: crate::FlowId::from_parts(2, 2),
                duration_ms: 11,
                bytes_in: Some(5),
                bytes_out: Some(6),
                outcome: Some(Outcome::TransportError),
                error: Some("reset".into()),
            }),
            LedgerLine::Capture {
                ts: chrono::DateTime::from_timestamp(300, 0).unwrap(),
                state: crate::record::CaptureState::Active { backend: "ntstat".into() },
                tool_call_id: None,
                note: None,
            },
        ];
        let mut s = String::new();
        for l in lines {
            s.push_str(&serde_json::to_string(&l).unwrap());
            s.push('\n');
        }
        s.push_str("{not json}\n\n");
        s
    }

    /// Fold `text` fed in pieces split at `cuts` (byte offsets), the way
    /// the CP index tails a growing file.
    fn fold_chunked(text: &str, cuts: &[usize]) -> (Vec<FlowRecord>, u64, usize) {
        let bytes = text.as_bytes();
        let mut fold = LedgerFold::default();
        let mut flows: Vec<FlowRecord> = Vec::new();
        let mut dropped = 0u64;
        let mut captures = 0usize;
        let mut offset = 0usize;
        let mut ends: Vec<usize> = cuts.to_vec();
        ends.push(bytes.len());
        for end in ends {
            let (complete, used) = split_complete_lines(&bytes[offset..end]);
            for line in std::str::from_utf8(complete).unwrap().lines() {
                match fold.feed_line(line) {
                    Some(FoldEvent::Flow(f)) => flows.push(*f),
                    Some(FoldEvent::Patch { index, patch }) => patch.apply(&mut flows[index]),
                    Some(FoldEvent::Dropped(n)) => dropped += n,
                    Some(FoldEvent::Capture(_)) => captures += 1,
                    None => {}
                }
            }
            offset += used;
        }
        (flows, dropped, captures)
    }

    #[test]
    fn chunked_fold_matches_the_whole_file_reader_at_every_split_point() {
        let text = ledger_text();
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("run_x.jsonl");
        std::fs::write(&path, &text).unwrap();
        let (whole_flows, whole_dropped) = read_flows_and_dropped(&path).unwrap();
        for cut in 0..=text.len() {
            let (flows, dropped, captures) = fold_chunked(&text, &[cut]);
            assert_eq!(flows, whole_flows, "split at byte {cut}");
            assert_eq!(dropped, whole_dropped, "split at byte {cut}");
            assert_eq!(captures, 1, "split at byte {cut}");
        }
    }

    #[test]
    fn a_socket_completion_rewrites_the_earlier_flows_outcome() {
        let (flows, dropped, _) = fold_chunked(&ledger_text(), &[]);
        assert_eq!(dropped, 3);
        assert_eq!(flows[0].bytes_in, Some(7));
        assert!(flows[0].body_complete);
        assert_eq!(flows[1].outcome, Outcome::TransportError);
        assert_eq!(flows[1].error.as_deref(), Some("reset"));
        assert_eq!(flows[1].bytes_out, Some(6));
        assert_eq!(flows[1].duration_ms, Some(11));
    }

    #[test]
    fn split_complete_lines_holds_back_an_unterminated_tail() {
        assert_eq!(split_complete_lines(b"a\nb\nc"), (&b"a\nb\n"[..], 4));
        assert_eq!(split_complete_lines(b"abc"), (&b""[..], 0));
        assert_eq!(split_complete_lines(b""), (&b""[..], 0));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-netflow --lib ledger::fold`
Expected: FAIL to compile — `LedgerFold`, `FoldEvent`, `split_complete_lines` not defined.

- [ ] **Step 3: Implement `fold.rs`** (above the test module):

```rust
//! The resumable ledger fold — the ONE place ledger lines become flows.
//!
//! Every reader feeds lines through [`LedgerFold`]: the whole-file readers
//! in [`super::views`] and the CP's netflow index, which tails a growing
//! ledger in appended chunks. Sharing the fold is what guarantees a file
//! read in pieces folds to exactly what a file read in one pass does.

use super::views::CaptureEntry;
use crate::record::{FlowId, FlowRecord, LedgerLine, Outcome};
use std::collections::HashMap;

/// The fields a `complete` / `socket_complete` line rewrites on an earlier
/// flow. Applying a patch always marks the flow's body complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowPatch {
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
    pub outcome: Option<Outcome>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

impl FlowPatch {
    pub fn apply(&self, f: &mut FlowRecord) {
        if let Some(b) = self.bytes_in {
            f.bytes_in = Some(b);
        }
        if let Some(b) = self.bytes_out {
            f.bytes_out = Some(b);
        }
        if let Some(o) = self.outcome {
            f.outcome = o;
        }
        if let Some(e) = &self.error {
            f.error = Some(e.clone());
        }
        f.duration_ms = Some(self.duration_ms);
        f.body_complete = true;
    }
}

/// What one ledger line contributes. `Patch::index` is the position of the
/// patched flow among this fold's `Flow` events, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum FoldEvent {
    Flow(Box<FlowRecord>),
    Patch { index: usize, patch: FlowPatch },
    Dropped(u64),
    Capture(CaptureEntry),
}

/// Fold state: which flow id sits at which position, so a later completion
/// line can find the flow it finishes. Blank and malformed lines, and
/// completions for an id this fold never saw, contribute nothing.
#[derive(Debug, Clone, Default)]
pub struct LedgerFold {
    index: HashMap<FlowId, usize>,
    len: usize,
}

impl LedgerFold {
    pub fn feed_line(&mut self, line: &str) -> Option<FoldEvent> {
        if line.trim().is_empty() {
            return None;
        }
        match serde_json::from_str::<LedgerLine>(line).ok()? {
            LedgerLine::Flow(f) => {
                self.index.insert(f.id, self.len);
                self.len += 1;
                Some(FoldEvent::Flow(f))
            }
            LedgerLine::Complete {
                id,
                bytes_in,
                duration_ms,
            } => Some(FoldEvent::Patch {
                index: *self.index.get(&id)?,
                patch: FlowPatch {
                    bytes_in: Some(bytes_in),
                    bytes_out: None,
                    outcome: None,
                    error: None,
                    duration_ms,
                },
            }),
            LedgerLine::SocketComplete(c) => Some(FoldEvent::Patch {
                index: *self.index.get(&c.id)?,
                patch: FlowPatch {
                    bytes_in: c.bytes_in,
                    bytes_out: c.bytes_out,
                    outcome: c.outcome,
                    error: c.error,
                    duration_ms: c.duration_ms,
                },
            }),
            LedgerLine::Dropped { count, .. } => Some(FoldEvent::Dropped(count)),
            LedgerLine::Capture {
                ts,
                state,
                tool_call_id,
                note,
            } => Some(FoldEvent::Capture(CaptureEntry {
                ts,
                state,
                tool_call_id,
                note,
            })),
        }
    }

    /// Flows seen so far.
    pub fn flow_count(&self) -> usize {
        self.len
    }

    /// Approximate heap held by the id map (for the CP index's budget).
    pub fn heap_bytes(&self) -> usize {
        self.index.capacity() * (std::mem::size_of::<FlowId>() + std::mem::size_of::<usize>() + 8)
    }
}

/// Split `buf` after its last `\n`: the complete-lines prefix and its byte
/// length. An unterminated tail is left for the next read.
pub fn split_complete_lines(buf: &[u8]) -> (&[u8], usize) {
    match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => (&buf[..=i], i + 1),
        None => (&buf[..0], 0),
    }
}
```

- [ ] **Step 4: Rewire `views.rs` onto the fold.** Replace the body of `read_flows_and_dropped` after the `File::open` match with:

```rust
    let mut fold = super::fold::LedgerFold::default();
    let mut flows: Vec<FlowRecord> = Vec::new();
    let mut dropped = 0u64;
    for line in BufReader::new(file).lines() {
        let line = line?;
        match fold.feed_line(&line) {
            Some(super::fold::FoldEvent::Flow(f)) => flows.push(*f),
            Some(super::fold::FoldEvent::Patch { index, patch }) => patch.apply(&mut flows[index]),
            Some(super::fold::FoldEvent::Dropped(n)) => dropped += n,
            Some(super::fold::FoldEvent::Capture(_)) | None => {}
        }
    }
    Ok((flows, dropped))
```

In `read_capture_states`, keep the `"capture"` pre-filter and replace the `if let Ok(LedgerLine::Capture { .. })` block with:

```rust
        if let Some(super::fold::FoldEvent::Capture(entry)) = fold.feed_line(&line) {
            out.push(entry);
        }
```

with `let mut fold = super::fold::LedgerFold::default();` declared before the loop. Remove now-unused imports (`FlowId`, `HashMap`, `LedgerLine`, `Outcome` if the compiler flags them). Keep every doc comment.

In `ledger/mod.rs` add `pub mod fold;` and `pub use fold::{split_complete_lines, FlowPatch, FoldEvent, LedgerFold};`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-netflow --lib ledger::` then `cargo test -p rupu-netflow --test it`
Expected: PASS (the new fold tests plus every existing `views` and `record_model` test unchanged).

- [ ] **Step 6: Clippy + format touched files, commit**

```bash
cargo clippy -p rupu-netflow --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-netflow/src/ledger/fold.rs crates/rupu-netflow/src/ledger/views.rs crates/rupu-netflow/src/ledger/mod.rs
git add crates/rupu-netflow/src/ledger/fold.rs crates/rupu-netflow/src/ledger/views.rs crates/rupu-netflow/src/ledger/mod.rs
git commit -m "refactor(netflow): one resumable LedgerFold behind every ledger reader"
```

---

### Task 2: `CompactRows` — interned per-file flow rows

**Files:**
- Create: `crates/rupu-netflow/src/ledger/compact.rs`
- Modify: `crates/rupu-netflow/src/ledger/mod.rs`

**Interfaces:**
- Consumes: `FlowPatch::apply` (Task 1).
- Produces: `pub struct CompactRows` (`Default`, `Clone`, `Debug`) with
  `pub fn push(&mut self, f: FlowRecord)`, `pub fn patch(&mut self, index: usize, p: &FlowPatch)`,
  `pub fn record(&self, index: usize) -> FlowRecord`, `pub fn len(&self) -> usize`, `pub fn is_empty(&self) -> bool`,
  `pub fn records_in_range<'a>(&'a self, range: &'a TimeRange) -> impl Iterator<Item = FlowRecord> + 'a`,
  `pub fn heap_bytes(&self) -> usize`; re-exported as `rupu_netflow::ledger::CompactRows`.

- [ ] **Step 1: Write the failing tests** (in `compact.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Direction, Fidelity, FlowProcess, Outcome};
    use crate::ledger::views::TimeRange;

    fn full(n: u64, origin: Origin) -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(n, n),
            ts: chrono::DateTime::from_timestamp(1_700_000_000 + n as i64, 5).unwrap(),
            ctx: FlowCtx {
                run_id: Some("run-1".into()),
                step_id: Some("step-1".into()),
                agent: Some("reviewer".into()),
                workspace_id: Some("ws-1".into()),
                tool_call_id: Some("tc-1".into()),
                origin,
            },
            fidelity: Fidelity::Full,
            method: "POST".into(),
            scheme: "https".into(),
            host: "api.anthropic.com".into(),
            port: 443,
            path: "/v1/messages?x=1".into(),
            peer_ip: Some("160.79.104.10".parse().unwrap()),
            resolved_ips: vec!["160.79.104.10".parse().unwrap(), "::1".parse().unwrap()],
            process: Some(FlowProcess { pid: 42, name: "curl".into() }),
            local_addr: Some("10.0.0.2:51515".into()),
            direction: Some(Direction::Outbound),
            http_version: Some("HTTP/2".into()),
            status: Some(529),
            outcome: Outcome::HttpError,
            error: Some("overloaded".into()),
            bytes_out: Some(100),
            bytes_in: Some(200),
            body_complete: true,
            ttfb_ms: Some(30),
            duration_ms: Some(40),
        }
    }

    fn origins() -> Vec<Origin> {
        vec![
            Origin::Provider("anthropic".into()),
            Origin::Scm("github".into()),
            Origin::Subprocess("bash".into()),
            Origin::Update,
            Origin::Cp,
            Origin::System,
        ]
    }

    #[test]
    fn every_field_round_trips_for_every_origin() {
        let mut rows = CompactRows::default();
        let input: Vec<FlowRecord> = origins().into_iter().enumerate().map(|(i, o)| full(i as u64, o)).collect();
        for f in &input {
            rows.push(f.clone());
        }
        let back: Vec<FlowRecord> = (0..rows.len()).map(|i| rows.record(i)).collect();
        assert_eq!(back, input);
    }

    #[test]
    fn empty_optionals_round_trip_as_none() {
        let mut f = full(9, Origin::System);
        f.ctx.run_id = None;
        f.ctx.tool_call_id = None;
        f.peer_ip = None;
        f.resolved_ips = vec![];
        f.process = None;
        f.local_addr = None;
        f.http_version = None;
        f.error = None;
        let mut rows = CompactRows::default();
        rows.push(f.clone());
        assert_eq!(rows.record(0), f);
    }

    #[test]
    fn a_patch_matches_flow_patch_apply() {
        let mut f = full(1, Origin::System);
        f.outcome = Outcome::Ok;
        f.error = None;
        let patch = crate::ledger::fold::FlowPatch {
            bytes_in: Some(9),
            bytes_out: None,
            outcome: Some(Outcome::Timeout),
            error: Some("slow".into()),
            duration_ms: 77,
        };
        let mut rows = CompactRows::default();
        rows.push(f.clone());
        rows.patch(0, &patch);
        patch.apply(&mut f);
        assert_eq!(rows.record(0), f);
    }

    #[test]
    fn records_in_range_filters_inclusively_and_keeps_file_order() {
        let mut rows = CompactRows::default();
        for n in 0..5 {
            rows.push(full(n, Origin::System));
        }
        let lo = rows.record(1).ts;
        let hi = rows.record(3).ts;
        let range = TimeRange { from: Some(lo), to: Some(hi) };
        let ids: Vec<FlowId> = rows.records_in_range(&range).map(|f| f.id).collect();
        assert_eq!(ids, vec![FlowId::from_parts(1, 1), FlowId::from_parts(2, 2), FlowId::from_parts(3, 3)]);
    }

    #[test]
    fn repeated_strings_are_stored_once() {
        let mut rows = CompactRows::default();
        rows.push(full(1, Origin::System));
        let one = rows.heap_bytes();
        for n in 2..1000 {
            rows.push(full(n, Origin::System));
        }
        let per_row = (rows.heap_bytes() - one) / 998;
        assert!(per_row < 400, "per-row bytes {per_row} — strings are not being interned");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-netflow --lib ledger::compact`
Expected: FAIL to compile — `CompactRows` not defined.

- [ ] **Step 3: Implement `compact.rs`** (above the tests):

```rust
//! Compact in-memory flow rows for the CP's netflow index.
//!
//! A ledger repeats the same few hosts, paths, methods and context strings
//! across thousands of flows, so each [`CompactRows`] keeps its own string
//! table and stores rows as symbols. The table is per file: dropping a
//! file's rows frees all of its memory. Conversion is exhaustive — the
//! destructuring below names every field with no `..`, so adding a field to
//! [`FlowRecord`], [`FlowCtx`] or [`FlowProcess`] fails to compile here
//! until the compact form carries it.

use super::fold::FlowPatch;
use super::views::TimeRange;
use crate::ctx::{FlowCtx, Origin};
use crate::record::{Direction, Fidelity, FlowId, FlowProcess, FlowRecord, Outcome};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

type Sym = u32;
const NONE: Sym = u32::MAX;

#[derive(Debug, Clone, Default)]
struct Interner {
    map: HashMap<Arc<str>, Sym>,
    strs: Vec<Arc<str>>,
    text_bytes: usize,
}

impl Interner {
    fn intern(&mut self, s: &str) -> Sym {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let a: Arc<str> = Arc::from(s);
        let i = self.strs.len() as Sym;
        self.strs.push(Arc::clone(&a));
        self.map.insert(a, i);
        self.text_bytes += s.len();
        i
    }

    fn opt(&mut self, s: Option<&str>) -> Sym {
        s.map_or(NONE, |s| self.intern(s))
    }

    fn get(&self, i: Sym) -> String {
        self.strs[i as usize].to_string()
    }

    fn get_opt(&self, i: Sym) -> Option<String> {
        (i != NONE).then(|| self.get(i))
    }

    fn heap_bytes(&self) -> usize {
        // Text once, plus per-entry Arc headers, the Vec slot and the map slot.
        self.text_bytes + self.strs.capacity() * 48
    }
}

#[derive(Debug, Clone, Copy)]
enum COrigin {
    Provider(Sym),
    Scm(Sym),
    Subprocess(Sym),
    Update,
    Cp,
    System,
}

#[derive(Debug, Clone)]
struct CompactFlow {
    id: FlowId,
    ts: DateTime<Utc>,
    run_id: Sym,
    step_id: Sym,
    agent: Sym,
    workspace_id: Sym,
    tool_call_id: Sym,
    origin: COrigin,
    fidelity: Fidelity,
    method: Sym,
    scheme: Sym,
    host: Sym,
    port: u16,
    path: Sym,
    peer_ip: Option<IpAddr>,
    resolved_ips: Box<[IpAddr]>,
    process: Option<(u32, Sym)>,
    local_addr: Sym,
    direction: Option<Direction>,
    http_version: Sym,
    status: Option<u16>,
    outcome: Outcome,
    error: Sym,
    bytes_out: Option<u64>,
    bytes_in: Option<u64>,
    body_complete: bool,
    ttfb_ms: Option<u64>,
    duration_ms: Option<u64>,
}

/// One ledger file's flows, in file order, with a per-file string table.
#[derive(Debug, Clone, Default)]
pub struct CompactRows {
    strings: Interner,
    rows: Vec<CompactFlow>,
    ip_bytes: usize,
}

impl CompactRows {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn push(&mut self, f: FlowRecord) {
        let row = self.compact(f);
        self.rows.push(row);
    }

    /// Apply a completion to row `index` through [`FlowPatch::apply`] itself,
    /// so the compact path can never drift from the whole-file reader.
    pub fn patch(&mut self, index: usize, p: &FlowPatch) {
        let mut f = self.record(index);
        p.apply(&mut f);
        self.ip_bytes -= self.rows[index].resolved_ips.len() * std::mem::size_of::<IpAddr>();
        self.rows[index] = self.compact(f);
    }

    pub fn record(&self, index: usize) -> FlowRecord {
        let r = &self.rows[index];
        let s = &self.strings;
        FlowRecord {
            id: r.id,
            ts: r.ts,
            ctx: FlowCtx {
                run_id: s.get_opt(r.run_id),
                step_id: s.get_opt(r.step_id),
                agent: s.get_opt(r.agent),
                workspace_id: s.get_opt(r.workspace_id),
                tool_call_id: s.get_opt(r.tool_call_id),
                origin: match r.origin {
                    COrigin::Provider(x) => Origin::Provider(s.get(x)),
                    COrigin::Scm(x) => Origin::Scm(s.get(x)),
                    COrigin::Subprocess(x) => Origin::Subprocess(s.get(x)),
                    COrigin::Update => Origin::Update,
                    COrigin::Cp => Origin::Cp,
                    COrigin::System => Origin::System,
                },
            },
            fidelity: r.fidelity,
            method: s.get(r.method),
            scheme: s.get(r.scheme),
            host: s.get(r.host),
            port: r.port,
            path: s.get(r.path),
            peer_ip: r.peer_ip,
            resolved_ips: r.resolved_ips.to_vec(),
            process: r.process.map(|(pid, name)| FlowProcess { pid, name: s.get(name) }),
            local_addr: s.get_opt(r.local_addr),
            direction: r.direction,
            http_version: s.get_opt(r.http_version),
            status: r.status,
            outcome: r.outcome,
            error: s.get_opt(r.error),
            bytes_out: r.bytes_out,
            bytes_in: r.bytes_in,
            body_complete: r.body_complete,
            ttfb_ms: r.ttfb_ms,
            duration_ms: r.duration_ms,
        }
    }

    /// Rows whose `ts` falls in `range` (same inclusive semantics as
    /// [`TimeRange::contains`]), materialized, in file order.
    pub fn records_in_range<'a>(
        &'a self,
        range: &'a TimeRange,
    ) -> impl Iterator<Item = FlowRecord> + 'a {
        (0..self.rows.len())
            .filter(move |&i| range.contains(self.rows[i].ts))
            .map(move |i| self.record(i))
    }

    /// Approximate heap held (rows + resolved-IP lists + string table).
    pub fn heap_bytes(&self) -> usize {
        self.rows.capacity() * std::mem::size_of::<CompactFlow>()
            + self.ip_bytes
            + self.strings.heap_bytes()
    }

    fn compact(&mut self, f: FlowRecord) -> CompactFlow {
        let FlowRecord {
            id,
            ts,
            ctx,
            fidelity,
            method,
            scheme,
            host,
            port,
            path,
            peer_ip,
            resolved_ips,
            process,
            local_addr,
            direction,
            http_version,
            status,
            outcome,
            error,
            bytes_out,
            bytes_in,
            body_complete,
            ttfb_ms,
            duration_ms,
        } = f;
        let FlowCtx {
            run_id,
            step_id,
            agent,
            workspace_id,
            tool_call_id,
            origin,
        } = ctx;
        let st = &mut self.strings;
        let origin = match origin {
            Origin::Provider(x) => COrigin::Provider(st.intern(&x)),
            Origin::Scm(x) => COrigin::Scm(st.intern(&x)),
            Origin::Subprocess(x) => COrigin::Subprocess(st.intern(&x)),
            Origin::Update => COrigin::Update,
            Origin::Cp => COrigin::Cp,
            Origin::System => COrigin::System,
        };
        self.ip_bytes += resolved_ips.len() * std::mem::size_of::<IpAddr>();
        CompactFlow {
            id,
            ts,
            run_id: st.opt(run_id.as_deref()),
            step_id: st.opt(step_id.as_deref()),
            agent: st.opt(agent.as_deref()),
            workspace_id: st.opt(workspace_id.as_deref()),
            tool_call_id: st.opt(tool_call_id.as_deref()),
            origin,
            fidelity,
            method: st.intern(&method),
            scheme: st.intern(&scheme),
            host: st.intern(&host),
            port,
            path: st.intern(&path),
            peer_ip,
            resolved_ips: resolved_ips.into_boxed_slice(),
            process: process.map(|FlowProcess { pid, name }| (pid, st.intern(&name))),
            local_addr: st.opt(local_addr.as_deref()),
            direction,
            http_version: st.opt(http_version.as_deref()),
            status,
            outcome,
            error: st.opt(error.as_deref()),
            bytes_out,
            bytes_in,
            body_complete,
            ttfb_ms,
            duration_ms,
        }
    }
}
```

If `Origin` has variants beyond the six above, the exhaustive matches fail to compile — add a `COrigin` variant for each and a case to `origins()` in the test.

In `ledger/mod.rs`: `pub mod compact;` and `pub use compact::CompactRows;`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-netflow --lib ledger::compact`
Expected: PASS (5 tests).

- [ ] **Step 5: Clippy, format, commit**

```bash
cargo clippy -p rupu-netflow --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-netflow/src/ledger/compact.rs crates/rupu-netflow/src/ledger/mod.rs
git add crates/rupu-netflow/src/ledger/compact.rs crates/rupu-netflow/src/ledger/mod.rs
git commit -m "feat(netflow): CompactRows — interned per-file flow rows for the CP index"
```

---

### Task 3: Explorer entry points that take whole-history inputs separately

**Files:**
- Modify: `crates/rupu-netflow/src/ledger/explorer.rs`

**Interfaces:**
- Produces (all `pub` in `rupu_netflow::ledger::explorer`):
  - `pub type HistPoint = (DateTime<Utc>, bool);` — a flow's timestamp and whether it errored
  - `pub fn is_error_outcome(o: Outcome) -> bool`
  - `pub fn histogram_from_points<I>(points: I, n: usize) -> HistogramView where I: IntoIterator<Item = HistPoint>, I::IntoIter: Clone`
  - `pub struct SankeyUniverse { pub workflows: BTreeMap<String, String>, pub origins: BTreeMap<String, String>, pub orgs: BTreeMap<String, String> }` (`Default`, `Clone`, `Debug`, `PartialEq`) with `add_workflow(&mut self, Option<&str>)`, `add_origin_key(&mut self, String)`, `add_org(&mut self, Option<&AsnInfo>)`, `from_flows(&[ExplorerFlow]) -> Self`
  - `pub fn sankey_view_with_universe(universe: &SankeyUniverse, flows: &[ExplorerFlow], window: &TimeRange, filters: &ExplorerFilters) -> SankeyView` — `flows` must contain at least every in-scope flow inside `window`
  - existing `histogram_view` and `sankey_view` keep their signatures and delegate.

- [ ] **Step 1: Write the failing tests** — add to the existing `#[cfg(test)] mod tests` in `explorer.rs` (reuse that module's existing flow-builder helpers; if it has none that sets `peer_ip`/`asn`/`workflow`, add this one):

```rust
    fn xf(secs: i64, wf: Option<&str>, origin: crate::Origin, asn: Option<(u32, &str)>, host: &str, ok: bool) -> ExplorerFlow {
        let mut flow = crate::ledger::fold::tests_support::bare_flow(secs);
        flow.ctx.origin = origin;
        flow.host = host.into();
        flow.outcome = if ok { Outcome::Ok } else { Outcome::HttpError };
        ExplorerFlow {
            run_id: wf.map(|_| "run-1".to_string()),
            workflow: wf.map(str::to_string),
            asn: asn.map(|(n, org)| AsnInfo { asn: n, org: org.into() }),
            flow,
        }
    }

    fn scope() -> Vec<ExplorerFlow> {
        vec![
            xf(100, Some("review"), crate::Origin::Provider("anthropic".into()), Some((1, "One")), "a.com", true),
            xf(200, None, crate::Origin::Scm("github".into()), None, "b.com", false),
            xf(300, Some("triage"), crate::Origin::System, Some((2, "Two")), "a.com", true),
            xf(400, Some("review"), crate::Origin::Provider("anthropic".into()), Some((1, "One")), "c.com", false),
        ]
    }

    #[test]
    fn histogram_from_points_matches_histogram_view() {
        let flows = scope();
        let points: Vec<HistPoint> = flows.iter().map(|f| (f.flow.ts, is_error_outcome(f.flow.outcome))).collect();
        assert_eq!(histogram_from_points(points.iter().copied(), EXPLORER_BUCKETS), histogram_view(&flows, EXPLORER_BUCKETS));
        assert_eq!(histogram_from_points(std::iter::empty::<HistPoint>(), 8), histogram_view(&[], 8));
    }

    #[test]
    fn sankey_with_a_separate_universe_matches_the_whole_scope_view() {
        let flows = scope();
        let window = TimeRange {
            from: Some(chrono::DateTime::from_timestamp(250, 0).unwrap()),
            to: None,
        };
        let filters = ExplorerFilters { origins: vec!["provider:anthropic".into()], ..Default::default() };
        let windowed: Vec<ExplorerFlow> = flows.iter().filter(|f| window.contains(f.flow.ts)).cloned().collect();
        let universe = SankeyUniverse::from_flows(&flows);
        assert_eq!(
            sankey_view_with_universe(&universe, &windowed, &window, &filters),
            sankey_view(&flows, &window, &filters),
        );
    }

    #[test]
    fn universe_built_key_by_key_matches_from_flows() {
        let flows = scope();
        let mut u = SankeyUniverse::default();
        for f in &flows {
            u.add_workflow(f.workflow.as_deref());
            u.add_origin_key(origin_key(&f.flow.ctx.origin));
            u.add_org(f.asn.as_ref());
        }
        assert_eq!(u, SankeyUniverse::from_flows(&flows));
        assert_eq!(u.workflows.get(UNKNOWN_KEY).map(String::as_str), Some(UNKNOWN_WORKFLOW_LABEL));
        assert_eq!(u.orgs.get("as1").map(String::as_str), Some("One"));
    }
```

For `bare_flow`, add a small `#[cfg(test)] pub(crate) mod tests_support { pub fn bare_flow(secs: i64) -> FlowRecord { … } }` to `fold.rs` containing the `flow(n, secs)` builder body from Task 1's tests (with `id: FlowId::new()`), and make Task 1's test `flow` call it. If `explorer.rs`'s test module already has an equivalent builder, use that instead and skip `tests_support`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-netflow --lib ledger::explorer`
Expected: FAIL to compile — `HistPoint`, `histogram_from_points`, `SankeyUniverse`, `sankey_view_with_universe`, `is_error_outcome` not defined.

- [ ] **Step 3: Implement.** In `explorer.rs`:

1. Add next to `UNKNOWN_KEY`:

```rust
/// One flow's contribution to a histogram: its timestamp and whether it
/// errored. The CP index keeps these for every flow (tier 1), so the
/// whole-history activity strip never needs the flows themselves.
pub type HistPoint = (DateTime<Utc>, bool);

/// The error rule every explorer view counts by.
pub fn is_error_outcome(o: Outcome) -> bool {
    !matches!(o, Outcome::Ok)
}
```

and change `ExplorerFlow::is_error` to `is_error_outcome(self.flow.outcome)`.

2. Replace `bucketize` with a points-based core plus the flow wrapper:

```rust
fn bucketize_points(
    points: impl Iterator<Item = HistPoint>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    n: usize,
) -> Vec<BucketAgg> {
    let mut buckets = vec![BucketAgg::default(); n];
    for (ts, error) in points {
        if let Some(i) = bucket_index(ts, from, to, n) {
            buckets[i].calls += 1;
            if error {
                buckets[i].errors += 1;
            }
        }
    }
    buckets
}

fn bucketize<'a>(
    flows: impl Iterator<Item = &'a ExplorerFlow>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    n: usize,
) -> Vec<BucketAgg> {
    bucketize_points(flows.map(|f| (f.flow.ts, f.is_error())), from, to, n)
}
```

3. Replace `histogram_view`'s body with a delegation, and add the points version (keep the existing doc comment on `histogram_view`):

```rust
pub fn histogram_view(scope_flows: &[ExplorerFlow], n: usize) -> HistogramView {
    histogram_from_points(scope_flows.iter().map(|f| (f.flow.ts, f.is_error())), n)
}

/// [`histogram_view`] over bare points — what the CP index holds for every
/// flow without keeping the flows.
pub fn histogram_from_points<I>(points: I, n: usize) -> HistogramView
where
    I: IntoIterator<Item = HistPoint>,
    I::IntoIter: Clone,
{
    let it = points.into_iter();
    let mut min: Option<DateTime<Utc>> = None;
    let mut max: Option<DateTime<Utc>> = None;
    for (ts, _) in it.clone() {
        min = Some(min.map_or(ts, |m| m.min(ts)));
        max = Some(max.map_or(ts, |m| m.max(ts)));
    }
    match (min, max) {
        (Some(from), Some(to)) => HistogramView {
            from: Some(from),
            to: Some(to),
            bucket_ms: bucket_ms(from, to, n),
            buckets: bucketize_points(it, from, to, n),
        },
        _ => HistogramView {
            from: None,
            to: None,
            bucket_ms: 0,
            buckets: vec![BucketAgg::default(); n],
        },
    }
}
```

4. Add `SankeyUniverse` and split `sankey_view`:

```rust
/// The topology's node universe — every key present in scope, with the
/// label of its first occurrence (labels are 1:1 with keys for every
/// dimension). Built from the flows themselves ([`Self::from_flows`]) or,
/// by the CP index, key by key from per-file summaries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SankeyUniverse {
    pub workflows: BTreeMap<String, String>,
    pub origins: BTreeMap<String, String>,
    pub orgs: BTreeMap<String, String>,
}

impl SankeyUniverse {
    pub fn add_workflow(&mut self, workflow: Option<&str>) {
        self.workflows
            .entry(workflow_key_of(workflow).to_string())
            .or_insert_with(|| workflow.unwrap_or(UNKNOWN_WORKFLOW_LABEL).to_string());
    }

    /// The origin column labels each node with its key.
    pub fn add_origin_key(&mut self, key: String) {
        self.origins.entry(key.clone()).or_insert(key);
    }

    pub fn add_org(&mut self, asn: Option<&AsnInfo>) {
        self.orgs.entry(org_key_of(asn)).or_insert_with(|| match asn {
            Some(a) => a.org.clone(),
            None => UNKNOWN_ORG_LABEL.to_string(),
        });
    }

    pub fn from_flows(flows: &[ExplorerFlow]) -> Self {
        let mut u = Self::default();
        for f in flows {
            u.add_workflow(f.workflow.as_deref());
            u.add_origin_key(f.origin_key());
            u.add_org(f.asn.as_ref());
        }
        u
    }
}
```

Rename the body of `sankey_view` to `sankey_view_with_universe(universe, flows, window, filters)`, change the `column` closure to take the universe map instead of building `labels` (`column(SkipDim::Workflow, &universe.workflows, |f| f.workflow_key().to_string())`, likewise origins/orgs; the `label` closures go away), iterate `flows` where it iterated `scope_flows`, and make `sankey_view` delegate:

```rust
pub fn sankey_view(scope_flows: &[ExplorerFlow], window: &TimeRange, filters: &ExplorerFilters) -> SankeyView {
    sankey_view_with_universe(&SankeyUniverse::from_flows(scope_flows), scope_flows, window, filters)
}
```

Keep the existing doc comment on `sankey_view`; on `sankey_view_with_universe` document that `flows` must contain every in-scope flow inside `window` (node aggregates and links only ever look at in-window flows).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-netflow --lib ledger::` then `cargo test -p rupu-cp --test it netflow_explorer::`
Expected: PASS — the new tests plus every existing explorer test, unchanged.

- [ ] **Step 5: Clippy, format, commit**

```bash
cargo clippy -p rupu-netflow --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-netflow/src/ledger/explorer.rs crates/rupu-netflow/src/ledger/fold.rs
git add crates/rupu-netflow/src/ledger/explorer.rs crates/rupu-netflow/src/ledger/fold.rs
git commit -m "refactor(netflow): explorer views take whole-history inputs separately"
```

---

### Task 4: `NetflowIndex` core — tier 1 + tier 2, refresh, `LedgerReader`

**Files:**
- Create: `crates/rupu-cp/src/netflow_index.rs`
- Modify: `crates/rupu-cp/src/lib.rs` (`pub mod netflow_index;`)
- Modify: `crates/rupu-config/src/netflow_config.rs` (`cp_index_budget_mb`)

**Interfaces:**
- Consumes: `LedgerFold`, `FoldEvent`, `split_complete_lines`, `CompactRows` (Tasks 1–2); `HistPoint`, `is_error_outcome`, `origin_key` (Task 3); `rupu_runtime::file_cache::FileStamp`.
- Produces (`pub` in `rupu_cp::netflow_index`):
  - `pub trait LedgerReader: Send + Sync { fn flows_in_range(&self, path: &Path, range: &TimeRange) -> (Vec<FlowRecord>, u64); fn capture_states(&self, path: &Path) -> Vec<CaptureEntry>; }`
  - `pub struct DirectReader;` — today's behaviour (`read_flows_in_range(..).unwrap_or_default()`, `read_capture_states(..).unwrap_or_default()`)
  - `pub struct LedgerSummary { pub flow_count: usize, pub dropped: u64, pub min_ts: Option<DateTime<Utc>>, pub max_ts: Option<DateTime<Utc>>, pub points: Vec<HistPoint>, pub origins: BTreeSet<String>, pub peer_ips: BTreeSet<Option<IpAddr>>, pub capture: Vec<CaptureEntry> }` with `pub fn overlaps(&self, range: &TimeRange) -> bool`
  - `pub struct NetflowIndex` with `pub fn new(budget_bytes: u64) -> Self`, `pub fn summary(&self, path: &Path) -> Option<Arc<LedgerSummary>>`, and `impl LedgerReader for NetflowIndex`
  - `pub const DEFAULT_BUDGET_MB: u64 = 256;`
  - `NetflowConfig.cp_index_budget_mb: u64` (default 256)

- [ ] **Step 1: Add the config key.** In `netflow_config.rs`, add the field (with doc) to `NetflowConfig`:

```rust
    /// Memory budget, in MiB, for the flow rows `cp serve`'s netflow index
    /// keeps resident. Over budget, the oldest files' rows are dropped and
    /// re-read on demand: slower, never wrong. Defaults to 256.
    #[serde(default = "NetflowConfig::default_cp_index_budget_mb")]
    pub cp_index_budget_mb: u64,
```

with `fn default_cp_index_budget_mb() -> u64 { 256 }` and `cp_index_budget_mb: Self::default_cp_index_budget_mb(),` in `Default`. Extend the existing defaults test with `assert_eq!(cfg.cp_index_budget_mb, 256);`. Run `cargo test -p rupu-config netflow` → PASS.

- [ ] **Step 2: Write the failing index tests** in `netflow_index.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::record::{CaptureState, LedgerLine, SocketCompletion};
    use rupu_netflow::{Fidelity, FlowCtx, FlowId, Origin, Outcome};
    use std::io::Write;

    fn flow(n: u64, secs: i64) -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(n, n),
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
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn all() -> TimeRange {
        TimeRange::unbounded()
    }

    /// The index must answer exactly what the direct reader answers.
    fn assert_same(index: &NetflowIndex, path: &Path, range: &TimeRange) {
        assert_eq!(index.flows_in_range(path, range), DirectReader.flows_in_range(path, range));
        assert_eq!(index.capture_states(path), DirectReader.capture_states(path));
    }

    #[test]
    fn a_new_file_reads_like_the_direct_reader() {
        let tmp = tempfile::TempDir::new().unwrap();
        let p = tmp.path().join("run_a.jsonl");
        append(&p, &line(&LedgerLine::Flow(Box::new(flow(1, 100)))));
        append(&p, &line(&LedgerLine::Dropped { count: 2, ts: chrono::DateTime::from_timestamp(150, 0).unwrap() }));
        append(&p, &line(&LedgerLine::Capture {
            ts: chrono::DateTime::from_timestamp(160, 0).unwrap(),
            state: CaptureState::Unavailable { reason: "no ntstat".into() },
            tool_call_id: None,
            note: None,
        }));
        let index = NetflowIndex::new(u64::MAX);
        assert_same(&index, &p, &all());
        let bounded = TimeRange { from: Some(chrono::DateTime::from_timestamp(150, 0).unwrap()), to: None };
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
        append(&p, &line(&LedgerLine::SocketComplete(SocketCompletion {
            id: FlowId::from_parts(1, 1),
            duration_ms: 5,
            bytes_in: Some(1),
            bytes_out: None,
            outcome: Some(Outcome::Timeout),
            error: None,
        })));
        assert_same(&index, &p, &all());
        let s = index.summary(&p).unwrap();
        assert_eq!(s.flow_count, 2);
        assert_eq!(s.points[0].1, true, "the patched outcome must flip the histogram point");
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
        std::fs::write(&p, format!("{}{}", line(&LedgerLine::Flow(Box::new(flow(7, 700)))), line(&LedgerLine::Flow(Box::new(flow(8, 800)))))).unwrap();
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
        append(&p, &line(&LedgerLine::Dropped { count: 4, ts: chrono::DateTime::from_timestamp(100, 0).unwrap() }));
        let index = NetflowIndex::new(u64::MAX);
        let later = TimeRange { from: Some(chrono::DateTime::from_timestamp(500, 0).unwrap()), to: None };
        assert_eq!(index.flows_in_range(&p, &later), (vec![], 4));
        assert_same(&index, &p, &later);
    }
}
```

`status()` is added in Task 5; for this task, add a minimal `pub fn status(&self) -> IndexStatus` returning only `files` (Task 5 fills the rest) — define `IndexStatus` now with all seven fields and fill the others with `0` until Task 5.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p rupu-cp --lib netflow_index`
Expected: FAIL to compile — module items not defined.

- [ ] **Step 4: Implement `netflow_index.rs`** (above the tests):

```rust
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

use rupu_netflow::ledger::explorer::{is_error_outcome, origin_key, HistPoint};
use rupu_netflow::ledger::{
    read_capture_states, read_flows_in_range, split_complete_lines, CaptureEntry, CompactRows,
    FoldEvent, LedgerFold, TimeRange,
};
use rupu_netflow::FlowRecord;
use rupu_runtime::file_cache::FileStamp;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
}

pub struct NetflowIndex {
    entries: Mutex<HashMap<PathBuf, Arc<Mutex<Entry>>>>,
    resident_bytes: AtomicU64,
    budget_bytes: AtomicU64,
    evictions: AtomicU64,
}

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
        }
    }

    /// Tier 1 for one ledger, refreshed; `None` when the file is gone.
    pub fn summary(&self, path: &Path) -> Option<Arc<LedgerSummary>> {
        let entry = self.entry(path);
        let out = {
            let mut e = entry.lock().unwrap_or_else(|p| p.into_inner());
            if !self.refresh(&mut e, path) {
                None
            } else {
                Some(Arc::clone(&e.summary))
            }
        };
        if out.is_none() {
            self.remove(path);
        }
        self.enforce_budget();
        out
    }

    fn entry(&self, path: &Path) -> Arc<Mutex<Entry>> {
        let mut map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        Arc::clone(map.entry(path.to_path_buf()).or_default())
    }

    fn remove(&self, path: &Path) {
        let removed = self.entries.lock().unwrap_or_else(|p| p.into_inner()).remove(path);
        if let Some(entry) = removed {
            if let Ok(mut e) = entry.lock() {
                self.drop_resident(&mut e);
            }
        }
    }

    fn drop_resident(&self, e: &mut Entry) {
        if let Some(r) = e.resident.take() {
            self.resident_bytes.fetch_sub(r.accounted, Ordering::Relaxed);
        }
    }

    fn account(&self, e: &mut Entry) {
        if let Some(r) = e.resident.as_mut() {
            let now = r.bytes();
            if now >= r.accounted {
                self.resident_bytes.fetch_add(now - r.accounted, Ordering::Relaxed);
            } else {
                self.resident_bytes.fetch_sub(r.accounted - now, Ordering::Relaxed);
            }
            r.accounted = now;
        }
    }

    /// Bring `e` up to date with the file. `false` = the file is gone.
    fn refresh(&self, e: &mut Entry, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        let stamp = FileStamp::of(&meta);
        if e.stamp == Some(stamp) {
            // Unchanged file: tier 1 is current; tier 2 may be evicted,
            // which `ensure_rows` handles.
            return true;
        }
        let ino = inode(&meta);
        let can_tail = e.stamp.is_some()
            && !e.unreadable
            && e.resident.is_some()
            && ino == e.ino
            && meta.len() >= e.offset
            && self.prefix_matches(path, &e.prefix);
        if can_tail {
            self.tail(e, path);
        } else {
            self.full_read(e, path);
        }
        e.stamp = Some(stamp);
        e.ino = ino;
        true
    }

    fn prefix_matches(&self, path: &Path, prefix: &[u8]) -> bool {
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        let mut buf = vec![0u8; prefix.len()];
        f.read_exact(&mut buf).is_ok() && buf == prefix
    }

    fn full_read(&self, e: &mut Entry, path: &Path) {
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
        self.tail(e, path);
    }

    /// Feed everything after `e.offset` that ends in a newline.
    fn tail(&self, e: &mut Entry, path: &Path) {
        let mut buf = Vec::new();
        let read = std::fs::File::open(path).and_then(|mut f| {
            f.seek(SeekFrom::Start(e.offset))?;
            f.read_to_end(&mut buf)
        });
        if read.is_err() {
            return;
        }
        let (complete, used) = split_complete_lines(&buf);
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
        let resident = e.resident.as_mut().expect("tail runs with rows resident");
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
    fn ensure_rows(&self, e: &mut Entry, path: &Path) {
        if e.resident.is_none() && !e.unreadable {
            self.full_read(e, path);
        }
    }

    /// Placeholder until Task 5 adds eviction.
    fn enforce_budget(&self) {}
}

impl LedgerReader for NetflowIndex {
    fn flows_in_range(&self, path: &Path, range: &TimeRange) -> (Vec<FlowRecord>, u64) {
        let entry = self.entry(path);
        let out = {
            let mut e = entry.lock().unwrap_or_else(|p| p.into_inner());
            if !self.refresh(&mut e, path) {
                None
            } else if e.unreadable {
                Some((Vec::new(), 0))
            } else if !e.summary.overlaps(range) {
                Some((Vec::new(), e.summary.dropped))
            } else {
                self.ensure_rows(&mut e, path);
                let dropped = e.summary.dropped;
                let rows = &e.resident.as_ref().expect("rows ensured").rows;
                Some((rows.records_in_range(range).collect(), dropped))
            }
        };
        let Some(out) = out else {
            self.remove(path);
            return (Vec::new(), 0);
        };
        self.enforce_budget();
        out
    }

    fn capture_states(&self, path: &Path) -> Vec<CaptureEntry> {
        self.summary(path).map(|s| s.capture.clone()).unwrap_or_default()
    }
}
```

Note the deliberate rule: an unchanged stamp is trusted (ledgers are append-only and timestamps are nanosecond on APFS/ext4); any stamp change opens the file and checks the recorded prefix before tailing.

In `lib.rs` add `pub mod netflow_index;` next to the other top-level modules.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-cp --lib netflow_index`
Expected: PASS (8 tests).

- [ ] **Step 6: Clippy, format, commit**

```bash
cargo clippy -p rupu-cp -p rupu-config --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-cp/src/netflow_index.rs crates/rupu-config/src/netflow_config.rs
git add crates/rupu-cp/src/netflow_index.rs crates/rupu-cp/src/lib.rs crates/rupu-config/src/netflow_config.rs
git commit -m "feat(cp): NetflowIndex — stat-validated, tailed per-ledger index behind LedgerReader"
```

---

### Task 5: Budget, eviction, status, `retain_only`

**Files:**
- Modify: `crates/rupu-cp/src/netflow_index.rs`

**Interfaces:**
- Produces: `pub fn set_budget_bytes(&self, bytes: u64)`, `pub fn status(&self) -> IndexStatus`, `pub fn retain_only(&self, keep: &std::collections::HashSet<PathBuf>)`, and
  `#[derive(Debug, Clone, Serialize, PartialEq, Eq)] pub struct IndexStatus { pub files: u64, pub flows: u64, pub tier1_bytes: u64, pub tier2_bytes: u64, pub budget_bytes: u64, pub resident_files: u64, pub evictions_total: u64 }`

- [ ] **Step 1: Write the failing tests** (add to the test module):

```rust
    fn ledger_with(dir: &Path, name: &str, base: u64, secs: i64, n: u64) -> PathBuf {
        let p = dir.join(name);
        for i in 0..n {
            append(&p, &line(&LedgerLine::Flow(Box::new(flow(base + i, secs + i as i64)))));
        }
        p
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
        // Evicted rows are re-read on demand and the answer is unchanged.
        assert_same(&index, &old, &all());
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
    fn retain_only_drops_entries_not_listed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = ledger_with(tmp.path(), "run_a.jsonl", 0, 100, 3);
        let b = ledger_with(tmp.path(), "run_b.jsonl", 10, 100, 4);
        let index = NetflowIndex::new(u64::MAX);
        index.summary(&a);
        index.summary(&b);
        index.retain_only(&std::iter::once(a.clone()).collect());
        let st = index.status();
        assert_eq!((st.files, st.flows), (1, 3));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-cp --lib netflow_index`
Expected: FAIL — `set_budget_bytes` / `retain_only` missing; eviction test fails on `resident_files`.

- [ ] **Step 3: Implement.** Replace the placeholder `enforce_budget` and the minimal `status`, and add the two methods:

```rust
    pub fn set_budget_bytes(&self, bytes: u64) {
        self.budget_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Drop tier 2 of the files with the oldest newest-flow first until the
    /// resident rows fit the budget. Entries locked by an in-flight read are
    /// skipped this pass (they are re-checked on the next call).
    fn enforce_budget(&self) {
        let budget = self.budget_bytes.load(Ordering::Relaxed);
        if self.resident_bytes.load(Ordering::Relaxed) <= budget {
            return;
        }
        let mut candidates: Vec<(Option<DateTime<Utc>>, Arc<Mutex<Entry>>)> = {
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
                    self.resident_bytes.fetch_sub(r.accounted, Ordering::Relaxed);
                    freed += r.accounted;
                    evicted += 1;
                }
            }
        }
        if evicted > 0 {
            self.evictions.fetch_add(evicted, Ordering::Relaxed);
            tracing::debug!(evicted, freed_bytes = freed, budget_bytes = budget, "netflow index evicted rows");
        }
    }

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

    /// Forget every entry whose path is not in `keep` (called with the full
    /// global listing, so pruned or unregistered ledgers do not linger).
    pub fn retain_only(&self, keep: &std::collections::HashSet<PathBuf>) {
        let dropped: Vec<Arc<Mutex<Entry>>> = {
            let mut map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
            let gone: Vec<PathBuf> = map.keys().filter(|p| !keep.contains(*p)).cloned().collect();
            gone.into_iter().filter_map(|p| map.remove(&p)).collect()
        };
        for entry in dropped {
            if let Ok(mut e) = entry.lock() {
                self.drop_resident(&mut e);
            }
        }
    }
```

Note on the zero-budget test: a read admits the file's rows, answers, releases the entry lock, then `enforce_budget` evicts them — so `tier2_bytes` returns to 0 after every call while every answer stays exact.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp --lib netflow_index`
Expected: PASS (12 tests).

- [ ] **Step 5: Clippy, format, commit**

```bash
cargo clippy -p rupu-cp --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-cp/src/netflow_index.rs
git add crates/rupu-cp/src/netflow_index.rs
git commit -m "feat(cp): netflow index memory budget — evict oldest rows, re-read on demand"
```

---

### Task 6: Route every local ledger read through `LedgerReader`

**Files:**
- Modify: `crates/rupu-cp/src/api/netflow.rs`
- Modify: `crates/rupu-cp/src/state.rs` (`netflow_index` field)
- Modify: `crates/rupu-cli/src/cmd/netflow.rs` (`DirectReader`)

**Interfaces:**
- Consumes: `LedgerReader`, `DirectReader`, `NetflowIndex`, `DEFAULT_BUDGET_MB` (Tasks 4–5).
- Produces (in `api/netflow.rs`):
  - `fn ledger_files_in_dir(netflow_dir: &StdPath) -> Vec<(String, PathBuf)>` — `read_dir` order, filtered by `is_per_run_ledger_path`, id = file stem
  - `fn global_ledger_files(global_dir: &StdPath) -> Vec<(String, PathBuf)>`
  - `fn project_ledger_files(meta: &RunMetaIndex, workspace: &StdPath, global_dir: &StdPath) -> Vec<(String, PathBuf)>`
  - `fn read_ledger_files(reader: &dyn LedgerReader, files: &[(String, PathBuf)], range: &TimeRange) -> (Vec<(String, FlowRecord)>, u64)`
  - `fn index_for(s: &AppState) -> Arc<NetflowIndex>` — applies the configured budget, returns the shared index
  - signature changes (first parameter `reader: &dyn LedgerReader`): `read_all_workspaces_sync`, `project_scoped_flows_meta_and_dropped`, `run_scoped_flows_and_dropped`, `run_scoped_flows_and_dropped_with`, `collect_run_netflow`
  - `AppState.netflow_index: Arc<crate::netflow_index::NetflowIndex>`

- [ ] **Step 1: Write the failing equivalence test** — add to `api/netflow.rs`'s `#[cfg(test)] mod tests`:

```rust
    use crate::netflow_index::{DirectReader, LedgerReader, NetflowIndex};

    fn ledger(dir: &std::path::Path, id: &str, flows: &[FlowRecord], dropped: u64) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(format!("{id}.jsonl"));
        let mut text = String::new();
        for f in flows {
            text.push_str(&serde_json::to_string(&rupu_netflow::record::LedgerLine::Flow(Box::new(f.clone()))).unwrap());
            text.push('\n');
        }
        if dropped > 0 {
            text.push_str(&serde_json::to_string(&rupu_netflow::record::LedgerLine::Dropped { count: dropped, ts: chrono::Utc::now() }).unwrap());
            text.push('\n');
        }
        std::fs::write(&p, text).unwrap();
        p
    }

    fn at(f: FlowRecord, secs: i64) -> FlowRecord {
        FlowRecord { ts: chrono::DateTime::from_timestamp(secs, 0).unwrap(), ..f }
    }

    /// Spec §3.6 for the flows lists: identical JSON through the index
    /// (ample budget, and a zero budget that forces re-reads) and through
    /// the direct reader, across windows.
    #[test]
    fn flows_lists_are_identical_through_the_index() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("netflow");
        ledger(&dir, "run_a", &[at(flow(FlowId::new(), None, "a.com"), 100), at(flow(FlowId::new(), None, "b.com"), 300)], 2);
        ledger(&dir, "run_b", &[at(flow(FlowId::new(), None, "c.com"), 200)], 0);
        let files = ledger_files_in_dir(&dir);
        let ranges = [
            rupu_netflow::ledger::TimeRange::unbounded(),
            rupu_netflow::ledger::TimeRange { from: Some(chrono::DateTime::from_timestamp(150, 0).unwrap()), to: None },
            rupu_netflow::ledger::TimeRange { from: None, to: Some(chrono::DateTime::from_timestamp(150, 0).unwrap()) },
            rupu_netflow::ledger::TimeRange { from: Some(chrono::DateTime::from_timestamp(500, 0).unwrap()), to: Some(chrono::DateTime::from_timestamp(100, 0).unwrap()) },
        ];
        for budget in [u64::MAX, 0] {
            let index = NetflowIndex::new(budget);
            for range in &ranges {
                let (want, want_dropped) = read_ledger_files(&DirectReader, &files, range);
                let (got, got_dropped) = read_ledger_files(&index, &files, range);
                let want = build_filtered_response(want, &RunMetaIndex::default(), want_dropped, None, range, &ExplorerFilters::default());
                let got = build_filtered_response(got, &RunMetaIndex::default(), got_dropped, None, range, &ExplorerFilters::default());
                assert_eq!(serde_json::to_value(&got).unwrap(), serde_json::to_value(&want).unwrap(), "budget {budget}, range {range:?}");
            }
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-cp --lib api::netflow::tests::flows_lists_are_identical_through_the_index`
Expected: FAIL to compile — `ledger_files_in_dir`, `read_ledger_files` not defined.

- [ ] **Step 3: Split listing from reading.** In `api/netflow.rs`, add `use crate::netflow_index::{LedgerReader, NetflowIndex};` to the imports (and `DirectReader` inside the test module). Keep every existing doc comment, moving it to whichever new function now owns that responsibility:

```rust
fn ledger_files_in_dir(netflow_dir: &StdPath) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(netflow_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !is_per_run_ledger_path(&path) {
                return None;
            }
            let id = path.file_stem().and_then(|s| s.to_str())?.to_string();
            Some((id, path))
        })
        .collect()
}

fn read_ledger_files(
    reader: &dyn LedgerReader,
    files: &[(String, PathBuf)],
    range: &rupu_netflow::ledger::TimeRange,
) -> (Vec<(String, FlowRecord)>, u64) {
    let mut flows = Vec::new();
    let mut dropped = 0u64;
    for (id, path) in files {
        let (f, d) = reader.flows_in_range(path, range);
        flows.extend(f.into_iter().map(|flow| (id.clone(), flow)));
        dropped += d;
    }
    (flows, dropped)
}
```

Then:
- Delete `read_all_run_ledgers_in_dir` (its two callers use `ledger_files_in_dir` + `read_ledger_files`).
- Turn `project_fallback_flows_and_dropped` into `fn project_fallback_ledger_files(meta: &RunMetaIndex, global_dir: &StdPath) -> Vec<(String, PathBuf)>`: build the same `HashSet<PathBuf>` as today, then map each path to `(stem, path)` (skip non-UTF-8 stems as today).
- Add:

```rust
fn project_ledger_files(meta: &RunMetaIndex, workspace: &StdPath, global_dir: &StdPath) -> Vec<(String, PathBuf)> {
    let local_dir = project_local_netflow_dir(workspace);
    let mut files = ledger_files_in_dir(&local_dir);
    if canonicalize_or_self(&local_dir) != canonicalize_or_self(&global_netflow_dir(global_dir)) {
        files.extend(project_fallback_ledger_files(meta, global_dir));
    }
    files
}

fn global_ledger_files(global_dir: &StdPath) -> Vec<(String, PathBuf)> {
    // (body = today's `read_all_workspaces_sync` directory union, unchanged)
    let workspaces = (WorkspaceStore { root: global_dir.join("workspaces") }).list().unwrap_or_default();
    let mut dirs: std::collections::HashSet<PathBuf> = workspaces
        .iter()
        .map(|w| canonicalize_or_self(&project_local_netflow_dir(std::path::Path::new(&w.path))))
        .collect();
    dirs.insert(canonicalize_or_self(&global_netflow_dir(global_dir)));
    dirs.iter().flat_map(|d| ledger_files_in_dir(d)).collect()
}
```

- `project_scoped_flows_meta_and_dropped(reader, global_store, workspace, global_dir, range)`: `let meta = project_run_meta(..); let files = project_ledger_files(&meta, workspace, global_dir); let (flows, dropped) = read_ledger_files(reader, &files, range); (flows, meta, dropped)`.
- `read_all_workspaces_sync(reader, global_dir, range)`: `read_ledger_files(reader, &global_ledger_files(global_dir), range)`.
- `run_scoped_flows_and_dropped{,_with}`: add `reader: &dyn LedgerReader` first; replace `rupu_netflow::ledger::read_flows_in_range(&ledger_path, range).unwrap_or_default()` with `reader.flows_in_range(&ledger_path, range)`.
- `collect_run_netflow`: add `reader: &dyn LedgerReader` first; pass it on; replace `rupu_netflow::ledger::read_capture_states(&ledger_path).unwrap_or_default()` with `reader.capture_states(&ledger_path)`.

- [ ] **Step 4: Wire the index into the handlers.** In `state.rs` add the field `pub netflow_index: Arc<crate::netflow_index::NetflowIndex>,` and in the constructor `netflow_index: Arc::new(crate::netflow_index::NetflowIndex::new(crate::netflow_index::DEFAULT_BUDGET_MB * 1024 * 1024)),`. In `api/netflow.rs`:

```rust
/// The shared index with the configured budget applied (read per request,
/// so a config edit takes effect without a restart).
fn index_for(s: &AppState) -> Arc<NetflowIndex> {
    let mb = netflow_config(s).cp_index_budget_mb;
    s.netflow_index.set_budget_bytes(mb.saturating_mul(1024 * 1024));
    Arc::clone(&s.netflow_index)
}
```

Every handler/closure that called one of the changed functions now does `let index = index_for(&state);` before `run_blocking`, moves `index` into the closure, and passes `&*index` as the reader: `get_project_netflow`, `get_global_netflow`, `run_scoped_flows_for_graph`, `get_netflow_graph`, `get_netflow_explorer` (project/global branches — Task 7 replaces these two), `explorer_run_scope` (Global/ProjectLocal), and both `collect_run_netflow` call sites. Unit tests in this module that call `run_scoped_flows_and_dropped_with` pass `&DirectReader`.

In `crates/rupu-cli/src/cmd/netflow.rs` `collect_run_ledger`: pass `&rupu_cp::netflow_index::DirectReader` as the new first argument (the CLI is a one-shot; it keeps reading files directly).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-cp --lib api::netflow` then `cargo test -p rupu-cp --test it netflow_api:: netflow_explorer:: host_run_netflow:: graph::` then `cargo test -p rupu-cli netflow`
Expected: PASS — the new equivalence test and every existing netflow test.

- [ ] **Step 6: Clippy, format, commit**

```bash
cargo clippy -p rupu-cp -p rupu-cli --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-cp/src/api/netflow.rs crates/rupu-cp/src/state.rs crates/rupu-cli/src/cmd/netflow.rs
git add crates/rupu-cp/src/api/netflow.rs crates/rupu-cp/src/state.rs crates/rupu-cli/src/cmd/netflow.rs
git commit -m "perf(cp): every local netflow read goes through the shared NetflowIndex"
```

---

### Task 7: Global/project explorer from the index

**Files:**
- Modify: `crates/rupu-cp/src/api/netflow.rs`

**Interfaces:**
- Consumes: `NetflowIndex::summary`, `LedgerReader::flows_in_range`, `retain_only` (Tasks 4–5); `histogram_from_points`, `SankeyUniverse`, `sankey_view_with_universe` (Task 3); `global_ledger_files`, `project_ledger_files`, `index_for` (Task 6).
- Produces: `pub(crate) fn indexed_explorer(index: &NetflowIndex, files: &[(String, PathBuf)], meta: &RunMetaIndex, table: Option<&AsnTable>, range: &TimeRange, filters: &ExplorerFilters) -> ExplorerResponse`

- [ ] **Step 1: Write the failing equivalence test** (in the same test module):

```rust
    fn asn_table() -> AsnTable {
        AsnTable::compact_from_tsv(std::io::Cursor::new(
            "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n2.0.0.0\t2.0.0.255\t15169\tUS\tGOOGLE\n",
        ))
        .unwrap()
    }

    fn with_peer(f: FlowRecord, ip: Option<&str>, ok: bool) -> FlowRecord {
        FlowRecord {
            peer_ip: ip.map(|s| s.parse().unwrap()),
            outcome: if ok { rupu_netflow::Outcome::Ok } else { rupu_netflow::Outcome::HttpError },
            ..f
        }
    }

    /// Spec §3.6 for the explorer: `indexed_explorer` (ample and zero
    /// budget) serializes identically to `build_explorer_response` over the
    /// full direct read, across windows and filters.
    #[test]
    fn the_explorer_is_identical_through_the_index() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("netflow");
        ledger(&dir, "run_a", &[
            at(with_peer(flow(FlowId::new(), None, "a.com"), Some("1.0.0.1"), true), 100),
            at(with_peer(flow(FlowId::new(), None, "b.com"), Some("2.0.0.1"), false), 300),
        ], 2);
        ledger(&dir, "run_b", &[at(with_peer(flow(FlowId::new(), None, "a.com"), None, true), 200)], 0);
        ledger(&dir, "run_c", &[], 5);
        let files = ledger_files_in_dir(&dir);
        let table = asn_table();
        let meta = RunMetaIndex::default();
        let t = |s| Some(chrono::DateTime::from_timestamp(s, 0).unwrap());
        let ranges = [
            rupu_netflow::ledger::TimeRange::unbounded(),
            rupu_netflow::ledger::TimeRange { from: t(150), to: None },
            rupu_netflow::ledger::TimeRange { from: t(150), to: t(250) },
            rupu_netflow::ledger::TimeRange { from: t(900), to: None },
            rupu_netflow::ledger::TimeRange { from: t(500), to: t(100) },
        ];
        let filter_sets = [
            ExplorerFilters::default(),
            ExplorerFilters { orgs: vec!["as13335".into()], ..Default::default() },
            ExplorerFilters { hosts: vec!["a.com:443".into()], ..Default::default() },
        ];
        for budget in [u64::MAX, 0] {
            let index = NetflowIndex::new(budget);
            for range in &ranges {
                for filters in &filter_sets {
                    let (tagged, dropped) = read_ledger_files(&DirectReader, &files, &rupu_netflow::ledger::TimeRange::unbounded());
                    let flows = to_explorer_flows(tagged, &meta, Some(&table));
                    let want = build_explorer_response(&flows, dropped, &meta.spans, true, range, filters);
                    let got = indexed_explorer(&index, &files, &meta, Some(&table), range, filters);
                    assert_eq!(
                        serde_json::to_value(&got).unwrap(),
                        serde_json::to_value(&want).unwrap(),
                        "budget {budget}, range {range:?}, filters {filters:?}"
                    );
                }
            }
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-cp --lib api::netflow::tests::the_explorer_is_identical_through_the_index`
Expected: FAIL to compile — `indexed_explorer` not defined.

- [ ] **Step 3: Implement `indexed_explorer`** next to `build_explorer_response`:

```rust
/// [`build_explorer_response`] for a global/project scope served from the
/// netflow index: the whole-history views (histogram, topology node
/// universe, dropped total) come from tier-1 summaries of every scope file;
/// the windowed views read rows only from files overlapping the resolved
/// window. Files are visited in `files` order and rows in file order — the
/// same order the direct read produces — so order-dependent details (a
/// timeline lane's org attribution) are identical.
pub(crate) fn indexed_explorer(
    index: &NetflowIndex,
    files: &[(String, PathBuf)],
    meta: &RunMetaIndex,
    table: Option<&AsnTable>,
    range: &rupu_netflow::ledger::TimeRange,
    filters: &ExplorerFilters,
) -> ExplorerResponse {
    let summaries: Vec<(&str, &StdPath, Arc<crate::netflow_index::LedgerSummary>)> = files
        .iter()
        .filter_map(|(id, p)| index.summary(p).map(|s| (id.as_str(), p.as_path(), s)))
        .collect();
    let dropped = summaries.iter().map(|(_, _, s)| s.dropped).sum();
    let histogram = explorer::histogram_from_points(
        summaries.iter().flat_map(|(_, _, s)| s.points.iter().copied()),
        EXPLORER_BUCKETS,
    );

    let mut universe = explorer::SankeyUniverse::default();
    for (id, _, s) in &summaries {
        if s.flow_count == 0 {
            continue;
        }
        let (_, workflow) = meta.attribution(id);
        universe.add_workflow(workflow.as_deref());
        for origin in &s.origins {
            universe.add_origin_key(origin.clone());
        }
        for ip in &s.peer_ips {
            let asn = ip.and_then(|ip| table.and_then(|t| t.lookup(ip)));
            universe.add_org(asn.as_ref());
        }
    }

    // Same resolution as `build_explorer_response`; every flow any windowed
    // view can count lies inside [from, to].
    let epoch = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;
    let from = range.from.or(histogram.from).unwrap_or(epoch);
    let to = range.to.or(histogram.to).unwrap_or(from);
    let rows_window = rupu_netflow::ledger::TimeRange { from: Some(from), to: Some(to) };
    let mut tagged = Vec::new();
    for (id, path, s) in &summaries {
        if s.overlaps(&rows_window) {
            let (flows, _) = index.flows_in_range(path, &rows_window);
            tagged.extend(flows.into_iter().map(|f| (id.to_string(), f)));
        }
    }
    let windowed = to_explorer_flows(tagged, meta, table);

    let timeline = explorer::timeline_view(&windowed, from, to, filters, &meta.spans, EXPLORER_BUCKETS);
    let sankey = explorer::sankey_view_with_universe(&universe, &windowed, range, filters);
    let kpis = explorer::kpi_view(
        windowed
            .iter()
            .filter(|f| range.contains(f.flow.ts) && filters.passes(f, None)),
    );
    ExplorerResponse {
        sankey,
        timeline,
        histogram,
        kpis,
        dropped_total: dropped,
        asn_loaded: table.is_some(),
        window: WindowEcho::from(range),
        incomplete: Vec::new(),
    }
}
```

If `ExplorerResponse` has fields beyond these, copy them exactly as `build_explorer_response` sets them.

- [ ] **Step 4: Switch the project/global explorer branches** in `get_netflow_explorer`:

Project branch closure body:

```rust
            let meta = project_run_meta(&store, &workspace);
            let files = project_ledger_files(&meta, &workspace, &global_dir);
            let table = load_asn_table(&cache);
            let mut resp = indexed_explorer(&index, &files, &meta, table.as_deref(), &range, &filters);
            resp.incomplete = scope_gap_from_workers(&meta);
            resp
```

Global branch closure body:

```rust
            let files = global_ledger_files(&global_dir);
            index.retain_only(&files.iter().map(|(_, p)| p.clone()).collect());
            let ledger_ids = files
                .iter()
                .filter(|(_, p)| index.summary(p).is_some_and(|s| s.flow_count > 0))
                .map(|(id, _)| id.clone())
                .collect();
            let meta = global_run_meta_cached(&meta_cache, &global_dir, &store, &ledger_ids);
            let table = load_asn_table(&cache);
            let mut resp = indexed_explorer(&index, &files, &meta, table.as_deref(), &range, &filters);
            resp.incomplete = scope_gap_from_workers(&meta);
            resp
```

(`ledger_ids` = files with at least one flow — the same set today's `tagged` ids produced from an unbounded read.) Both closures take `let index = index_for(&state);` moved in. `range` / `filters` are moved in as they are today.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-cp --lib api::netflow` then `cargo test -p rupu-cp --test it netflow_explorer:: netflow_api::`
Expected: PASS.

- [ ] **Step 6: Clippy, format, commit**

```bash
cargo clippy -p rupu-cp --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-cp/src/api/netflow.rs
git add crates/rupu-cp/src/api/netflow.rs
git commit -m "perf(cp): global/project explorer reads whole-history views from index summaries"
```

---

### Task 8: Prewarm, status endpoint, docs

**Files:**
- Modify: `crates/rupu-cp/src/api/netflow.rs` (route + handler + prewarm helper)
- Modify: `crates/rupu-cp/src/lib.rs` (prewarm at serve start)
- Test: `crates/rupu-cp/tests/it/netflow_api.rs` (add one test; reuses its `new_state` / `serve` / `write_global_ledger` / `e2e_flow` helpers)
- Modify: `docs/configuration.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: `index_for`, `global_ledger_files`, `NetflowIndex::{summary, retain_only, status}`.
- Produces: `GET /api/netflow/index` → `IndexStatus` JSON; `pub(crate) fn prewarm_netflow_index(index: &NetflowIndex, global_dir: &StdPath) -> usize`.

- [ ] **Step 1: Write the failing integration test** — append to `tests/it/netflow_api.rs`:

```rust
/// `GET /api/netflow/index` reports the index after a global read — and a
/// second read returns the same flows from the index.
#[tokio::test]
async fn netflow_index_status_reports_files_flows_and_budget() {
    let global = tempfile::tempdir().unwrap();
    write_global_ledger(
        global.path(),
        "run-indexed",
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some("run-indexed"),
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
        )))],
    );
    let addr = serve(new_state(global.path())).await;

    let first: serde_json::Value = reqwest::get(format!("http://{addr}/api/netflow"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: serde_json::Value = reqwest::get(format!("http://{addr}/api/netflow"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first, second, "a cached read must answer exactly like the first");

    let resp = reqwest::get(format!("http://{addr}/api/netflow/index")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let status: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(status["files"], 1, "{status}");
    assert_eq!(status["flows"], 1, "{status}");
    assert_eq!(status["budget_bytes"], 256 * 1024 * 1024, "{status}");
    for key in ["tier1_bytes", "tier2_bytes", "resident_files", "evictions_total"] {
        assert!(status[key].is_u64(), "missing {key}: {status}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-cp --test it netflow_api::netflow_index_status`
Expected: FAIL — 404 on `/api/netflow/index`.

- [ ] **Step 3: Implement.** In `api/netflow.rs`:

```rust
        .route("/api/netflow/index", get(get_netflow_index))
```

```rust
/// `GET /api/netflow/index` — the netflow index's size, budget and
/// evictions (spec §3.5). API only.
async fn get_netflow_index(
    State(state): State<AppState>,
) -> Json<crate::netflow_index::IndexStatus> {
    Json(index_for(&state).status())
}

/// Read every local ledger into the index once, so the first Network page
/// load does not pay for opening every file. Returns the file count.
pub(crate) fn prewarm_netflow_index(index: &NetflowIndex, global_dir: &StdPath) -> usize {
    let files = global_ledger_files(global_dir);
    for (_, path) in &files {
        index.summary(path);
    }
    index.retain_only(&files.iter().map(|(_, p)| p.clone()).collect());
    files.len()
}
```

In `lib.rs`, next to the usage prewarm block:

```rust
    // Read every netflow ledger into the index in the background, so the
    // first Network page load does not pay for opening every file.
    {
        let global = app_state.global_dir.clone();
        let index = std::sync::Arc::clone(&app_state.netflow_index);
        let budget_mb = app_state
            .config
            .read()
            .map(|c| c.netflow.cp_index_budget_mb)
            .unwrap_or(crate::netflow_index::DEFAULT_BUDGET_MB);
        tokio::task::spawn_blocking(move || {
            index.set_budget_bytes(budget_mb.saturating_mul(1024 * 1024));
            let started = std::time::Instant::now();
            let files = crate::api::netflow::prewarm_netflow_index(&index, &global);
            let st = index.status();
            info!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                files,
                flows = st.flows,
                tier1_bytes = st.tier1_bytes,
                tier2_bytes = st.tier2_bytes,
                "netflow index warmed"
            );
        });
    }
```

(Match how `app_state.config` is accessed elsewhere in `lib.rs`; if it is not a `std::sync::RwLock`, use the same accessor `netflow_config` uses.)

- [ ] **Step 4: Docs.** In `docs/configuration.md`, add a `## [netflow]` section before `## [cp]`:

```markdown
## `[netflow]`

| Key | Default | Meaning |
|---|---|---|
| `cp_index_budget_mb` | `256` | Memory budget for the flow rows `rupu cp serve` keeps in its netflow index. Over budget, the rows of the files with the oldest flows are dropped and re-read from disk when a view needs them: wide windows get slower, answers never change. Per-file summaries (timestamps, counts, origins) always stay in memory. Check usage with `GET /api/netflow/index`. Read on every request — no restart needed. |
```

In `CLAUDE.md`, in the `rupu-cp` bullet, after the `GET /api/hosts` load-time rules sentence, add:

```markdown
Netflow reads go through `netflow_index::NetflowIndex` (`AppState.netflow_index`, prewarmed in `serve_on`): a per-ledger drop-in (`LedgerReader`) that stats instead of re-opening, tails appended lines, keeps per-file summaries resident and compact rows under `[netflow].cp_index_budget_mb`; the global/project explorer builds whole-history views from the summaries (`indexed_explorer`). Status: `GET /api/netflow/index`.
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-cp --test it netflow_api::`
Expected: PASS.

- [ ] **Step 6: Clippy, format, commit**

```bash
cargo clippy -p rupu-cp --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-cp/src/api/netflow.rs crates/rupu-cp/src/lib.rs crates/rupu-cp/tests/it/netflow_api.rs
git add crates/rupu-cp/src/api/netflow.rs crates/rupu-cp/src/lib.rs crates/rupu-cp/tests/it/netflow_api.rs docs/configuration.md CLAUDE.md
git commit -m "feat(cp): prewarm the netflow index at serve start; GET /api/netflow/index"
```

---

### Task 9: Web — independent section loading and the 24h default

**Files:**
- Modify: `crates/rupu-cp/web/src/components/netflow/TimeRangePicker.tsx`
- Modify: `crates/rupu-cp/web/src/components/netflow/explorer/NetflowExplorer.tsx`
- Modify: `crates/rupu-cp/web/src/components/netflow/explorer/NetflowExplorer.test.tsx`
- Modify (assertion updates only, if they assert the default window): `crates/rupu-cp/web/src/pages/Netflow.test.tsx`, `crates/rupu-cp/web/src/components/project/ProjectNetworkTab.test.tsx`

**Interfaces:**
- Produces: `export function presetValue(preset: Exclude<TimeRangePreset, 'custom'>, now: Date): TimeRangeValue` from `TimeRangePicker.tsx`.

- [ ] **Step 1: Write the failing tests.** In `NetflowExplorer.test.tsx`, pin the clock and add a `DAY` constant:

```ts
const NOW = new Date('2026-10-06T12:00:00Z');
const DAY = { from: '2026-10-05T12:00:00.000Z' };

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(NOW);
  // ...existing mock resets/resolutions stay as they are
});

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});
```

Update every existing assertion that passes `undefined` as the RANGE argument for global scope to pass `DAY` instead, e.g.:

```ts
    expect(fetchNetflowExplorer).toHaveBeenCalledWith(undefined, DAY, undefined);
    expect(fetchGlobalNetflow).toHaveBeenCalledWith(DAY, undefined);
```

and `toHaveBeenLastCalledWith(undefined, DAY, filters)` / `toHaveBeenLastCalledWith(DAY, filters)` in the filter tests. Then add:

```ts
  it('renders the aggregate sections while the flows request is still pending', async () => {
    fetchGlobalNetflow.mockReturnValue(new Promise(() => {}));
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('Workflows');
    expect(screen.getByText('Flows')).toBeInTheDocument();
    expect(screen.getByText(/loading network flows/i)).toBeInTheDocument();
  });

  it('renders the flows table while the explorer request is still pending', async () => {
    fetchNetflowExplorer.mockReturnValue(new Promise(() => {}));
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('api.anthropic.com');
    expect(screen.queryByText('Workflows')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '24h' })).toBeInTheDocument();
    expect(screen.getByText(/loading network flows/i)).toBeInTheDocument();
  });

  it('an explorer error leaves the picker and the table usable', async () => {
    fetchNetflowExplorer.mockRejectedValue(new Error('explorer down'));
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('explorer down');
    await screen.findByText('api.anthropic.com');
    expect(screen.getByRole('button', { name: '24h' })).toBeInTheDocument();
  });

  it('a flows error leaves the aggregates on screen', async () => {
    fetchGlobalNetflow.mockRejectedValue(new Error('flows down'));
    render(<NetflowExplorer scope="global" />);
    await screen.findByText('flows down');
    expect(screen.getByText('Workflows')).toBeInTheDocument();
  });

  it('run scope without a span still opens on All', async () => {
    render(<NetflowExplorer scope="run" runId="run-1" />);
    await waitFor(() =>
      expect(fetchNetflowExplorer).toHaveBeenCalledWith('run:run-1', undefined, undefined),
    );
    expect(fetchRunNetflow).toHaveBeenCalledWith('run-1', undefined, undefined);
  });
```

(The "table while explorer pending" test relies on `api.anthropic.com` appearing only in the table when the explorer has not loaded — the explorer fixture's lanes/org cards are not rendered then.)

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/netflow/explorer/NetflowExplorer.test.tsx`
Expected: FAIL — default window is `undefined`, and pending-section tests find only "Loading network flows…".

- [ ] **Step 3: Export `presetValue`** from `TimeRangePicker.tsx` (below `presetFrom`) and use it in `selectPreset`:

```ts
/** The applied value for a relative preset, resolved against `now` —
 *  exactly what clicking that preset produces. */
export function presetValue(
  preset: Exclude<TimeRangePreset, 'custom'>,
  now: Date,
): TimeRangeValue {
  const from = presetFrom(preset, now);
  return from !== undefined ? { preset, from } : { preset };
}
```

```ts
  function selectPreset(preset: TimeRangePreset) {
    if (preset === 'custom') {
      setCustomEditorOpen(true);
      return;
    }
    onChange(presetValue(preset, now()));
  }
```

- [ ] **Step 4: Rework `NetflowExplorer.tsx`.**

Import `presetValue` alongside `toNetflowRange`. Initial range:

```ts
  const [range, setRange] = useState<TimeRangeValue>(() =>
    initialWindow && (initialWindow.from || initialWindow.to)
      ? { preset: 'custom', from: initialWindow.from, to: initialWindow.to }
      : scope === 'run'
        ? { preset: 'all' }
        : presetValue('24h', new Date()),
  );
```

Replace the `error` / `refreshing` state and the single fetch effect with per-request state and two effects:

```ts
  const [explorerError, setExplorerError] = useState<string | null>(null);
  const [flowsError, setFlowsError] = useState<string | null>(null);
  // True while a refetch is in flight OVER existing data, per request.
  const [explorerRefreshing, setExplorerRefreshing] = useState(false);
  const [flowsRefreshing, setFlowsRefreshing] = useState(false);

  // The aggregates and the flows list load independently: each section
  // renders as soon as its own response arrives (the aggregates are a few
  // KB; the flows list can be large). Previous data stays on screen during
  // a refetch, marked by "Updating…".
  useEffect(() => {
    let cancelled = false;
    setExplorerError(null);
    setExplorerRefreshing(true);
    const f = filtersAreEmpty(filters) ? undefined : filters;
    fetchNetflowExplorer(scopeParam, toNetflowRange(range), f)
      .then((e) => {
        if (cancelled) return;
        setExplorer(e);
        setExplorerRefreshing(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setExplorerRefreshing(false);
        setExplorerError(e instanceof Error ? e.message : 'Failed to load network flows');
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope, projectId, runId, range, filters]);

  useEffect(() => {
    let cancelled = false;
    setFlowsError(null);
    setFlowsRefreshing(true);
    const q = toNetflowRange(range);
    const f = filtersAreEmpty(filters) ? undefined : filters;
    const flowsFetch =
      scope === 'run' && runId
        ? fetchRunNetflow(runId, q, f)
        : scope === 'project' && projectId
          ? fetchProjectNetflow(projectId, q, f)
          : fetchGlobalNetflow(q, f);
    flowsFetch
      .then((fl) => {
        if (cancelled) return;
        setFlows(fl);
        setFlowsRefreshing(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setFlowsRefreshing(false);
        setFlowsError(e instanceof Error ? e.message : 'Failed to load network flows');
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope, projectId, runId, range, filters]);
```

Keep the existing doc comment at the top of the file but update its "One fetch effect, one Promise.all, one error surface" sentence to describe the two independent effects. Replace the `if (error) …` and `if (explorer === null || flows === null) …` early returns, and the render, with:

```tsx
  const selectedTopo: Record<TopoDim, string[]> = {
    wf: filters.workflows,
    or: filters.origins,
    org: filters.orgs,
  };
  const filtersActive = !filtersAreEmpty(filters);
  const updating =
    (explorerRefreshing && explorer !== null) || (flowsRefreshing && flows !== null);

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-end gap-3">
        {updating && (
          <p role="status" className="text-note text-ink-mute">
            Updating…
          </p>
        )}
        {explorer && flows && (
          <CoveragePopover
            scope={scope}
            droppedTotal={explorer.dropped_total}
            capture={flows.capture}
          />
        )}
      </div>
      {explorerError || explorer === null ? (
        // The picker stays reachable before the aggregates arrive and on
        // their error — changing the window is the retry affordance.
        <div className="space-y-3">
          <TimeRangePicker value={range} onChange={changeRange} />
          {explorerError ? (
            <p className="text-sm text-err">{explorerError}</p>
          ) : (
            <p className="text-sm text-ink-dim">Loading network flows…</p>
          )}
        </div>
      ) : (
        <>
          <ActivityStrip
            histogram={explorer.histogram}
            range={range}
            onRangeChange={changeRange}
            onZoom={zoomTo}
            appliedWindow={explorer.window}
          />
          <KpiStrip kpis={explorer.kpis} />
          {/* …the existing visualization panel, OrgCards and FilterChips
              blocks, unchanged… */}
        </>
      )}
      {flowsError ? (
        <p className="text-sm text-err">{flowsError}</p>
      ) : flows === null ? (
        <p className="text-sm text-ink-dim">Loading network flows…</p>
      ) : (
        <NetflowTable
          flows={flows.flows}
          droppedTotal={flows.dropped_total}
          incomplete={flows.incomplete}
          asnLoaded={flows.asn_loaded}
          scope={scope}
          appliedWindow={flows.window}
          showAttribution={scope !== 'run'}
          onRowClick={setSelectedFlow}
          filtersActive={filtersActive}
        />
      )}
      <FlowDetailPanel flow={selectedFlow} scope={scope} onClose={() => setSelectedFlow(null)} />
    </div>
  );
```

Move the existing panel/`OrgCards`/`FilterChips` JSX verbatim into the fragment where the comment sits (they read `explorer`, which is non-null there). `clearAll` and the chip's `onClearWindow` stay `{ preset: 'all' }` (unchanged semantics). `orgLabels` already handles `explorer === null`.

- [ ] **Step 5: Run the web tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/netflow src/pages/Netflow.test.tsx src/components/project src/pages/RunDetail.netflow.test.tsx`
Expected: PASS. If `Netflow.test.tsx` / `ProjectNetworkTab.test.tsx` assert the old `undefined` default window, update only that argument (pin the clock the same way) — no other assertion changes.

- [ ] **Step 6: Typecheck, lint, commit**

```bash
cd crates/rupu-cp/web && npx tsc --noEmit && npx eslint src/components/netflow src/pages/Netflow.test.tsx src/components/project
git add src/components/netflow/TimeRangePicker.tsx src/components/netflow/explorer/NetflowExplorer.tsx src/components/netflow/explorer/NetflowExplorer.test.tsx src/pages/Netflow.test.tsx src/components/project/ProjectNetworkTab.test.tsx
git commit -m "perf(cp-web): Network sections load independently; global/project open on 24h"
```

(Only `git add` the test files you actually changed.)

---

### Task 10: Web — virtualized `SortableTable` rendering for `NetflowTable`

**Files:**
- Create: `crates/rupu-cp/web/src/components/lists/useWindowVirtualRows.ts`
- Modify: `crates/rupu-cp/web/src/components/lists/SortableTable.tsx`
- Modify: `crates/rupu-cp/web/src/components/netflow/NetflowTable.tsx`
- Modify: `crates/rupu-cp/web/src/components/lists/SortableTable.test.tsx`

**Interfaces:**
- Produces: `export const ESTIMATED_ROW_PX = 41;`, `export const OVERSCAN_ROWS = 20;`, `export function useWindowVirtualRows(keys: string[], enabled: boolean): { bodyRef: React.RefObject<HTMLTableSectionElement | null>; start: number; end: number; topPx: number; bottomPx: number; measure: (key: string) => (el: HTMLTableRowElement | null) => void }`; `SortableTable` prop `virtualize?: { threshold: number }`.

- [ ] **Step 1: Write the failing tests** — add to `SortableTable.test.tsx` (reuse its existing imports; add `vi` if missing):

```tsx
describe('SortableTable virtualization', () => {
  type Row = { id: string; n: number };
  const rows = (count: number): Row[] =>
    Array.from({ length: count }, (_, i) => ({ id: `r${i}`, n: count - i }));
  const columns = [
    { key: 'id', header: 'Id', render: (r: Row) => r.id },
    { key: 'n', header: 'N', sortable: true, sortValue: (r: Row) => r.n, render: (r: Row) => String(r.n) },
  ];
  const mounted = (c: HTMLElement) =>
    Array.from(c.querySelectorAll('tbody tr:not([aria-hidden])')) as HTMLTableRowElement[];
  const spacers = (c: HTMLElement) =>
    Array.from(c.querySelectorAll('tbody tr[aria-hidden]')) as HTMLTableRowElement[];

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('at or below the threshold renders every row exactly as before', () => {
    const { container } = render(
      <SortableTable columns={columns} rows={rows(10)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
    );
    expect(mounted(container)).toHaveLength(10);
    expect(spacers(container)).toHaveLength(0);
  });

  it('above the threshold mounts only the viewport plus overscan, with a bottom spacer', () => {
    const { container } = render(
      <SortableTable columns={columns} rows={rows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
    );
    // jsdom: no layout, so rows use the estimate; viewport = innerHeight (768).
    const visible = Math.ceil(window.innerHeight / ESTIMATED_ROW_PX);
    expect(mounted(container)).toHaveLength(visible + OVERSCAN_ROWS);
    const [bottom] = spacers(container);
    expect(bottom.style.height).toBe(`${(1000 - visible - OVERSCAN_ROWS) * ESTIMATED_ROW_PX}px`);
  });

  it('scrolling moves the window and adds a top spacer', async () => {
    const { container } = render(
      <SortableTable columns={columns} rows={rows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
    );
    vi.spyOn(HTMLTableSectionElement.prototype, 'getBoundingClientRect').mockReturnValue({
      top: -100 * ESTIMATED_ROW_PX, bottom: 0, left: 0, right: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({}),
    } as DOMRect);
    await act(async () => {
      document.dispatchEvent(new Event('scroll'));
    });
    const first = mounted(container)[0];
    expect(first.textContent).toContain(`r${100 - OVERSCAN_ROWS}`);
    expect(spacers(container)[0].style.height).toBe(`${(100 - OVERSCAN_ROWS) * ESTIMATED_ROW_PX}px`);
  });

  it('sorting still orders the full row set', () => {
    const { container } = render(
      <SortableTable columns={columns} rows={rows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Sort by N' }));
    expect(mounted(container)[0].textContent).toContain('r999'); // n = 1, ascending
  });
});
```

Import `ESTIMATED_ROW_PX` and `OVERSCAN_ROWS` from `./useWindowVirtualRows`, and `act` from `@testing-library/react`.

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/lists/SortableTable.test.tsx`
Expected: FAIL — module `./useWindowVirtualRows` not found; `virtualize` prop unknown.

- [ ] **Step 3: Implement `useWindowVirtualRows.ts`:**

```ts
// Window virtualization for long tables that scroll with the page.
//
// Only the rows intersecting the viewport (plus an overscan on each side)
// are mounted; spacer rows above and below stand in for the rest, sized
// from each row's measured height (estimated until measured), so page
// height, scrollbar and scroll position match a fully rendered table.
// Scroll is observed on the document in the capture phase, which catches
// whichever ancestor actually scrolls.

import { useEffect, useRef, useState } from 'react';

/** Row height assumed before a row has been measured (and in jsdom). */
export const ESTIMATED_ROW_PX = 41;
/** Rows mounted beyond the viewport on each side. */
export const OVERSCAN_ROWS = 20;

export function useWindowVirtualRows(keys: string[], enabled: boolean) {
  const bodyRef = useRef<HTMLTableSectionElement | null>(null);
  const heights = useRef(new Map<string, number>());
  const [viewport, setViewport] = useState({ top: 0, height: 0 });

  useEffect(() => {
    if (!enabled) return;
    const update = () => {
      const el = bodyRef.current;
      if (!el) return;
      const top = -el.getBoundingClientRect().top;
      const height = window.innerHeight;
      setViewport((v) => (v.top === top && v.height === height ? v : { top, height }));
    };
    update();
    document.addEventListener('scroll', update, { capture: true, passive: true });
    window.addEventListener('resize', update);
    return () => {
      document.removeEventListener('scroll', update, { capture: true });
      window.removeEventListener('resize', update);
    };
  }, [enabled]);

  const measure = (key: string) => (el: HTMLTableRowElement | null) => {
    if (!el) return;
    const h = el.getBoundingClientRect().height;
    if (h > 0) heights.current.set(key, h);
  };

  if (!enabled) {
    return { bodyRef, start: 0, end: keys.length, topPx: 0, bottomPx: 0, measure };
  }

  const h = (k: string) => heights.current.get(k) ?? ESTIMATED_ROW_PX;
  const visTop = Math.max(0, viewport.top);
  const visBottom = viewport.top + viewport.height;
  let first = 0;
  let acc = 0;
  while (first < keys.length && acc + h(keys[first]) <= visTop) {
    acc += h(keys[first]);
    first++;
  }
  let last = first;
  let accEnd = acc;
  while (last < keys.length && accEnd < visBottom) {
    accEnd += h(keys[last]);
    last++;
  }
  const start = Math.max(0, first - OVERSCAN_ROWS);
  const end = Math.min(keys.length, last + OVERSCAN_ROWS);
  let topPx = 0;
  for (let i = 0; i < start; i++) topPx += h(keys[i]);
  let bottomPx = 0;
  for (let i = end; i < keys.length; i++) bottomPx += h(keys[i]);
  return { bodyRef, start, end, topPx, bottomPx, measure };
}
```

Check the arithmetic against the tests: with `top = 0`, `height = 768`, estimate 41 → `first = 0`, `last = ceil(768/41) = 19`, `start = 0`, `end = 39` → 39 mounted rows (= visible + overscan), bottom spacer `(1000 - 39) * 41`. With `top = 4100` → `first = 100`, `start = 80`, top spacer `80 * 41`. Note `viewport.height` starts at 0 until the effect runs; the effect runs synchronously in the test's `render` (inside `act`), so the first assertion sees the updated state.

- [ ] **Step 4: Wire it into `SortableTable.tsx`.** Add the prop (with doc) to the props type and destructuring:

```ts
  /** Mount only the rows near the viewport once the table has more than
   *  `threshold` rows (window virtualization; sorting still covers every
   *  row). At or below the threshold rendering is unchanged. For tables
   *  without `renderDetail` (detail-row heights are not measured). */
  virtualize?: { threshold: number };
```

After `sorted` is computed:

```ts
  const isVirtual = virtualize !== undefined && sorted.length > virtualize.threshold;
  const virtualKeys = isVirtual ? sorted.map(rowKey) : [];
  const win = useWindowVirtualRows(virtualKeys, isVirtual);
  const visibleRows = isVirtual ? sorted.slice(win.start, win.end) : sorted;
```

(The hook is called unconditionally — `isVirtual` only toggles its behaviour.) In the JSX: put `ref={win.bodyRef}` on `<tbody>`; render `visibleRows.map(...)` instead of `sorted.map(...)`; on the main row `<tr>` add `ref={isVirtual ? win.measure(key) : undefined}`; and around the mapped rows:

```tsx
          {isVirtual && win.topPx > 0 && (
            <tr aria-hidden="true" style={{ height: win.topPx }}>
              <td colSpan={totalCols} />
            </tr>
          )}
          {/* visibleRows.map(...) */}
          {isVirtual && win.bottomPx > 0 && (
            <tr aria-hidden="true" style={{ height: win.bottomPx }}>
              <td colSpan={totalCols} />
            </tr>
          )}
```

Update the file's header comment ("Sorting is purely client-side over the `rows` prop…") with one sentence on `virtualize`.

In `NetflowTable.tsx`, pass `virtualize={{ threshold: 500 }}` to `SortableTable`.

- [ ] **Step 5: Run the web tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/lists src/components/netflow`
Expected: PASS — new virtualization tests plus every existing `SortableTable` / `NetflowTable` test unchanged.

- [ ] **Step 6: Typecheck, lint, commit**

```bash
cd crates/rupu-cp/web && npx tsc --noEmit && npx eslint src/components/lists src/components/netflow
git add src/components/lists/useWindowVirtualRows.ts src/components/lists/SortableTable.tsx src/components/lists/SortableTable.test.tsx src/components/netflow/NetflowTable.tsx
git commit -m "perf(cp-web): virtualize the netflow table above 500 rows (same rendering)"
```

---

### Task 11: Live verification on matt's Mac

**Files:** none (results go in the PR description).

- [ ] **Step 1: Build the web UI and an install-able binary** (the binary embeds `web/dist`):

```bash
make cp-web
```

then, as its own command:

```bash
cargo build --release -p rupu-cli
```

- [ ] **Step 2: Run this branch's CP on a spare port against the real `~/.rupu`** (do not touch the installed `:7878` daemon):

```bash
./target/release/rupu cp serve --bind 127.0.0.1:7999
```

(run in the background; wait for the `netflow index warmed` log line and record its `elapsed_ms`, `files`, `flows`, `tier1_bytes`, `tier2_bytes`.)

- [ ] **Step 3: Measure** the spec §1 table against `:7999` (and re-run against `:7878` for the baseline in the same session):

```bash
H=$(date -u -v-1H +%Y-%m-%dT%H:%M:%SZ); D=$(date -u -v-1d +%Y-%m-%dT%H:%M:%SZ)
for u in "/api/netflow" "/api/netflow/explorer" "/api/netflow?from=$D" "/api/netflow/explorer?from=$D"; do curl -s -o /dev/null -w "$u: %{time_total}s %{size_download}B\n" "http://127.0.0.1:7999$u"; done
curl -s http://127.0.0.1:7999/api/netflow/index
```

Expected: warm `/api/netflow` well under the 0.41 s baseline; explorer under 0.47 s; the status shows resident memory under budget.

- [ ] **Step 4: Check rendering** in the browser pane at `http://127.0.0.1:7999` → Network: picker shows `24h`, aggregates appear before the table, the table renders, and switching to `All` shows the full table scrolling smoothly. Compare against `:7878` side by side — the same sections, labels and numbers (for the same window). Take one screenshot of each for the PR.

- [ ] **Step 5: Stop the `:7999` server**, then hand off: the installed `:7878` daemon only picks this up after a release + `rupu update` + restart.

---

## Self-review notes

- Spec coverage: §3.1 → Task 1; §3.2 tier 2 → Task 2, tier 1 + entry → Task 4; §3.3 → Task 4 (+ `retain_only` Task 5/7/8); §3.4 → Task 5; §3.5 file selection/flows list → Task 6, explorer → Tasks 3 + 7, status → Task 8, budget source → Task 6 (`index_for`); §3.6 → Tasks 6–7 equivalence tests; §4.1 → Task 9; §4.2 → Task 9; §4.3 → Task 10; §5 → tests in every task + Task 11; §6 docs → Task 8.
- Run scope keeps `build_explorer_response` over the full run read (via the index); it is small and merges transcript flows, as the spec allows.
- The equivalence tests construct the old answer with `DirectReader`, i.e. exactly today's per-file reads, so "same values" is checked against the real current behaviour, not a re-implementation.
