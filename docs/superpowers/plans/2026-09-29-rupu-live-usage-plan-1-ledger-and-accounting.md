# Live usage — Plan 1: usage ledger + correct accounting — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every token an agent spends inside a run is recorded per LLM call in a
per-run ledger. It is folded by one shared function that every CP endpoint and
every CLI view uses, so in-flight, retried, cancelled, dispatched, parallel and
compaction usage are all counted exactly once.

**Architecture:**
- The agent loop gains an `on_usage` hook. The orchestrator attaches a hook at
  its single launch choke point (`dispatch_one`), and the CLI `dispatch_agent`
  dispatcher attaches one to its children. Both append a row to
  `<runs>/<id>/usage.jsonl`.
- `rupu-cp::usage_index` folds ledger rows plus a fallback over "known
  transcripts" without ledger rows, incrementally and cached process-wide.
- `rupu-cp::usage`'s existing entry points (`summarize_run`, `run_metrics`,
  `run_transcript_paths`) are re-implemented on top of it, so ~20 call sites
  keep their signatures.
- The CLI (which already depends on `rupu-cp`) uses the same functions.

**Tech Stack:** Rust 2021 workspace (tokio, serde, chrono, ulid), crates
`rupu-transcript`, `rupu-agent`, `rupu-orchestrator`, `rupu-cp`, `rupu-cli`.

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-live-usage-ledger-design.md`
(§3, §4, §5.1, §5.2, §8). Read it before starting any task.

## Global Constraints

- Workspace deps only — never add a version to a crate `Cargo.toml`; pin in the
  root `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- Errors: `thiserror` in libraries, `anyhow` in `rupu-cli`.
- Usage accounting must **never fail a run**: every ledger write error is
  `tracing::warn!`-logged and swallowed.
- Ledger file: `<RunStore root>/<run_id>/usage.jsonl`, one JSON object per line,
  `"v": 1`, dedup key `"id"` (ULID).
- Never run package-wide `cargo fmt`. Main is fmt-dirty under the pinned
  toolchain; format only the files you touched: `rustfmt --edition 2021 <file>`.
- Never use bare `git stash` / `git stash pop` (the stash stack is shared with
  other sessions). Use WIP commits.
- Public repo: test fixtures are invented from scratch, never adapted from real
  run data.
- Baseline first: before Task 1, run
  `cargo test -p rupu-transcript -p rupu-agent -p rupu-orchestrator -p rupu-cp -p rupu-cli 2>&1 | grep -E '^test result|FAILED|panicked' > /tmp/baseline.txt`
  (use the scratchpad dir, not /tmp, if one is available). Record any
  pre-existing failures so they are not attributed to this work.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-transcript/src/cursor.rs` (new) | `JsonlCursor` — incremental offset-based JSONL line reader (partial-line hold-back, UTF-8-safe, reset on shrink/replace); `transcript_key(path)` |
| `crates/rupu-transcript/src/event.rs` | `Event::Usage` gains `purpose: Option<String>` (`"compaction"`) |
| `crates/rupu-agent/src/runner.rs` | `UsageKind`, `UsageTurn`, `OnUsageCallback`; `AgentRunOpts.on_usage`; hook call after every `Usage` write; compaction usage; `RunResult.total_tokens_cached` |
| `crates/rupu-orchestrator/src/usage_ledger.rs` (new) | `LedgerRow`, `LedgerKind`, `LedgerTag`, `UnitTokenCounters`, `UsageLedger` (writer + `hook()`) |
| `crates/rupu-orchestrator/src/runs.rs` | `RunStore::usage_ledger_path`, `KnownTranscript`, `known_transcripts_from_event_line`, `known_transcripts_from_step_result_line`, `RunStore::known_transcripts` |
| `crates/rupu-orchestrator/src/runner.rs` | ledger hook at all six `dispatch_one` sites; `run_parallel_step` gets `workflow_run_id`; honest `UnitCompleted` tokens |
| `crates/rupu-cli/src/cmd/dispatch.rs` | `CliAgentDispatcher::with_usage_ledger`; child hook |
| `crates/rupu-cp/src/usage_index.rs` (new) | `RunUsage`, `Tokens`, fold state, process-global `UsageIndex` |
| `crates/rupu-cp/src/usage.rs` | `run_usage`, `transcripts_usage`; `summarize_run`/`run_metrics`/`run_transcript_paths` re-implemented; `UsageSummary.partial`; `TurnPoint` series from fold |
| `crates/rupu-cp/src/api/{usage,agents,runs,run_streams,sessions,graph}.rs` | consumers switched to the fold; aggregates include standalone + session transcripts |
| `crates/rupu-cli/src/output/live_run.rs` | tokens/cost from the fold, not the focused transcript |
| `crates/rupu-cli/src/cmd/{run,workflow,session,usage_report,autoflow}.rs`, `output/{printer,workflow_printer}.rs` | live `rupu run` usage line; plain-printer cost; session cached sum; one-shot callers use the fold |

---

### Task 1: `JsonlCursor` + `transcript_key` in `rupu-transcript`

**Files:**
- Create: `crates/rupu-transcript/src/cursor.rs`
- Modify: `crates/rupu-transcript/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `cursor.rs`

**Interfaces:**
- Produces:
  - `pub struct JsonlCursor` (`Default`, `Clone`, `Debug`) with
    `pub fn new() -> Self`, `pub fn offset(&self) -> u64` and
    `pub fn drain_with(&mut self, path: &Path, on_reset: impl FnOnce(), on_line: impl FnMut(&str)) -> std::io::Result<DrainStats>`.
  - `pub struct DrainStats { pub reset: bool, pub lines: usize, pub bytes: u64 }`.
  - `pub fn transcript_key(path: &Path) -> Option<String>`.
  - All three are re-exported from `rupu_transcript`.

- [ ] **Step 1: Write the failing tests** — create `crates/rupu-transcript/src/cursor.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn collect(c: &mut JsonlCursor, p: &Path) -> (Vec<String>, DrainStats) {
        let mut out = Vec::new();
        let st = c.drain_with(p, || {}, |l| out.push(l.to_string())).unwrap();
        (out, st)
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = JsonlCursor::new();
        let (lines, st) = collect(&mut c, &dir.path().join("nope.jsonl"));
        assert!(lines.is_empty());
        assert!(!st.reset);
        assert_eq!(c.offset(), 0);
    }

    #[test]
    fn reads_only_new_complete_lines_and_holds_back_partial() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        write!(f, "{{\"a\":1}}\n{{\"b\":").unwrap();
        f.flush().unwrap();
        let mut c = JsonlCursor::new();
        let (lines, _) = collect(&mut c, &p);
        assert_eq!(lines, vec!["{\"a\":1}"]);
        assert_eq!(c.offset(), 8);
        write!(f, "2}}\n").unwrap();
        f.flush().unwrap();
        let (lines, _) = collect(&mut c, &p);
        assert_eq!(lines, vec!["{\"b\":2}"]);
        let (lines, _) = collect(&mut c, &p);
        assert!(lines.is_empty(), "no new bytes → no lines");
    }

    #[test]
    fn utf8_split_across_reads_is_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("u.jsonl");
        let line = "{\"t\":\"héllo✓\"}\n";
        let bytes = line.as_bytes();
        // Write up to the middle of the multi-byte '✓' (3 bytes).
        let cut = line.find('✓').unwrap() + 1;
        std::fs::write(&p, &bytes[..cut]).unwrap();
        let mut c = JsonlCursor::new();
        let (lines, _) = collect(&mut c, &p);
        assert!(lines.is_empty());
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(&bytes[cut..]).unwrap();
        let (lines, _) = collect(&mut c, &p);
        assert_eq!(lines, vec![line.trim_end()]);
    }

    #[test]
    fn shrink_resets_and_rereads_from_start() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, "{\"a\":1}\n{\"a\":2}\n").unwrap();
        let mut c = JsonlCursor::new();
        let _ = collect(&mut c, &p);
        std::fs::write(&p, "{\"z\":9}\n").unwrap();
        let mut resets = 0;
        let mut lines = Vec::new();
        let st = c
            .drain_with(&p, || resets += 1, |l| lines.push(l.to_string()))
            .unwrap();
        assert!(st.reset);
        assert_eq!(resets, 1);
        assert_eq!(lines, vec!["{\"z\":9}"]);
    }

    #[cfg(unix)]
    #[test]
    fn replaced_file_same_length_resets() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.jsonl");
        std::fs::write(&p, "{\"a\":1}\n").unwrap();
        let mut c = JsonlCursor::new();
        let _ = collect(&mut c, &p);
        let tmp = dir.path().join("r.tmp");
        std::fs::write(&tmp, "{\"b\":2}\n{\"c\":3}\n").unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        let (lines, st) = collect(&mut c, &p);
        assert!(st.reset);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn blank_lines_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("b.jsonl");
        std::fs::write(&p, "\n{\"a\":1}\n   \n").unwrap();
        let mut c = JsonlCursor::new();
        let (lines, _) = collect(&mut c, &p);
        assert_eq!(lines, vec!["{\"a\":1}"]);
    }

    #[test]
    fn large_file_streams_in_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.jsonl");
        let mut body = String::new();
        for i in 0..50_000 {
            body.push_str(&format!("{{\"i\":{i},\"pad\":\"{}\"}}\n", "x".repeat(200)));
        }
        std::fs::write(&p, &body).unwrap();
        let mut c = JsonlCursor::new();
        let (lines, st) = collect(&mut c, &p);
        assert_eq!(lines.len(), 50_000);
        assert_eq!(st.bytes, body.len() as u64);
    }

    #[test]
    fn transcript_key_flat_and_nested() {
        assert_eq!(
            transcript_key(Path::new("/g/transcripts/run_01ABC.jsonl")).as_deref(),
            Some("run_01ABC")
        );
        assert_eq!(
            transcript_key(Path::new("/g/runs/run_P/sub/sub_01X/transcript.jsonl")).as_deref(),
            Some("sub_01X")
        );
        assert_eq!(transcript_key(Path::new("/")), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-transcript cursor`
Expected: compile error (`JsonlCursor` / `transcript_key` not found). If
`tempfile` is not a dev-dependency of `rupu-transcript`, add
`tempfile = { workspace = true }` under `[dev-dependencies]` in
`crates/rupu-transcript/Cargo.toml`, confirming the root `Cargo.toml` pins
`tempfile` in `[workspace.dependencies]` (it does; other crates use it).

- [ ] **Step 3: Implement** — put this *above* the test module in `cursor.rs`:

```rust
//! Incremental, offset-based JSONL line reader shared by every live consumer
//! (CP usage index, CP transcript/run-event tailers, CLI live view).
//!
//! Contract (spec 2026-09-29 §4.4):
//! - reads only bytes appended since the last call (seek, never re-read);
//! - consumes up to the last `\n`; a partial trailing line is held back until
//!   its newline lands (never skipped, never parsed half-written);
//! - decodes UTF-8 per whole line (a read boundary inside a multi-byte char
//!   can't drop data);
//! - a file that shrank, or was replaced by a different inode, resets to
//!   offset 0 and calls `on_reset` before any line of the new content.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const CHUNK: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdent {
    dev: u64,
    ino: u64,
}

fn ident(meta: &std::fs::Metadata) -> Option<FileIdent> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileIdent {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// Incremental reader state for one file. Cheap to clone; holds no fd.
#[derive(Debug, Clone, Default)]
pub struct JsonlCursor {
    offset: u64,
    ident: Option<FileIdent>,
}

/// What one [`JsonlCursor::drain_with`] call did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainStats {
    /// The file shrank or was replaced; the cursor restarted at 0.
    pub reset: bool,
    /// Non-blank lines delivered to `on_line`.
    pub lines: usize,
    /// Bytes consumed (the offset advance, including newlines).
    pub bytes: u64,
}

impl JsonlCursor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Byte offset of the first unconsumed byte.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Deliver every complete, non-blank line appended since the last call.
    /// A missing file is "no data yet" (`Ok`, nothing delivered, no reset).
    pub fn drain_with(
        &mut self,
        path: &Path,
        on_reset: impl FnOnce(),
        mut on_line: impl FnMut(&str),
    ) -> std::io::Result<DrainStats> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(DrainStats::default())
            }
            Err(e) => return Err(e),
        };
        let mut stats = DrainStats::default();
        let id = ident(&meta);
        let replaced = matches!((self.ident, id), (Some(a), Some(b)) if a != b);
        if meta.len() < self.offset || replaced {
            self.offset = 0;
            stats.reset = true;
            on_reset();
        }
        self.ident = id;
        if meta.len() == self.offset {
            return Ok(stats);
        }
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut carry: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            carry.extend_from_slice(&buf[..n]);
            let Some(last_nl) = carry.iter().rposition(|b| *b == b'\n') else {
                continue;
            };
            for raw in carry[..last_nl].split(|b| *b == b'\n') {
                let Ok(line) = std::str::from_utf8(raw) else {
                    continue;
                };
                let line = line.trim_end_matches('\r');
                if line.trim().is_empty() {
                    continue;
                }
                stats.lines += 1;
                on_line(line);
            }
            let consumed = (last_nl + 1) as u64;
            self.offset += consumed;
            stats.bytes += consumed;
            carry.drain(..=last_nl);
        }
        Ok(stats)
    }
}

/// Identity of a transcript file: its stem (`run_<ULID>` for the flat layout),
/// or the parent directory name for the nested sub-run layout
/// `…/<sub_id>/transcript.jsonl`. Ledger rows and known-transcript sets are
/// both keyed by this, so they can never disagree on identity.
pub fn transcript_key(path: &Path) -> Option<String> {
    match path.file_stem().and_then(|s| s.to_str()) {
        Some("transcript") => path
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|s| s.to_str())
            .map(str::to_string),
        Some(stem) if !stem.is_empty() => Some(stem.to_string()),
        _ => None,
    }
}
```

In `crates/rupu-transcript/src/lib.rs` add `pub mod cursor;` and
`pub use cursor::{transcript_key, DrainStats, JsonlCursor};`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-transcript cursor`
Expected: all 8 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-transcript
git commit -m "feat(transcript): JsonlCursor incremental reader + transcript_key"
```

---

### Task 2: `Event::Usage.purpose` (compaction marker)

**Files:**
- Modify: `crates/rupu-transcript/src/event.rs` (`Usage` variant, ~line 135)
- Modify: every struct-literal construction of `Event::Usage { … }` in the
  workspace (≈43 hits; `grep -rn 'Event::Usage {' --include='*.rs' crates`).
  Pattern matches ending in `..` need no change.
- Test: `crates/rupu-transcript/tests/usage_event.rs`

**Interfaces:**
- Produces:
  - `Event::Usage { provider, model, served_model, input_tokens, output_tokens, cached_tokens, purpose: Option<String> }`.
  - `purpose` is `None` for a normal turn and `Some("compaction")` for a
    compaction summariser call.
  - Serde: `#[serde(default, skip_serializing_if = "Option::is_none")]`.

- [ ] **Step 1: Write the failing test** — append to `crates/rupu-transcript/tests/usage_event.rs`:

```rust
#[test]
fn usage_purpose_round_trips_and_defaults_to_none() {
    let with = rupu_transcript::Event::Usage {
        provider: "anthropic".into(),
        model: "m".into(),
        served_model: None,
        input_tokens: 10,
        output_tokens: 2,
        cached_tokens: 0,
        purpose: Some("compaction".into()),
    };
    let json = serde_json::to_string(&with).unwrap();
    assert!(json.contains("\"purpose\":\"compaction\""));
    let back: rupu_transcript::Event = serde_json::from_str(&json).unwrap();
    assert_eq!(back, with);

    // Older lines (no purpose) still parse, as None, and don't serialize it.
    let old = r#"{"type":"usage","data":{"provider":"p","model":"m","input_tokens":1,"output_tokens":1,"cached_tokens":0}}"#;
    let ev: rupu_transcript::Event = serde_json::from_str(old).unwrap();
    match &ev {
        rupu_transcript::Event::Usage { purpose, .. } => assert!(purpose.is_none()),
        _ => panic!("expected usage"),
    }
    assert!(!serde_json::to_string(&ev).unwrap().contains("purpose"));
}
```

(If `Event` does not derive `PartialEq`, compare `serde_json::to_value` of
both sides instead of `assert_eq!(back, with)`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-transcript --test usage_event`
Expected: compile error — no field `purpose`.

- [ ] **Step 3: Implement** — in `event.rs` add the field last in the `Usage` variant:

```rust
        #[serde(default)]
        cached_tokens: u32,
        /// Why this call happened, when it is not a normal agent turn.
        /// `Some("compaction")` = the context-compaction summariser call —
        /// real, billed spend that is NOT an agent turn (turn counters skip
        /// it; token/cost totals include it). `None` = a normal turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<String>,
```

Then add `purpose: None,` to every struct-literal construction the compiler
flags: `cargo build --workspace --all-targets 2>&1 | grep -A3 'missing field .purpose.'`,
iterating until clean.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-transcript && cargo build --workspace --all-targets`
Expected: PASS; the workspace builds.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "feat(transcript): Usage.purpose marks compaction spend"
```

---

### Task 3: Agent-loop `on_usage` hook, compaction usage, `total_tokens_cached`

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs`
  - types near line 101;
  - `AgentRunOpts` (~595-724);
  - `CompactionOutcome` (~303) and `compact_messages` (~317-420);
  - the per-turn `Usage` write (~1298-1316);
  - the compaction call site (~455-500);
  - `RunResult` (~726).
- Modify: `crates/rupu-agent/src/lib.rs` — re-export the new types next to
  `OnToolCallCallback`.
- Modify: every `AgentRunOpts { … }` literal in the workspace (≈60; they all
  set `on_stream_event:`). Add `on_usage: None,` right after it.
- Modify: every `RunResult { … }` literal — add `total_tokens_cached: …`.
- Test: `crates/rupu-agent/tests/runner_usage_hook.rs` (new)

**Interfaces:**
- Produces (exported from `rupu_agent`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageKind { Turn, Compaction }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageTurn {
    pub kind: UsageKind,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    /// Billable output (output + reasoning) — identical to the transcript.
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

pub type OnUsageCallback = Arc<dyn Fn(&UsageTurn) + Send + Sync>;
```

  - `AgentRunOpts.on_usage: Option<OnUsageCallback>`.
  - `RunResult.total_tokens_cached: u64`.
  - `CompactionOutcome.usage: rupu_providers::Usage`.

- [ ] **Step 1: Write the failing test** — `crates/rupu-agent/tests/runner_usage_hook.rs`.
  Model it on `crates/rupu-agent/tests/runner_basic.rs`: copy that file's
  imports and its `MockProvider` + `BypassDecider` + `AgentRunOpts` construction
  helper verbatim, then:

```rust
#[tokio::test]
async fn on_usage_fires_once_per_turn_with_transcript_values() {
    // Two scripted responses: a tool_use turn, then a final text turn, each
    // carrying usage {input: 100/200, output: 10/20, cached: 5/7}.
    let seen: Arc<std::sync::Mutex<Vec<rupu_agent::UsageTurn>>> = Default::default();
    let seen2 = seen.clone();
    let mut opts = /* the runner_basic.rs opts builder, with the two-response MockProvider */;
    opts.on_usage = Some(Arc::new(move |u: &rupu_agent::UsageTurn| {
        seen2.lock().unwrap().push(u.clone());
    }));
    let transcript = opts.transcript_path.clone();
    let rr = rupu_agent::run_agent(opts).await.unwrap();

    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|u| u.kind == rupu_agent::UsageKind::Turn));
    assert_eq!(got[0].input_tokens, 100);
    assert_eq!(got[1].output_tokens, 20);
    assert_eq!(rr.total_tokens_cached, 12);

    // Hook values == the transcript's Usage events, in order.
    let usages: Vec<(u32, u32, u32)> = rupu_transcript::JsonlReader::iter(&transcript)
        .unwrap()
        .flatten()
        .filter_map(|e| match e {
            rupu_transcript::Event::Usage { input_tokens, output_tokens, cached_tokens, .. } =>
                Some((input_tokens, output_tokens, cached_tokens)),
            _ => None,
        })
        .collect();
    assert_eq!(usages, vec![(100, 10, 5), (200, 20, 7)]);
}
```

Script the `MockProvider` responses exactly as `runner_basic.rs` does, setting
`usage: Usage { input_tokens, output_tokens, cached_tokens, reasoning_tokens: 0 }`
on each scripted `LlmResponse`. If the mock exposes a builder for usage, use it.

Add a second test, `compaction_call_emits_usage_with_purpose_and_hook_kind`.
- Configure `context_window_tokens: Some(1000)` and `compact_at_percent: Some(50)`.
- Script a first turn whose usage has `input_tokens: 900`, so compaction
  triggers before turn 2 (see how `crates/rupu-agent/tests/*compact*` or the
  runner's inline compaction tests set this up — mirror them).
- Assert that the hook saw one `UsageKind::Compaction` call.
- Assert that the transcript has a `Usage` with `purpose == Some("compaction")`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-agent --test runner_usage_hook`
Expected: compile error (no `on_usage`, `UsageTurn`, `total_tokens_cached`).

- [ ] **Step 3: Implement**

1. Types, next to `OnToolCallCallback` (~line 101): paste the `UsageKind` /
   `UsageTurn` / `OnUsageCallback` definitions from **Interfaces**. Re-export
   them from `lib.rs` alongside `OnToolCallCallback`.
2. `AgentRunOpts`, directly after `on_stream_event`:

```rust
    /// Per-LLM-call usage hook (spec 2026-09-29 §3.3). Invoked synchronously
    /// right after every transcript `Usage` write — normal turns AND the
    /// compaction summariser — before any early exit, so no billed call can
    /// be missed. Must be cheap and must never panic; the orchestrator uses
    /// it to append the run's usage ledger.
    pub on_usage: Option<OnUsageCallback>,
```

3. Immediately after the per-turn `writer.write(&Event::Usage { … })?;`
   (~line 1316), set `purpose: None` in that literal, and add:

```rust
            total_cached += resp.usage.cached_tokens as u64;
            if let Some(cb) = &opts.on_usage {
                cb(&UsageTurn {
                    kind: UsageKind::Turn,
                    provider: opts.provider_name.clone(),
                    model: opts.model.clone(),
                    input_tokens: resp.usage.input_tokens as u64,
                    output_tokens: billable_output_tokens,
                    cached_tokens: resp.usage.cached_tokens as u64,
                });
            }
```

   Declare `let mut total_cached: u64 = 0;` next to
   `total_in` / `total_out` (~1074). Set `total_tokens_cached: total_cached` in
   every `RunResult { … }` the function returns (grep `RunResult {` inside
   `run_agent`).

4. `CompactionOutcome` gains `pub usage: rupu_providers::Usage,`. In
   `compact_messages`, set it from `summary_resp.usage.clone()` in the
   `Ok(Some(CompactionOutcome { … }))` literal.

5. At the call site that handles `Ok(Some(outcome))` from `compact_messages`
   (~455-500; it has `writer` and `opts` in scope — if the helper doesn't,
   return the usage up to the caller that does), write:

```rust
                let cu = &outcome.usage;
                let billable = cu.output_tokens as u64 + cu.reasoning_tokens as u64;
                writer.write(&Event::Usage {
                    provider: opts.provider_name.clone(),
                    model: opts.model.clone(),
                    served_model: None,
                    input_tokens: cu.input_tokens,
                    output_tokens: billable as u32,
                    cached_tokens: cu.cached_tokens,
                    purpose: Some("compaction".to_string()),
                })?;
                if let Some(cb) = &opts.on_usage {
                    cb(&UsageTurn {
                        kind: UsageKind::Compaction,
                        provider: opts.provider_name.clone(),
                        model: opts.model.clone(),
                        input_tokens: cu.input_tokens as u64,
                        output_tokens: billable,
                        cached_tokens: cu.cached_tokens as u64,
                    });
                }
```

   Do **not** add the compaction tokens to `total_in` / `total_out`. Those feed
   `RunComplete.total_tokens` and the context-budget arithmetic, which are
   turn-scoped. The spend is recorded in the transcript `Usage` and the ledger.

6. Add `on_usage: None,` to every other `AgentRunOpts { … }` literal. Add
   `total_tokens_cached: 0,` (or the real value where one exists) to other
   `RunResult` literals. Let the compiler find them:
   `cargo build --workspace --all-targets`.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-agent && cargo build --workspace --all-targets`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "feat(agent): on_usage hook per LLM call; compaction usage recorded; RunResult.total_tokens_cached"
```

---

### Task 4: Usage ledger writer (`rupu-orchestrator::usage_ledger`)

**Files:**
- Create: `crates/rupu-orchestrator/src/usage_ledger.rs`
- Modify: `crates/rupu-orchestrator/src/lib.rs` (`pub mod usage_ledger;`)
- Modify: `crates/rupu-orchestrator/src/runs.rs` — add
  `pub fn usage_ledger_path(&self, run_id: &str) -> PathBuf { self.run_dir(run_id).join("usage.jsonl") }`
  next to `events_path` (~1279).
- Test: inline tests in `usage_ledger.rs`

**Interfaces:**
- Consumes: `rupu_agent::{UsageTurn, UsageKind, OnUsageCallback}` (Task 3).
- Produces:

```rust
pub const LEDGER_VERSION: u32 = 1;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKind { Turn, Compaction }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerRow { v, id, at, kind, step_id, unit_index, unit_key, agent_run_id,
    parent_agent_run_id, transcript, agent, provider, model, input_tokens, output_tokens, cached_tokens }
#[derive(Debug, Clone, Default)]
pub struct LedgerTag { pub step_id: Option<String>, pub unit_index: Option<usize>, pub unit_key: Option<String> }
#[derive(Debug, Default)]
pub struct UnitTokenCounters { pub input: AtomicU64, pub output: AtomicU64 }
#[derive(Clone)]
pub struct UsageLedger { /* path + lazily opened shared file */ }
impl UsageLedger {
    pub fn open(path: PathBuf) -> Self;
    pub fn for_run(store: &RunStore, run_id: &str) -> Self;
    pub fn path(&self) -> &Path;
    pub fn append(&self, row: &LedgerRow);
    pub fn hook(&self, tag: LedgerTag, agent_run_id: String, parent_agent_run_id: Option<String>,
                transcript: PathBuf, agent: String, counters: Option<Arc<UnitTokenCounters>>)
        -> rupu_agent::OnUsageCallback;
}
```

- [ ] **Step 1: Write the failing tests** (bottom of the new file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn turn(input: u64) -> rupu_agent::UsageTurn {
        rupu_agent::UsageTurn {
            kind: rupu_agent::UsageKind::Turn,
            provider: "anthropic".into(),
            model: "claude-x".into(),
            input_tokens: input,
            output_tokens: 3,
            cached_tokens: 1,
        }
    }

    #[test]
    fn hook_appends_one_attributed_row_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let led = UsageLedger::open(dir.path().join("usage.jsonl"));
        let counters = Arc::new(UnitTokenCounters::default());
        let hook = led.hook(
            LedgerTag { step_id: Some("assess".into()), unit_index: Some(2), unit_key: Some("gw".into()) },
            "run_A".into(),
            None,
            dir.path().join("run_A.jsonl"),
            "reviewer".into(),
            Some(counters.clone()),
        );
        hook(&turn(100));
        hook(&turn(50));
        let body = std::fs::read_to_string(led.path()).unwrap();
        let rows: Vec<LedgerRow> = body.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        assert_eq!(rows[0].v, LEDGER_VERSION);
        assert_eq!(rows[0].step_id.as_deref(), Some("assess"));
        assert_eq!(rows[0].unit_index, Some(2));
        assert_eq!(rows[0].agent_run_id, "run_A");
        assert_eq!(rows[1].input_tokens, 50);
        assert_eq!(rows[0].kind, LedgerKind::Turn);
        assert_eq!(counters.input.load(Ordering::Relaxed), 150);
        assert_eq!(counters.output.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn concurrent_appends_from_two_handles_never_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let a = UsageLedger::open(path.clone());
        let b = UsageLedger::open(path.clone());
        let mut hs = Vec::new();
        for led in [a, b] {
            hs.push(std::thread::spawn(move || {
                let h = led.hook(LedgerTag::default(), "run_X".into(), None,
                    std::path::PathBuf::from("/t/run_X.jsonl"), "a".into(), None);
                for i in 0..500 { h(&turn(i)); }
            }));
        }
        for h in hs { h.join().unwrap(); }
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 1000);
        for l in body.lines() {
            serde_json::from_str::<LedgerRow>(l).expect("every line is a whole row");
        }
    }

    #[test]
    fn unwritable_ledger_never_panics() {
        let led = UsageLedger::open(std::path::PathBuf::from("/nonexistent-dir-xyz/usage.jsonl"));
        let h = led.hook(LedgerTag::default(), "r".into(), None, "/t/r.jsonl".into(), "a".into(), None);
        h(&turn(1)); // must not panic
    }

    #[test]
    fn row_json_shape_is_stable() {
        let row = LedgerRow {
            v: 1, id: "01X".into(),
            at: chrono::DateTime::parse_from_rfc3339("2026-09-29T00:00:00Z").unwrap().with_timezone(&chrono::Utc),
            kind: LedgerKind::Compaction, step_id: None, unit_index: None, unit_key: None,
            agent_run_id: "sub_1".into(), parent_agent_run_id: Some("run_P".into()),
            transcript: "/t/sub_1/transcript.jsonl".into(), agent: "a".into(),
            provider: "p".into(), model: "m".into(), input_tokens: 1, output_tokens: 2, cached_tokens: 0,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["kind"], "compaction");
        assert_eq!(v["parent_agent_run_id"], "run_P");
        assert!(v.get("step_id").is_none(), "None fields are omitted");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-orchestrator usage_ledger`
Expected: compile error (module missing).

- [ ] **Step 3: Implement** (top of `usage_ledger.rs`):

```rust
//! Per-run usage ledger (spec 2026-09-29 §3): one JSON row per LLM call made
//! anywhere inside a workflow run, appended to `<runs>/<id>/usage.jsonl`.
//! Rows carry a ULID `id` so any consumer that sees a line twice (mirror
//! replays, tailer races) counts it once. Writing never fails a run.

use crate::runs::RunStore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const LEDGER_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKind {
    Turn,
    Compaction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerRow {
    pub v: u32,
    pub id: String,
    pub at: DateTime<Utc>,
    pub kind: LedgerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_key: Option<String>,
    pub agent_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_run_id: Option<String>,
    pub transcript: PathBuf,
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
}

/// Where in the workflow an agent run sits. `step_id: None` = a dispatched
/// sub-agent (attributed to its ancestor's step by the fold).
#[derive(Debug, Clone, Default)]
pub struct LedgerTag {
    pub step_id: Option<String>,
    pub unit_index: Option<usize>,
    pub unit_key: Option<String>,
}

/// Running totals for one unit, filled by its hook — the honest source for
/// `UnitCompleted.tokens_in/out` (spec §3.4), on success AND failure.
#[derive(Debug, Default)]
pub struct UnitTokenCounters {
    pub input: AtomicU64,
    pub output: AtomicU64,
}

/// Cloneable append handle. The file opens lazily on first append (so a
/// ledger for a run that never calls a model creates no file) with
/// `O_APPEND | O_CREAT`; each row is ONE `write_all` of `json + "\n"` under
/// the lock, so rows from several handles/processes never interleave.
#[derive(Clone)]
pub struct UsageLedger {
    path: PathBuf,
    file: Arc<Mutex<Option<std::fs::File>>>,
}

impl std::fmt::Debug for UsageLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageLedger").field("path", &self.path).finish()
    }
}

impl UsageLedger {
    pub fn open(path: PathBuf) -> Self {
        Self { path, file: Arc::new(Mutex::new(None)) }
    }

    pub fn for_run(store: &RunStore, run_id: &str) -> Self {
        Self::open(store.usage_ledger_path(run_id))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one row. Errors are logged and swallowed — usage accounting
    /// must never fail a run.
    pub fn append(&self, row: &LedgerRow) {
        let mut line = match serde_json::to_vec(row) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "usage ledger: serialize failed");
                return;
            }
        };
        line.push(b'\n');
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.is_none() {
            match std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
                Ok(f) => *guard = Some(f),
                Err(e) => {
                    tracing::warn!(path = %self.path.display(), error = %e, "usage ledger: open failed");
                    return;
                }
            }
        }
        if let Some(f) = guard.as_mut() {
            if let Err(e) = f.write_all(&line).and_then(|_| f.flush()) {
                tracing::warn!(path = %self.path.display(), error = %e, "usage ledger: write failed");
            }
        }
    }

    /// Build the agent-loop `on_usage` hook for one agent run.
    pub fn hook(
        &self,
        tag: LedgerTag,
        agent_run_id: String,
        parent_agent_run_id: Option<String>,
        transcript: PathBuf,
        agent: String,
        counters: Option<Arc<UnitTokenCounters>>,
    ) -> rupu_agent::OnUsageCallback {
        let ledger = self.clone();
        Arc::new(move |u: &rupu_agent::UsageTurn| {
            if let Some(c) = &counters {
                c.input.fetch_add(u.input_tokens, Ordering::Relaxed);
                c.output.fetch_add(u.output_tokens, Ordering::Relaxed);
            }
            ledger.append(&LedgerRow {
                v: LEDGER_VERSION,
                id: ulid::Ulid::new().to_string(),
                at: Utc::now(),
                kind: match u.kind {
                    rupu_agent::UsageKind::Turn => LedgerKind::Turn,
                    rupu_agent::UsageKind::Compaction => LedgerKind::Compaction,
                },
                step_id: tag.step_id.clone(),
                unit_index: tag.unit_index,
                unit_key: tag.unit_key.clone(),
                agent_run_id: agent_run_id.clone(),
                parent_agent_run_id: parent_agent_run_id.clone(),
                transcript: transcript.clone(),
                agent: agent.clone(),
                provider: u.provider.clone(),
                model: u.model.clone(),
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cached_tokens: u.cached_tokens,
            });
        })
    }
}
```

Confirm `ulid`, `chrono`, `tracing` and `serde_json` are already
`rupu-orchestrator` deps (they are used in `runner.rs` / `runs.rs`). Add
`tempfile` to dev-deps only if it is missing.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-orchestrator usage_ledger`
Expected: 4 PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-orchestrator
git commit -m "feat(orchestrator): per-run usage ledger writer + agent hook"
```

---

### Task 5: Known transcripts (`RunStore::known_transcripts` + line parsers)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs` (new items near
  `sub_run_ids_recursive`, ~1079)
- Modify: `crates/rupu-cp/src/api/findings.rs:246-305` — `sub_run_ids_from_events`
  becomes a thin wrapper over the new parser (behavior unchanged; its two tests
  must still pass).
- Test: inline tests in `runs.rs`

**Interfaces:**
- Produces (in `rupu_orchestrator::runs`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownTranscript {
    /// `rupu_transcript::transcript_key(&path)`.
    pub key: String,
    /// Step id the transcript belongs to; `None` for a dispatched child.
    pub step_id: Option<String>,
    pub path: PathBuf,
}
/// Transcript named by ONE events.jsonl line (StepWorking w/ path, UnitStarted, DispatchStarted).
pub fn known_transcript_from_event_line(line: &str) -> Option<KnownTranscript>;
/// Transcripts named by ONE step_results.jsonl line (the step + every item).
pub fn known_transcripts_from_step_result_line(line: &str) -> Vec<KnownTranscript>;
impl RunStore {
    /// step_results ∪ events ∪ recursive sub-runs of the run id AND of every
    /// key found so far — deduped by key, first occurrence wins (step_results
    /// first, so a known step label beats an event-derived one).
    pub fn known_transcripts(&self, run_id: &str) -> Vec<KnownTranscript>;
    /// Transcript path of a dispatched sub-run: `<root>/<parent>/sub/<sub>/transcript.jsonl`.
    pub fn sub_run_transcript_path(&self, parent_run_id: &str, sub_run_id: &str) -> PathBuf;
}
```

- [ ] **Step 1: Write the failing tests** (in `runs.rs`'s test module; reuse
  its existing `sample_step_result` helper and tempdir store setup):

```rust
    #[test]
    fn known_transcripts_unions_step_results_events_and_dispatch_subruns() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let rec = /* create a run exactly like the existing step_results tests do */;
        let t = tmp.path().join("transcripts");
        // 1) a completed step with one fan-out item
        let mut sr = sample_step_result("assess");
        sr.transcript_path = t.join("run_STEP.jsonl");
        sr.items = vec![/* one ItemResultRecord built like existing tests,
                           transcript_path = t.join("run_ITEM1.jsonl") */];
        store.append_step_result(&rec.id, &sr).unwrap();
        // 2) events: an in-flight unit (retry attempt) + a linear step working + a dispatch
        let ev = store.events_path(&rec.id);
        let lines = [
            serde_json::json!({"type":"unit_started","run_id":rec.id,"step_id":"assess","index":1,
                "unit_key":"b","agent":"a","transcript_path": t.join("run_RETRY.jsonl")}),
            serde_json::json!({"type":"step_working","run_id":rec.id,"step_id":"verify","note":null,
                "transcript_path": t.join("run_LIVE.jsonl")}),
            serde_json::json!({"type":"dispatch_started","run_id":rec.id,"sub_run_id":"sub_D1",
                "agent":"helper","transcript_path": store.sub_run_transcript_path("run_LIVE","sub_D1")}),
        ];
        std::fs::write(&ev, lines.iter().map(|l| l.to_string() + "\n").collect::<String>()).unwrap();
        // 3) a grandchild only discoverable by walking sub/ of a known key
        let (sub2, _) = store.create_sub_run("sub_D1", "helper").unwrap();

        let got = store.known_transcripts(&rec.id);
        let keys: Vec<&str> = got.iter().map(|k| k.key.as_str()).collect();
        for want in ["run_STEP", "run_ITEM1", "run_RETRY", "run_LIVE", "sub_D1", sub2.as_str()] {
            assert!(keys.contains(&want), "missing {want} in {keys:?}");
        }
        assert_eq!(keys.len(), 6, "deduped: {keys:?}");
        let step_of = |k: &str| got.iter().find(|x| x.key == k).unwrap().step_id.clone();
        assert_eq!(step_of("run_RETRY").as_deref(), Some("assess"));
        assert_eq!(step_of("sub_D1"), None);
    }

    #[test]
    fn known_transcripts_tolerates_garbage_and_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        assert!(store.known_transcripts("run_nope").is_empty());
        assert!(known_transcript_from_event_line("{not json").is_none());
        assert!(known_transcript_from_event_line(r#"{"type":"step_working","run_id":"r","step_id":"s","note":"tool"}"#).is_none());
    }
```

Fill the two `/* … */` spots by copying the construction code the existing
`step_results` tests in `runs.rs` already use (`RunStore::create(...)` and an
`ItemResultRecord { … }` literal). Do not invent new helpers.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-orchestrator known_transcripts`
Expected: compile error.

- [ ] **Step 3: Implement** (in `runs.rs`):

```rust
/// A transcript some part of a run wrote to (spec 2026-09-29 §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownTranscript {
    pub key: String,
    pub step_id: Option<String>,
    pub path: PathBuf,
}

fn known(path: PathBuf, step_id: Option<String>) -> Option<KnownTranscript> {
    let key = rupu_transcript::transcript_key(&path)?;
    Some(KnownTranscript { key, step_id, path })
}

/// The transcript a single `events.jsonl` line names, if any. Parsed
/// leniently as JSON (not the typed `Event`), so a newer writer's extra
/// variants never break discovery.
pub fn known_transcript_from_event_line(line: &str) -> Option<KnownTranscript> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ty = v.get("type")?.as_str()?;
    if !matches!(ty, "step_working" | "unit_started" | "dispatch_started") {
        return None;
    }
    let path = PathBuf::from(v.get("transcript_path")?.as_str()?);
    let step = if ty == "dispatch_started" {
        None
    } else {
        v.get("step_id").and_then(|s| s.as_str()).map(str::to_string)
    };
    known(path, step)
}

/// Transcripts a single `step_results.jsonl` line names (step + items).
pub fn known_transcripts_from_step_result_line(line: &str) -> Vec<KnownTranscript> {
    let Ok(rec) = serde_json::from_str::<StepResultRecord>(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    out.extend(known(rec.transcript_path.clone(), Some(rec.step_id.clone())));
    for item in &rec.items {
        out.extend(known(item.transcript_path.clone(), Some(rec.step_id.clone())));
    }
    out
}

impl RunStore {
    pub fn sub_run_transcript_path(&self, parent_run_id: &str, sub_run_id: &str) -> PathBuf {
        self.sub_run_transcript(parent_run_id, sub_run_id)
    }

    pub fn known_transcripts(&self, run_id: &str) -> Vec<KnownTranscript> {
        let mut out: Vec<KnownTranscript> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut push = |k: KnownTranscript, out: &mut Vec<KnownTranscript>| {
            if seen.insert(k.key.clone()) {
                out.push(k);
            }
        };
        let sr = self.run_dir(run_id).join("step_results.jsonl");
        if let Ok(body) = std::fs::read_to_string(&sr) {
            for line in body.lines().filter(|l| !l.trim().is_empty()) {
                for k in known_transcripts_from_step_result_line(line) {
                    push(k, &mut out);
                }
            }
        }
        if let Ok(body) = std::fs::read_to_string(self.events_path(run_id)) {
            for line in body.lines().filter(|l| !l.trim().is_empty()) {
                if let Some(k) = known_transcript_from_event_line(line) {
                    push(k, &mut out);
                }
            }
        }
        // Dispatched sub-agents live under `<root>/<parent>/sub/` where the
        // parent is the DISPATCHING AGENT's run id (not the workflow run id),
        // so walk from the run id and from every key found so far.
        let mut roots: Vec<String> = vec![run_id.to_string()];
        roots.extend(out.iter().map(|k| k.key.clone()));
        for root in roots {
            for sub in self.sub_run_ids_recursive(&root) {
                // Parent of `sub` is unknown here; recompute its path by
                // searching the direct parent chain is unnecessary — the
                // nested layout's key is the sub id, and its path is
                // `<root>/<parent>/sub/<sub>/transcript.jsonl`. Resolve it by
                // scanning `root`'s subtree for the directory.
                if let Some(path) = self.find_sub_run_transcript(&root, &sub) {
                    push(KnownTranscript { key: sub, step_id: None, path }, &mut out);
                }
            }
        }
        out
    }

    /// Locate `<…>/sub/<sub_id>/transcript.jsonl` under `root`'s subtree.
    fn find_sub_run_transcript(&self, root: &str, sub_id: &str) -> Option<PathBuf> {
        let mut frontier = vec![root.to_string()];
        let mut guard = 0u32;
        while let Some(id) = frontier.pop() {
            guard += 1;
            if guard > 10_000 {
                return None;
            }
            let candidate = self.sub_run_transcript(&id, sub_id);
            if candidate.exists() {
                return Some(candidate);
            }
            frontier.extend(self.sub_run_ids(&id));
        }
        None
    }
}
```

**Simplification allowed:** if you change `sub_run_ids_recursive` to also
return each child's parent id (a private
`sub_run_edges_recursive(&self, run_id) -> Vec<(String parent, String child)>`
that both functions use), use that edge list to build the path directly and
delete `find_sub_run_transcript`. Prefer this if it keeps
`sub_run_ids_recursive`'s public behavior identical (its tests must pass
unchanged).

Then in `crates/rupu-cp/src/api/findings.rs`, replace the body of
`sub_run_ids_from_events` with:

```rust
    let Ok(body) = std::fs::read_to_string(store.events_path(parent)) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(rupu_orchestrator::runs::known_transcript_from_event_line)
        .filter(|k| k.step_id.is_some()) // findings scope: step/unit transcripts, as before
        .map(|k| k.key)
        .collect()
```

The old function matched only `UnitStarted` and `StepWorking` (not
`DispatchStarted`); the `.filter(|k| k.step_id.is_some())` keeps that exact
behavior. Remove the now-unused `BufReader` / `Event` imports if clippy flags
them.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-orchestrator known_transcripts && cargo test -p rupu-cp findings`
Expected: PASS (including the two existing `resolve_run_scope_*` tests).

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-orchestrator crates/rupu-cp/src/api/findings.rs
git commit -m "feat(orchestrator): RunStore::known_transcripts — step results ∪ live events ∪ dispatch sub-runs"
```

---

### Task 6: Orchestrator emits the ledger at every agent launch + honest unit totals

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs`:
  - `dispatch_one` (~7449);
  - its six callers: `run_reject_cleanup` ~5810, `run_linear_step` ~6199,
    `run_fanout_step` ~6944, `run_parallel_step` ~7267, `dispatch_fixer` ~7999,
    `run_panel_iteration` ~8144;
  - `run_node` ~4551 (passes `run_id` into `run_parallel_step`);
  - the three `UnitCompleted` emissions ~6992 / ~8018 / ~8172;
  - the remote-unit completion path in `run_fanout_step` (~6794 / ~6858).
- Test: `crates/rupu-orchestrator/tests/usage_ledger_e2e.rs` (new)

**Interfaces:**
- Consumes: `UsageLedger`, `LedgerTag`, `UnitTokenCounters` (Task 4); `AgentRunOpts.on_usage` (Task 3).
- Produces:
  - `dispatch_one(…, on_usage: Option<rupu_agent::OnUsageCallback>)`, a new
    final parameter.
  - A private
    `fn ledger_hook(opts: &OrchestratorRunOpts, workflow_run_id: &str, tag: LedgerTag, agent_run_id: &str, transcript: &Path, agent: &str, counters: Option<Arc<UnitTokenCounters>>) -> Option<rupu_agent::OnUsageCallback>`
    that returns `None` when `opts.run_store` is `None` or `workflow_run_id` is
    empty.
  - `run_parallel_step(workflow_run_id: &str, step, ctx, opts, continue_on_error)`.

- [ ] **Step 1: Write the failing e2e test** — `crates/rupu-orchestrator/tests/usage_ledger_e2e.rs`.
  Base it on `crates/rupu-orchestrator/tests/linear_runner.rs` (for the
  `MockProvider`-backed `StepFactory` + `OrchestratorRunOpts` with a tempdir
  `RunStore`) and `tests/dispatch_agents_parallel.rs` / `tests/branch_runner.rs`
  (for fan-out / parallel / panel YAML). Copy their setup helpers; do not invent
  new ones. The workflow YAML:

```yaml
name: ledger-e2e
steps:
  - id: plan
    agent: a
    prompt: "go"
  - id: fan
    agent: a
    for_each: "x\ny\nz"
    prompt: "{{ item }}"
    max_parallel: 3
  - id: par
    parallel:
      - id: p1
        agent: a
        prompt: "one"
      - id: p2
        agent: a
        prompt: "two"
```

The mock returns one final-text response with
`usage { input_tokens: 10, output_tokens: 1, cached_tokens: 0 }` per agent run.
Assertions after `run_workflow` completes:

```rust
    let ledger = store.usage_ledger_path(&run_id);
    let rows: Vec<rupu_orchestrator::usage_ledger::LedgerRow> = std::fs::read_to_string(&ledger)
        .unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    // 1 (plan) + 3 (fan units) + 2 (parallel subs) agent runs × 1 LLM call each
    assert_eq!(rows.len(), 6);
    let by_step = |s: &str| rows.iter().filter(|r| r.step_id.as_deref() == Some(s)).count();
    assert_eq!(by_step("plan"), 1);
    assert_eq!(by_step("fan"), 3);
    assert_eq!(by_step("par"), 2);
    let mut idx: Vec<usize> = rows.iter().filter(|r| r.step_id.as_deref() == Some("fan"))
        .filter_map(|r| r.unit_index).collect();
    idx.sort();
    assert_eq!(idx, vec![0, 1, 2]);
    // Every row's transcript is a real transcript containing a Usage event.
    for r in &rows { assert!(r.transcript.exists(), "{:?}", r.transcript); }
```

Add a second test, `unit_completed_carries_real_tokens`. Collect events with
the `CollectingSink` pattern from `crates/rupu-orchestrator/src/runner.rs`
tests (~9776), or with a `JsonlSink` into the run's `events.jsonl` that you
read back. Assert that every `unit_completed` for step `fan` has
`tokens_in == 10 && tokens_out == 1`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-orchestrator --test usage_ledger_e2e`
Expected: FAIL — ledger file missing, and the `UnitCompleted` tokens are 0.

- [ ] **Step 3: Implement**

1. Add the helper near `dispatch_one`:

```rust
/// Build the usage-ledger hook for one agent run (spec 2026-09-29 §3.3).
/// `None` for in-memory runs (no store / empty run id).
fn ledger_hook(
    opts: &OrchestratorRunOpts,
    workflow_run_id: &str,
    tag: crate::usage_ledger::LedgerTag,
    agent_run_id: &str,
    transcript: &Path,
    agent: &str,
    counters: Option<Arc<crate::usage_ledger::UnitTokenCounters>>,
) -> Option<rupu_agent::OnUsageCallback> {
    let store = opts.run_store.as_ref()?;
    if workflow_run_id.is_empty() {
        return None;
    }
    let ledger = crate::usage_ledger::UsageLedger::for_run(store, workflow_run_id);
    Some(ledger.hook(
        tag,
        agent_run_id.to_string(),
        None,
        transcript.to_path_buf(),
        agent.to_string(),
        counters,
    ))
}
```

2. `dispatch_one` gains a final parameter
   `on_usage: Option<rupu_agent::OnUsageCallback>`. After the
   `agent_opts.pause = pause;` line, add `agent_opts.on_usage = on_usage;`.

3. At each call site, build the hook **before** any `tokio::spawn` (the
   callback is an `Arc`, so clone it into the task) and pass it as the new last
   argument:

| Site | `workflow_run_id` in scope as | `LedgerTag` |
|---|---|---|
| `run_reject_cleanup` ~5810 | `run_id` (local, ~line 5619) | `{ step_id: Some(step.id.clone()), unit_index: None, unit_key: None }`; agent run id `step_run_id` |
| `run_linear_step` ~6199 | `workflow_run_id` | `{ step_id: Some(step.id.clone()), .. }`; agent run id `run_id` |
| `run_fanout_step` ~6944 | `workflow_run_id` (cloned into the task — find the existing clone used for `UnitStarted`) | `{ step_id: Some(step_id.clone()), unit_index: Some(idx), unit_key: Some(unit_key.clone()) }`; agent run id `run_id_clone`; **with** `counters` |
| `run_parallel_step` ~7267 | new `workflow_run_id` param | `{ step_id: Some(parent_step_id.clone()), unit_index: Some(idx), unit_key: Some(sub_id.clone()) }`; agent run id `run_id_clone` |
| `dispatch_fixer` ~7999 | `workflow_run_id` | `{ step_id: Some(step.id.clone()), unit_index: Some(unit_index), unit_key: Some(unit_key.clone()) }`; **with** `counters` |
| `run_panel_iteration` ~8144 | `workflow_run_id` | `{ step_id: Some(parent_step_id.clone()), unit_index: Some(<the view_index/unit index used for that panelist's UnitStarted>), unit_key: Some(<its unit_key>) }`; **with** `counters` |

   The `agent` argument is the agent name passed to that `dispatch_one` call.

4. `run_parallel_step` signature: add `workflow_run_id: &str` as the first
   parameter. At `run_node` (~4551), call
   `run_parallel_step(run_id, step, ctx, opts, effective_continue_on_error)`.
   Fix any other callers the compiler flags.

5. Honest `UnitCompleted` (the three sites). Create
   `let counters = Arc::new(crate::usage_ledger::UnitTokenCounters::default());`
   per unit, pass `Some(counters.clone())` into `ledger_hook`, and emit:

```rust
                            tokens_in: counters.input.load(std::sync::atomic::Ordering::Relaxed),
                            tokens_out: counters.output.load(std::sync::atomic::Ordering::Relaxed),
```

   Replace the "Tokens are not available…emit 0" comments with:
   `// Real totals from this unit's usage hook (spec 2026-09-29 §3.4).`
   When `ledger_hook` returns `None` (no store), counters stay 0. That is
   correct for in-memory runs.

6. Remote (placed / distributed) units. After the unit's outcome is known and
   before its `UnitCompleted` emit, when the unit ran remotely and its
   transcript path exists locally (the mirror), fold it once:

```rust
    let (tin, tout) = rupu_transcript::aggregate(&[&transcript_path], Default::default())
        .iter()
        .fold((0u64, 0u64), |(i, o), r| (i + r.input_tokens, o + r.output_tokens));
```

   Use `(tin, tout)` for that unit's `UnitCompleted`. Locate the remote branch
   by the `UnitDispatch` construction (~6794 / ~6858). Its completion flows into
   the same `UnitCompleted` emit, so compute the pair there, gated on the
   placement being remote.

7. Update the `UnitCompleted` doc comment in `executor/event.rs` (~88-93) to:
   "`tokens_in` / `tokens_out` are the unit's real totals — from its usage
   hook for local units, from its mirrored transcript for remote units (`0`
   only for in-memory runs with no store)."

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-orchestrator` (the whole crate — `dispatch_one`'s
signature change touches every step kind).
Expected: PASS, including the new e2e tests and every pre-existing test.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-orchestrator
git commit -m "feat(orchestrator): usage ledger at every agent launch; honest UnitCompleted tokens"
```

---

### Task 7: `dispatch_agent` children write the root run's ledger

**Files:**
- Modify: `crates/rupu-cli/src/cmd/dispatch.rs`:
  - struct (~26-75): add the `usage_ledger` field;
  - child `AgentRunOpts` (~330-370): set `on_usage`.
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` ~3214 and ~4759, and
  `crates/rupu-cli/src/resume.rs` ~297 — chain `.with_usage_ledger(…)` after
  `CliAgentDispatcher::new(…)`.
- Test: `crates/rupu-cli/src/cmd/dispatch.rs` test module (next to the existing
  DispatchStarted/Completed test ~600-700).

**Interfaces:**
- Consumes: `rupu_orchestrator::usage_ledger::{UsageLedger, LedgerTag}`.
- Produces:
  `pub fn with_usage_ledger(self: Arc<Self>, ledger: UsageLedger) -> Arc<Self>`.
  If `new` returns `Arc<Self>`, instead add a `usage_ledger: Option<UsageLedger>`
  parameter to `new` and update its four callers. Choose whichever fits
  `new`'s actual return type; both are acceptable.

- [ ] **Step 1: Write the failing test.** Extend the existing
  dispatch-events test (~602-700), which already runs a child through a mock
  provider without the network. Construct the dispatcher with a ledger at
  `tmp/usage.jsonl`. After the dispatch, assert that the ledger has ≥1 row
  where:
  - `step_id == None`;
  - `agent_run_id == <the sub_run_id from DispatchStarted>`;
  - `parent_agent_run_id == Some(<the parent_run_id passed to dispatch>)`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli dispatch`
Expected: FAIL (no ledger rows / no API).

- [ ] **Step 3: Implement.** In the child opts literal, replace `on_usage: None,`
  (added in Task 3) with:

```rust
            on_usage: self.usage_ledger.as_ref().map(|l| {
                l.hook(
                    rupu_orchestrator::usage_ledger::LedgerTag::default(),
                    sub_run_id.clone(),
                    Some(parent_run_id.to_string()),
                    transcript_path.clone(),
                    agent_name.to_string(),
                    None,
                )
            }),
```

At the three workflow build sites, pass
`rupu_orchestrator::usage_ledger::UsageLedger::open(runs_dir.join(&run_id).join("usage.jsonl"))`.
Use the same `runs_dir` / `run_id` those sites already use for `events.jsonl`.
In `resume.rs` the base is `global.join("runs")` for one branch and `runs_dir`
for the other; mirror each branch's `events_path` exactly. `cmd/run.rs:787`
(standalone `rupu run`) passes **no** ledger: those children are counted by the
fallback (spec §3.3).

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-cli dispatch && cargo build -p rupu-cli`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-cli
git commit -m "feat(cli): dispatch_agent children append the root run's usage ledger"
```

---

### Task 8: The fold — `rupu-cp::usage_index` + re-implemented `usage.rs` entry points

**Files:**
- Create: `crates/rupu-cp/src/usage_index.rs`
- Modify: `crates/rupu-cp/src/lib.rs` (`pub mod usage_index;`)
- Modify: `crates/rupu-cp/src/usage.rs`:
  - `UsageSummary` gains `partial`;
  - `run_transcript_paths`, `summarize_run`, `run_metrics` and `turn_series`
    are re-implemented;
  - new `run_usage`, `transcripts_usage`, `summarize_run_usage`.
- Test: inline tests in `usage_index.rs`. The existing `usage.rs` tests must
  still pass; update the three `run_transcript_paths` tests only if their
  expectations were "step_results only" and now legitimately include more.

**Interfaces:**
- Consumes: `JsonlCursor`, `transcript_key` (Task 1); `LedgerRow`, `LedgerKind`
  (Task 4); `KnownTranscript`, `known_transcript_from_event_line`,
  `known_transcripts_from_step_result_line` (Task 5);
  `crate::host::transcript_paths::{global_dir_of, cache_path, agent_mirror_path}`.
- Produces:

```rust
// usage_index.rs
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Tokens { pub input: u64, pub output: u64, pub cached: u64 }
#[derive(Debug, Clone, Default)]
pub struct RunUsage {
    pub rows: Vec<rupu_transcript::UsageRow>,
    pub by_step: BTreeMap<String, Vec<rupu_transcript::UsageRow>>, // "" = unattributed
    pub by_unit: BTreeMap<(String, usize), Tokens>,
    pub turns: u64,
    pub duration_ms: Option<u64>,
    pub points: Vec<crate::usage::TurnPoint>,
    pub partial: bool,
    /// Changes whenever this run's series is rebuilt non-append-only (Plan 2 `epoch`).
    pub epoch: u64,
}
pub struct UsageIndex { … }
impl UsageIndex {
    pub fn global() -> &'static UsageIndex;
    pub fn run_usage(&self, store: &RunStore, run_id: &str) -> Arc<RunUsage>;
    pub fn transcripts_usage(&self, labeled: &[(String, PathBuf)]) -> Arc<RunUsage>;
    /// Resolved local paths of every known transcript of a run (aggregate dedupe, netflow).
    pub fn resolved_transcripts(&self, store: &RunStore, run_id: &str) -> Vec<PathBuf>;
}
// usage.rs
pub struct UsageSummary { …existing…, #[serde(default)] pub partial: bool }
pub fn run_usage(store: &RunStore, run_id: &str) -> Arc<RunUsage>;
pub fn transcripts_usage(labeled: &[(String, PathBuf)]) -> Arc<RunUsage>;
pub fn summarize_run_usage(u: &RunUsage, pricing: &PricingConfig) -> UsageSummary; // sets partial
// unchanged signatures, new bodies:
pub fn summarize_run(store, run_id, pricing) -> UsageSummary;
pub fn run_metrics(store, run_id, pricing) -> RunMetrics;
pub fn run_transcript_paths(store, run_id) -> Vec<PathBuf>;
```

**Fold rules (spec §4.2) — the implementation MUST follow these exactly:**
1. **Ledger.** Parse each `usage.jsonl` line as `LedgerRow`. Skip unparseable
   lines. Dedup by `id`. Accumulate per `agent_run_id`:
   `AgentAgg { step_id, unit_index, parent, by_model: BTreeMap<(provider, model, agent), Tokens> }`.
   Record `transcript_key(&row.transcript)` into `ledger_keys`. `turns += 1`
   only for `kind == Turn`. Push `(agent_run_id, Tokens)` to the raw points.
2. **Known set.** Build it incrementally from `step_results.jsonl` (via
   `known_transcripts_from_step_result_line`) and `events.jsonl` (via
   `known_transcript_from_event_line`), each with its own `JsonlCursor`.
   `RunStore::known_transcripts`' sub-run walk is applied **only** when the run
   has no ledger file, or when the run is terminal and not yet sealed (see 5).
   For a live ledger run, dispatch children arrive as ledger rows and as
   `DispatchStarted` events.
3. **Fallback.** For each known key **not** in `ledger_keys`:
   - Resolve the path in this order: the path itself if it exists; else, when
     `rec.worker_id` is set, `cache_path(global, worker, path)` if it exists;
     else `agent_mirror_path(global, key)` if it exists.
   - If none resolves, set `partial = true` and skip it.
   - Otherwise fold it through a shared per-path `TranscriptFold` cache
     (`JsonlCursor` plus: `agent` from `RunStart`; `by_model` keyed
     `(provider, model)`; `turns`, incremented for every `Usage` whose
     `purpose != Some("compaction")`, even before `RunStart` — matching the old
     `aggregate_rows_and_metrics`; tokens counted only after `RunStart`;
     `duration_ms` = the last `RunComplete`; `saw_run_start`; `points`).
   - A file with `RunStart` and no usage contributes a zero row with
     `runs = 1`.
   - A `step_results`-labeled key uses its step as the label; an event-labeled
     key uses its event step; otherwise `""`.
4. **Finish** (`build_run_usage`):
   - Resolve each ledger agent's step and unit through the `parent` chain:
     walk up at most 64 hops until an agg with `step_id` is found; an
     unresolved agent goes to label `""`.
   - `rows` is keyed `(provider, model, agent)`, and each contributing
     transcript adds `runs += 1`. Ledger: one transcript per `agent_run_id`.
     Fallback: one per file.
   - Build `by_step` and `by_unit` the same way.
   - `points` = the ledger raw points (labeled via the resolved step) in arrival
     order, followed by the fallback points in known-set order. `turn` is the
     1-based index.
   - `duration_ms` = the max fallback `RunComplete` duration.
5. **Caching.** `UsageIndex` holds
   `Mutex<HashMap<(PathBuf store_root, String run_id), Arc<Mutex<RunState>>>>`
   and `Mutex<HashMap<PathBuf, Arc<Mutex<TranscriptFold>>>>`.
   - Hold the map lock only long enough to get or insert the entry `Arc`.
   - `RunState` keeps the last `Arc<RunUsage>` and returns it unchanged when a
     call consumed zero bytes from the ledger, events, step_results and every
     fallback transcript, and found no new keys.
   - A terminal run (`rec.status` terminal, via `store.load`) is **sealed**
     after one full computation, including the sub-run walk. After that it is
     re-validated only by `std::fs::metadata` len/mtime of `run.json`,
     `usage.jsonl`, `events.jsonl` and `step_results.jsonl`, with no transcript
     stats. That keeps the Usage page (all runs) cheap.
   - A ledger `reset` (the file shrank) clears the ledger fold and bumps
     `epoch`. So does a fallback transcript's reset, or a fallback key appearing
     after ledger points already exist.
   - `epoch` must differ across CP restarts: `UsageIndex` stores
     `base_epoch: u64` = UNIX nanos at construction (`Default` impl computes
     it). Each new `RunState` starts at `base_epoch`, and every bump is `+= 1`.
     `UsageIndex` therefore gets a hand-written `Default`, not
     `#[derive(Default)]`.

- [ ] **Step 1: Write the failing tests** (`usage_index.rs` test module). Helpers:
  - `fn ledger_line(id, step, unit, agent_run, parent, transcript, input, output, kind) -> String`
    (via `serde_json::to_string(&LedgerRow{…})`);
  - `fn transcript_lines(agent, provider, model, &[(in, out)]) -> String`, which
    writes a `run_start` event then `usage` events, in the exact serde shape of
    `rupu_transcript::Event` (build the `Event` values and serialize them —
    don't hand-write JSON).

  Tests:

```rust
    #[test]
    fn ledger_rows_are_summed_and_deduped_by_id() { /* two identical lines (same id) + one more → counts 2 rows' tokens */ }

    #[test]
    fn transcript_with_ledger_rows_is_not_double_counted() {
        /* run with usage.jsonl rows for transcript run_A AND step_results naming run_A.jsonl
           whose file has Usage events → total == ledger total only */
    }

    #[test]
    fn known_transcript_without_ledger_rows_falls_back_to_file() {
        /* no usage.jsonl; step_results names run_A (done), events names run_B (in-flight),
           both files have usage → total == A + B; turns == events count; by_step keyed by step */
    }

    #[test]
    fn dispatch_child_is_attributed_to_ancestor_step() {
        /* ledger: run_P {step "s", unit 1}; sub_C {parent run_P}; sub_G {parent sub_C}
           → by_step["s"] includes all three; by_unit[("s",1)] includes all three */
    }

    #[test]
    fn compaction_rows_count_tokens_not_turns() { /* kind compaction → turns unchanged */ }

    #[test]
    fn unreadable_known_transcript_sets_partial() { /* events name /nope/run_X.jsonl → partial */ }

    #[test]
    fn incremental_append_equals_one_shot() {
        /* compute run_usage; append 3 more ledger lines + 1 partial line; compute again →
           equals a fresh UsageIndex::default() computed after completing the partial line;
           and the second call before completion does NOT include the partial row */
    }

    #[test]
    fn unchanged_run_returns_same_arc() { /* two calls, no writes → Arc::ptr_eq */ }

    #[test]
    fn remote_worker_run_resolves_mirror_cache() {
        /* reuse the `seed_remote_run` fixture from usage.rs tests (~840): known transcript missing
           locally, cache file present → counted, partial == false */
    }
```

Write each test body fully when implementing. Every comment above states the
exact fixture and assertion. Create runs with `RunStore::create` as the
existing `usage.rs` tests do (see `seed_remote_run`). Tests construct
`UsageIndex::default()` (not `global()`), so they are isolated.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp usage_index`
Expected: compile error.

- [ ] **Step 3: Implement `usage_index.rs`** per the rules above. Code skeleton (fill every body):

```rust
//! Incremental, process-cached usage fold — the ONE definition of "a run's
//! usage" (spec 2026-09-29 §4). Every CP endpoint and the CLI go through here.

use crate::usage::TurnPoint;
use rupu_orchestrator::runs::{
    known_transcript_from_event_line, known_transcripts_from_step_result_line, KnownTranscript, RunStore,
};
use rupu_orchestrator::usage_ledger::{LedgerKind, LedgerRow};
use rupu_transcript::{transcript_key, Event, JsonlCursor, UsageRow};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

type ModelKey = (String, String, String);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Tokens { pub input: u64, pub output: u64, pub cached: u64 }
impl Tokens {
    fn add(&mut self, o: Tokens) { self.input += o.input; self.output += o.output; self.cached += o.cached; }
}

#[derive(Debug, Clone, Default)]
pub struct RunUsage { /* fields exactly as in Interfaces */ }

#[derive(Debug, Default, Clone)]
struct AgentAgg { step_id: Option<String>, unit_index: Option<usize>, parent: Option<String>,
                  by_model: BTreeMap<ModelKey, Tokens> }

#[derive(Debug, Default)]
struct LedgerFold { cursor: JsonlCursor, seen: HashSet<String>, agents: HashMap<String, AgentAgg>,
                    keys: HashSet<String>, points: Vec<(String, Tokens)>, turns: u64 }

#[derive(Debug, Default)]
struct TranscriptFold { cursor: JsonlCursor, agent: Option<String>, by_model: BTreeMap<(String, String), Tokens>,
                        turns: u64, duration_ms: Option<u64>, points: Vec<Tokens>, saw_run_start: bool }

#[derive(Debug, Default)]
struct RunState {
    ledger: LedgerFold,
    events: JsonlCursor,
    step_results: JsonlCursor,
    known: Vec<KnownTranscript>,       // insertion-ordered
    known_keys: HashSet<String>,
    walked: bool,                      // sub-run walk done
    seal: Option<[(u64, Option<std::time::SystemTime>); 4]>,
    last: Option<Arc<RunUsage>>,
    epoch: u64,
}

pub struct UsageIndex {
    runs: Mutex<HashMap<(PathBuf, String), Arc<Mutex<RunState>>>>,
    files: Mutex<HashMap<PathBuf, Arc<Mutex<TranscriptFold>>>>,
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
        Self { runs: Mutex::default(), files: Mutex::default(), base_epoch }
    }
}

impl UsageIndex {
    pub fn global() -> &'static UsageIndex {
        static G: OnceLock<UsageIndex> = OnceLock::new();
        G.get_or_init(UsageIndex::default)
    }
    pub fn run_usage(&self, store: &RunStore, run_id: &str) -> Arc<RunUsage> { /* rules 1–5 */ }
    pub fn transcripts_usage(&self, labeled: &[(String, PathBuf)]) -> Arc<RunUsage> {
        /* fallback-only fold over the given paths (label = step label), shared file cache;
           no caching of the result beyond the per-file folds */
    }
    pub fn resolved_transcripts(&self, store: &RunStore, run_id: &str) -> Vec<PathBuf> {
        /* run_usage(store, run_id) first (populates state), then the known list resolved with
           the same resolution as rule 3 (existing paths only) PLUS every ledger row's
           transcript path that exists */
    }
}
```

Then in `usage.rs`:
- Add `#[serde(default)] pub partial: bool` to `UsageSummary`. It is
  `Default`-derived, so existing constructors compile. `rollup` ORs `partial`.
- Add `run_usage` / `transcripts_usage`, which delegate to
  `UsageIndex::global()`.
- Add
  `summarize_run_usage(u, pricing) = { let mut s = summarize(&u.rows, pricing); s.partial = u.partial; s }`.
- Re-implement:
  - `summarize_run` → `summarize_run_usage(&run_usage(store, id), pricing)`;
  - `run_metrics` → `RunMetrics { usage: summarize_run_usage(..), turns: u.turns, duration_ms }`,
    where `duration_ms` = `rec.finished_at - rec.started_at` when
    `store.load(id)` yields a finished record, else `u.duration_ms`;
  - `run_transcript_paths` →
    `UsageIndex::global().resolved_transcripts(store, run_id)`.
- Keep `summarize_paths`, `run_metrics_paths` and `turn_series(labeled)` as
  they are (standalone/session callers still use them until Task 9). Update
  their doc comments to point at `transcripts_usage` as the preferred path.
- Update `usage.rs`'s module docs to describe the ledger-first rule.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-cp usage && cargo test -p rupu-cp`
Expected: PASS. If `macos_fixtures` fails because `UsageSummary` gained
`partial`, run `make macos-fixtures` from the repo root, re-run, and commit the
regenerated `apps/rupu-macos/Fixtures/*.json`.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-cp apps/rupu-macos/Fixtures
git commit -m "feat(cp): usage_index — ledger-first incremental fold; summarize_run/run_metrics on top"
```

---

### Task 9: CP consumers — run-scoped endpoints, agent runs, sessions, aggregates

**Files:**
- Modify: `crates/rupu-cp/src/api/usage.rs`:
  - `local_usage` ~414-455;
  - timeline ~795-827;
  - `/runs` ~882-947;
  - `UsageRunRow` ~845-860 gains `kind`.
- Modify: `crates/rupu-cp/src/api/agents.rs:579-605`.
- Modify: `crates/rupu-cp/src/api/runs.rs:1087-1100` (`build_usage_timeline_json`
  → points from `run_usage`).
- Modify: `crates/rupu-cp/src/api/run_streams.rs:757-763, 800-812`: agent-run
  usage covers the own transcript plus recursive dispatch sub-runs.
- Modify: `crates/rupu-cp/src/api/sessions.rs:145-170, 462-477`.
- Create: `crates/rupu-cp/src/usage_sources.rs` — standalone and session
  transcript discovery for aggregates.
- Test: `crates/rupu-cp/tests/usage.rs` (existing integration suite — extend
  it).

**Interfaces:**
- Consumes: `run_usage`, `transcripts_usage`, `resolved_transcripts`,
  `summarize_run_usage` (Task 8).
- Produces:

```rust
// usage_sources.rs
pub enum SourceKind { Workflow, Agent, Session }
pub struct ExtraSource {            // a non-RunStore spend source
    pub kind: SourceKind,           // Agent | Session
    pub id: String,                 // run id (transcript key)
    pub session_id: Option<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub agent: String,
    pub paths: Vec<(String, PathBuf)>, // own transcript + recursive dispatch sub-runs
}
/// Standalone + session transcripts under `global`, EXCLUDING any path in `claimed`.
pub fn extra_sources(global: &Path, run_store: &RunStore, claimed: &HashSet<PathBuf>) -> Vec<ExtraSource>;
```

  `UsageRunRow.kind: &'static str` is `"workflow" | "agent" | "session"`,
  serialized as a string.

- [ ] **Step 1: Write the failing integration tests** in `crates/rupu-cp/tests/usage.rs`.
  Follow that file's existing app/`AppState` harness:
  1. `usage_endpoint_counts_inflight_step_from_ledger`: a RunStore run
     (status Running) whose `usage.jsonl` has 2 rows and no step_results →
     `/api/usage` summary `total_tokens` equals the rows' sum.
  2. `usage_endpoint_includes_standalone_and_session_transcripts_once`:
     - `<global>/transcripts/run_S.jsonl` (standalone, with `.meta.json`);
     - a session whose `session.json` `runs[0].transcript_path` names
       `<global>/transcripts/run_T.jsonl` (with a meta carrying
       `session_id`);
     - a workflow run whose step_results name `<global>/transcripts/run_W.jsonl`.

     Assert the total equals S + T + W (W counted once, not also as
     standalone). Assert `/api/usage/runs` has three rows with kinds
     `workflow` / `agent` / `session`.
  3. `run_usage_timeline_includes_inflight_points`: events name an in-flight
     transcript with 2 Usage events → `/api/runs/:id/usage-timeline` returns
     2 points labeled with that step.
  4. `session_usage_is_live_from_transcripts`:
     - the session's `session.json` totals are 0 (turn in flight);
     - the transcript has usage;
     - `GET /api/sessions/:id` usage equals the transcript sum.
  5. `agent_run_usage_includes_dispatch_children`: a standalone run
     `run_A.jsonl` plus `<runs>/run_A/sub/sub_1/transcript.jsonl`, both with
     usage → the agent-runs list row for `run_A` has the sum.

  Build every fixture with serialized `rupu_transcript::Event` values. Look up
  the session fixture shape in `crates/rupu-cp/tests/sessions_host.rs` /
  `sessions_live.rs` and copy it.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cp --test usage`
Expected: the 5 new tests FAIL.

- [ ] **Step 3: Implement**
  - `api/usage.rs` (all three sites):
    - Replace `run_transcript_paths` + `aggregate` with
      `let u = crate::usage::run_usage(&s.run_store, &r.id); let mut rows = u.rows.clone();`,
      keeping the inline `workflow` / `workspace_id` / `host_id` stamping.
    - Build `claimed: HashSet<PathBuf>` from `resolved_transcripts` of every
      listed run.
    - Call `extra_sources(&global, &s.run_store, &claimed)`. For each source,
      `transcripts_usage(&src.paths)`, and stamp rows with
      `workflow = ""`, `workspace_id`/`host_id` as available (`host_id = "local"`).
    - Apply the same time-window filter used for runs, on `src.started_at`.
    - `/api/usage/runs` emits a `UsageRunRow` per extra source, with
      `kind = "agent" | "session"`, `workflow_name = None`, and
      `run_id = src.id`.
    - Heavy loops run inside `tokio::task::spawn_blocking`. Clone the
      `Arc<RunStore>`, the pricing and the global dir into the closure.
  - `usage_sources.rs`:
    - Scan `global.join("transcripts")` non-recursively for `*.jsonl`,
      excluding `*.meta.json` and the `archive/` dir.
    - Read the sidecar meta (the partial-DTO pattern at
      `api/run_streams.rs:101-118`: `run_id`, `session_id`, `trigger_source`)
      and the agent/`started_at` from the transcript head
      (`rupu_transcript::JsonlReader::head` / the same helper
      `collect_standalone_runs` uses).
    - `session_id.is_some()` or `trigger_source == "session_turn"` ⇒ Session,
      else Agent.
    - Also walk every session's `session.json` `runs[].transcript_path` that is
      NOT under `<global>/transcripts`. Reuse the session store reader
      `api/sessions.rs` uses.
    - Paths = the own transcript plus
      `run_store.known_transcripts(<key>)`-style recursive dispatch sub-runs.
      Standalone runs' children live under `<runs>/<run_id>/sub/`, so call
      `run_store.sub_run_ids_recursive(key)` and
      `run_store.sub_run_transcript_path(...)`.
    - Skip any path in `claimed`.
  - `api/agents.rs:579-605` — per-agent rollup: replace the paths +
    `aggregate` with `run_usage(...).rows`.
  - `api/runs.rs::build_usage_timeline_json`:
    `serde_json::to_value(&crate::usage::run_usage(store, id).points)`.
    Keep the `store.load` 404 check.
  - `api/run_streams.rs:757-763, 800-812` (agent runs): replace
    `run_metrics_paths(&[tp], …)` with
    `transcripts_usage(&labeled)`, where `labeled` = the own transcript plus its
    recursive dispatch sub-runs (same helper as `usage_sources`). Map it to
    `RunMetrics` via `summarize_run_usage`, `u.turns` and `u.duration_ms`.
  - `api/sessions.rs::session_usage` (~145-170):
    - Fold `transcripts_usage` over `runs[].transcript_path` (label = run id)
      and price with `summarize_run_usage`. This replaces pricing the
      `session.json` totals against the session's model.
    - Keep `runs: 1` semantics by setting `summary.runs = 1`.
    - The timeline (~462-477) becomes `transcripts_usage(&labeled).points`.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-cp`
Expected: PASS (new + existing). Regenerate the macOS fixtures if
`UsageRunRow` is in them (`make macos-fixtures`), and commit them.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-cp apps/rupu-macos/Fixtures
git commit -m "feat(cp): every usage surface reads the fold; aggregates include standalone + session spend"
```

---

### Task 10: CLI — live view from the fold; `rupu run` live usage; plain-printer cost; one-shot callers

**Files:**
- Modify: `crates/rupu-cli/src/output/live_run.rs`:
  - the tick loop ~1580-1660;
  - `apply` `DispatchCompleted` arm ~605-625;
  - `add_active_tokens` / `accumulate_cost` ~669-714.
- Modify: `crates/rupu-cli/src/cmd/run.rs` (~900-962 tail loop), and
  `crates/rupu-cli/src/output/printer.rs` (~752 token line).
- Modify: `crates/rupu-cli/src/output/workflow_printer.rs` (~925, ~3335-3345,
  ~3574).
- Modify: `crates/rupu-cli/src/cmd/workflow.rs:2457`,
  `cmd/usage_report.rs:176-219`, `cmd/autoflow.rs:7160-7215`,
  `cmd/session.rs:1340` and `:7589-7645`.
- Test: `live_run.rs` inline tests (update the existing
  `add_active_tokens_*` tests); `crates/rupu-cli/tests/cli_usage.rs`.

**Interfaces:**
- Consumes: `rupu_cp::usage::{run_usage, summarize_run_usage, transcripts_usage}`,
  `rupu_cp::usage_index::{RunUsage, Tokens}`.
- Produces: `LiveRunState::apply_run_usage(&mut self, u: &RunUsage, pricing: &PricingConfig)`,
  which sets the run, step and unit tokens and the cost from the fold. Unit
  tokens are keyed by `(step_id, unit_index)` → the unit at that index.

- [ ] **Step 1: Write the failing tests** (in `live_run.rs`'s test module):

```rust
    #[test]
    fn apply_run_usage_sets_all_units_not_just_focused() {
        let mut state = fanout_state(true); // existing helper: step "assess" with 3 units
        let mut u = rupu_cp::usage_index::RunUsage::default();
        u.by_unit.insert(("assess".into(), 0), rupu_cp::usage_index::Tokens { input: 100, output: 10, cached: 0 });
        u.by_unit.insert(("assess".into(), 2), rupu_cp::usage_index::Tokens { input: 300, output: 30, cached: 0 });
        u.by_step.insert("assess".into(), vec![row("anthropic", "claude-x", 400, 40)]);
        u.rows = vec![row("anthropic", "claude-x", 400, 40)];
        state.apply_run_usage(&u, &rupu_config::PricingConfig::default());
        assert_eq!(state.steps[1].units[0].tokens, 110);
        assert_eq!(state.steps[1].units[2].tokens, 330);
        assert_eq!(state.tokens_in, 400);
        assert_eq!(state.tokens_out, 40);
    }

    #[test]
    fn dispatch_completed_no_longer_adds_tokens() {
        let mut state = fanout_state(true);
        let before = (state.tokens_in, state.tokens_out);
        state.apply(&WfEvent::DispatchCompleted { /* fields as the existing dispatch test builds them,
            tokens_in: 999, tokens_out: 999 */ });
        assert_eq!((state.tokens_in, state.tokens_out), before);
    }
```

(`row(...)` is a local helper building `rupu_transcript::UsageRow`. Match
`fanout_state`'s real field paths — `steps[1]` is `assess` in that fixture;
verify before asserting.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cli live_run`
Expected: FAIL (no `apply_run_usage`; dispatch still adds).

- [ ] **Step 3: Implement**
  1. `live_run.rs`:
     - Add `apply_run_usage`: set `self.tokens_in/out` from `u.rows` sums, each
       step's `tokens_in/out` from `u.by_step`, and each unit's `tokens` from
       `u.by_unit`. Set the run cost from
       `rupu_cp::usage::summarize(&u.rows, pricing).cost_usd`. It **replaces**
       the accumulated values (it is idempotent), and it replaces
       `accumulate_cost`'s state.
     - In the tick loop, once per second (keep an `Instant` last-refresh), call
       `rupu_cp::usage::run_usage(&store, &run_id)` and `apply_run_usage`.
       `store` is the `RunStore` the loop already loads `run.json` from.
     - Delete the `Event::Usage` branch that calls `add_active_tokens` /
       `accumulate_cost` in the transcript drain. Keep
       `map_transcript_event` → `push_activity` for the feed.
     - Delete the token lines from the `DispatchCompleted` arm (keep the status
       update).
     - Remove `add_active_tokens` / `accumulate_cost` and their tests if nothing
       else uses them. Otherwise leave them, `#[cfg(test)]`-free and unused, and
       let clippy decide (prefer deletion).
  2. `rupu run` (`cmd/run.rs`, tail loop ~900-962):
     - On each tailed transcript `Event::Usage`, accumulate in / out / cached
       and price the delta with `rupu_config::pricing::lookup(...)` +
       `cost_usd(...)`. Load the pricing config the same way `rupu usage` does.
     - Call a new
       `printer.usage_live(tokens_in, tokens_out, cost: Option<f64>)` that
       redraws a single status line `⇡{in} ⇣{out} · {cost}` via the existing
       `formatTokens`-equivalent helper in `printer.rs`. In non-TTY mode, print
       nothing until the end.
     - At the end, `printer.step_done` prints `N tokens · $X.XX` (cost when
       priced, `—` when not).
  3. `workflow_printer.rs` plain mode:
     - At each `StepCompleted`, print the step's `tokens · $cost` from
       `run_usage(store, run_id).by_step[step_id]` priced with
       `summarize(...)`.
     - At run end, print the run total with cost.
     - Keep the existing `total_tokens` accumulation from `RunComplete` only
       where no store is available (in-memory).
  4. One-shot callers:
     - `cmd/workflow.rs:2457` (run show) → `rupu_cp::usage::run_usage(&store, &id).rows`.
     - `cmd/usage_report.rs:190` → the same, for RunStore runs. Also make the
       standalone scan skip any path in `resolved_transcripts` of listed runs.
       This mirrors the CP's `claimed` rule and replaces its "skip referenced
       paths" logic.
     - `cmd/autoflow.rs::summarize_run_transcripts` → `summarize_run(store, id, pricing)`.
     - `cmd/session.rs:1340` → `rupu_cp::usage::transcripts_usage(&labeled).points`.
  5. Session cached (`cmd/session.rs:7589-7645`): take the turn's cached total
     from `RunResult.total_tokens_cached` (Task 3) instead of `live-usage.json`.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-cli`
Expected: PASS (update the `cli_usage.rs` expectations only where they encoded
"completed steps only"; note each such change in the commit message).

- [ ] **Step 5: Commit**

```bash
git add -A crates/rupu-cli
git commit -m "feat(cli): live view + run show/usage read the fold; rupu run shows live tokens+cost; plain printer prints cost"
```

---

### Task 11: Verification, real-data regression check, PR

**Files:** none new (the real-data check is a scratch script — **not** committed).

- [ ] **Step 1: Full suite + clippy**

```bash
cargo test -p rupu-transcript -p rupu-agent -p rupu-orchestrator -p rupu-cp -p rupu-cli 2>&1 | grep -E '^test result|FAILED|panicked'
cargo clippy -p rupu-transcript -p rupu-agent -p rupu-orchestrator -p rupu-cp -p rupu-cli --all-targets -- -D warnings
```

Expected: every suite `ok`, except failures already recorded in the baseline.
Clippy clean.

- [ ] **Step 2: Real-data regression (read-only).** Write a scratch test binary
  or an `#[ignore]`d test **in the scratchpad, not the repo**. It must:
  - iterate `~/.rupu/runs/*`;
  - for each run, compare `rupu_cp::usage::run_usage(&store, id)` total tokens
    against the direct sum of `Usage` events over every transcript path found
    in that run's `step_results.jsonl`, `events.jsonl` and `sub/` tree (the
    spec §1 measurement);
  - expect equality for every run with locally readable transcripts, and
    specifically the §1 table's corrected totals
    (`run_01M1NC9AN0V749FACZHQZQCPNT` → 118,163,026).

  Record the pass count and the four §1 runs' before/after numbers for the PR
  body. Do not commit any of it (public repo; private run data).

- [ ] **Step 3: Open the PR** (branch `claude/rupu-token-tracking-issue-c74877`).
  Push with an explicit refspec (`git push origin HEAD:refs/heads/<branch>`;
  the repo has `push.default=matching`), then
  `gh pr create --base main --title "fix(usage): per-run usage ledger — count every token, in-flight included"`.
  The body covers:
  - the §1 table (before/after);
  - what changed per crate;
  - the note that the live CP UI is Plan 2;
  - `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

---

## Self-review notes (plan author)

- **Spec coverage:**
  - §3.1–3.3 → Tasks 3, 4, 6, 7.
  - §3.4 → Task 6.
  - §4 → Tasks 1, 5, 8.
  - §5.1 → Tasks 8, 9.
  - §5.2 → Task 9.
  - §8 → Task 10.
  - The §1 honesty items (session cached, `UnitCompleted`) → Tasks 3, 6, 10.
  - §5.3, §5.4, §6, §7 are Plan 2; §9 is Plan 3.
- **Spec deviation (intentional, recorded in the spec):** the fold and the
  index live in `rupu-cp` (not `rupu-orchestrator`), because the CLI already
  depends on `rupu-cp`. The ledger is derived from `run_store` + `run_id` with
  no new `OrchestratorRunOpts` field.
