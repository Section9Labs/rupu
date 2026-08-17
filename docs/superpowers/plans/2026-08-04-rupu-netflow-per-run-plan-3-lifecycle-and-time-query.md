# Netflow Plan 3 — ledger lifecycle and time-range query

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give netflow ledgers the same retention story transcripts already have, and let the CP query flows by time range.

**Architecture:** `rupu netflow prune --older-than 30d` mirrors `rupu transcript prune`, reusing its duration parser. Because Plan 1 made ledgers per-run, a time-range query selects *files* before reading rows, so filtering is cheap rather than a full scan.

**Tech Stack:** Rust 2021, `chrono`, `clap`; React + TypeScript for the picker.

**Spec:** `docs/superpowers/specs/2026-08-04-rupu-netflow-per-run-ledger-design.md` §3.4 and §6
**Depends on:** Plan 1 (per-run ledgers). Independent of Plan 2.

## Global Constraints

- **Workspace deps only.** Versions pinned in the root `Cargo.toml`.
- `#![deny(clippy::all)]` and `unsafe_code = "forbid"` workspace-wide.
- **Reuse, do not reinvent.** `parse_retention_duration` (`crates/rupu-cli/src/cmd/retention.rs`) already parses `30d` / `12h` / `1w`. `rupu transcript prune` already defines the `--older-than` / `--dry-run` argument shape. Copy both; a second duration syntax in the same CLI would be a defect.
- **Absent bounds means everything.** A range query with no `from`/`to` returns the full set, so existing callers are unaffected.
- **`Fidelity` and `Option` semantics are untouched.** Filtering must never turn an unobservable value into a number.
- **Format ONLY files you touch, with `rustfmt --edition 2021 <path>`.** Never `cargo fmt`; never `rustfmt` a crate root or `mod.rs`.
- **Never use `git stash`** — the stash stack is shared across worktrees.
- **Run tests in the FOREGROUND.** No `pgrep -f` wait loops.

---

### Task 1: Time-range filtering in the ledger views

**Files:**
- Modify: `crates/rupu-netflow/src/ledger/views.rs`

**Interfaces:**
- Consumes: `FlowRecord`, `read_flows_and_dropped`.
- Produces:

```rust
pub struct TimeRange {
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

impl TimeRange {
    pub fn unbounded() -> Self;
    pub fn contains(&self, ts: chrono::DateTime<chrono::Utc>) -> bool;
}

pub fn read_flows_in_range(path: &Path, range: &TimeRange) -> std::io::Result<(Vec<FlowRecord>, u64)>;
```

Task 2's CLI and Task 3's API both call `read_flows_in_range`.

- [ ] **Step 1: Write the failing tests**

```rust
    fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn an_unbounded_range_contains_everything() {
        let r = TimeRange::unbounded();
        assert!(r.contains(at(0)));
        assert!(r.contains(at(1_900_000_000)));
    }

    #[test]
    fn bounds_are_inclusive_at_both_ends() {
        let r = TimeRange {
            from: Some(at(100)),
            to: Some(at(200)),
        };
        assert!(r.contains(at(100)), "from is inclusive");
        assert!(r.contains(at(200)), "to is inclusive");
        assert!(!r.contains(at(99)));
        assert!(!r.contains(at(201)));
    }

    #[test]
    fn a_half_open_range_bounds_only_the_side_it_names() {
        let from_only = TimeRange { from: Some(at(100)), to: None };
        assert!(!from_only.contains(at(99)));
        assert!(from_only.contains(at(1_900_000_000)));

        let to_only = TimeRange { from: None, to: Some(at(100)) };
        assert!(to_only.contains(at(0)));
        assert!(!to_only.contains(at(101)));
    }

    #[test]
    fn read_flows_in_range_filters_rows_and_keeps_the_dropped_count() {
        // The dropped count describes the whole file, not the window: a
        // record lost to overflow has no timestamp to filter on, and
        // hiding it because of a time filter would be exactly the silent
        // under-reporting this subsystem exists to prevent.
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("run-a.jsonl");
        // Write three Flow lines at t=100, t=150, t=250, plus a Dropped{count:4}.
        // (Build the lines with serde_json::to_string over LedgerLine, as the
        // existing tests in this file do.)

        let (flows, dropped) = read_flows_in_range(
            &path,
            &TimeRange { from: Some(at(120)), to: Some(at(200)) },
        )
        .unwrap();

        assert_eq!(flows.len(), 1, "only the t=150 flow is in range");
        assert_eq!(dropped, 4, "loss is reported regardless of the window");
    }

    #[test]
    fn read_flows_in_range_on_a_missing_file_is_empty_not_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (flows, dropped) =
            read_flows_in_range(&tmp.path().join("absent.jsonl"), &TimeRange::unbounded()).unwrap();
        assert!(flows.is_empty());
        assert_eq!(dropped, 0);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-netflow in_range`
Expected: FAIL to compile — `cannot find type TimeRange`.

- [ ] **Step 3: Implement**

```rust
/// An inclusive time window. Absent bounds mean unbounded on that side,
/// so `TimeRange::unbounded()` selects everything and existing callers
/// are unaffected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeRange {
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

impl TimeRange {
    pub fn unbounded() -> Self {
        Self::default()
    }

    pub fn contains(&self, ts: chrono::DateTime<chrono::Utc>) -> bool {
        self.from.is_none_or(|f| ts >= f) && self.to.is_none_or(|t| ts <= t)
    }
}

/// Read a ledger, keeping only flows inside `range`.
///
/// The `Dropped` total is NOT filtered: a record lost to channel overflow
/// has no timestamp to test, and suppressing the count because of a time
/// window would silently under-report loss — the exact defect this
/// subsystem exists to prevent.
pub fn read_flows_in_range(
    path: &Path,
    range: &TimeRange,
) -> std::io::Result<(Vec<FlowRecord>, u64)> {
    let (flows, dropped) = read_flows_and_dropped(path)?;
    Ok((
        flows.into_iter().filter(|f| range.contains(f.ts)).collect(),
        dropped,
    ))
}
```

If `Option::is_none_or` is unavailable on the pinned toolchain, use `map_or(true, ...)` and note the substitution.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-netflow` then `cargo build --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/ledger/views.rs
git status --porcelain
git add crates/rupu-netflow/src/ledger/views.rs
git commit -m "feat(netflow): time-range filtering over a ledger"
```

---

### Task 2: `rupu netflow prune`

**Files:**
- Create: `crates/rupu-cli/src/cmd/netflow.rs`
- Modify: `crates/rupu-cli/src/cmd/mod.rs`, and the clap dispatcher that registers subcommands (find it with `grep -rn "Transcript(" crates/rupu-cli/src/`)

**Interfaces:**
- Consumes: `paths::netflow_dir` and `archived_netflow_dir` (Plan 1 Task 2), `parse_retention_duration`.
- Produces: `rupu netflow prune --older-than <DURATION> [--dry-run]`.

**Read `crates/rupu-cli/src/cmd/transcript.rs`'s `PruneArgs` and its `prune` implementation first.** Mirror the argument names exactly — `--older-than`, `--dry-run` — so the two commands do not diverge in feel. `rupu-cli` is a thin dispatcher by architecture rule 2: arg parsing plus delegation, no business logic.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn prune_removes_ledgers_older_than_the_cutoff_and_keeps_newer_ones() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let old = dir.join("run-old.jsonl");
        let new = dir.join("run-new.jsonl");
        std::fs::write(&old, "{}\n").unwrap();
        std::fs::write(&new, "{}\n").unwrap();
        // Backdate `old` well past the cutoff. filetime is already a
        // workspace dep used by other tests — check, and if it is not,
        // set the mtime with std::fs::File::set_modified.

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert_eq!(removed.len(), 1);
        assert!(!old.exists(), "the stale ledger is gone");
        assert!(new.exists(), "the recent ledger survives");
    }

    #[test]
    fn dry_run_reports_without_deleting() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let old = dir.join("run-old.jsonl");
        std::fs::write(&old, "{}\n").unwrap();
        // Backdate as above.

        let removed = prune_ledgers(dir, chrono::Duration::days(30), true).unwrap();

        assert_eq!(removed.len(), 1, "still reported");
        assert!(old.exists(), "but not deleted");
    }

    #[test]
    fn prune_ignores_non_ledger_files() {
        // The netflow dir also holds the self-ignoring .gitignore and an
        // archive/ subdirectory. Neither is a ledger.
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join(".gitignore"), "*\n").unwrap();
        std::fs::create_dir_all(dir.join("archive")).unwrap();
        // Backdate both well past the cutoff.

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert!(removed.is_empty());
        assert!(dir.join(".gitignore").exists());
        assert!(dir.join("archive").is_dir());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli prune_removes_ledgers`
Expected: FAIL to compile — `cannot find function prune_ledgers`.

- [ ] **Step 3: Implement**

Write `prune_ledgers(dir: &Path, older_than: chrono::Duration, dry_run: bool) -> anyhow::Result<Vec<PathBuf>>`, selecting only `*.jsonl` files directly in `dir` whose mtime is older than the cutoff. Skip directories and any non-`.jsonl` entry — the `.gitignore` and `archive/` must survive, which is what the third test pins.

Then the clap surface, mirroring `transcript.rs`:

```rust
#[derive(ClapArgs, Debug)]
pub struct PruneArgs {
    /// Retention cutoff, e.g. `30d`, `12h`, or `1w`.
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Preview deletions without removing files.
    #[arg(long)]
    pub dry_run: bool,
}
```

parsing the duration with `crate::cmd::retention::parse_retention_duration`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-cli prune` then `cargo build --workspace`
Expected: PASS.

- [ ] **Step 5: Check it by hand**

```bash
cargo run -p rupu-cli -- netflow prune --older-than 30d --dry-run
```
Expected: it runs and reports without deleting. If you have no ledgers yet, it should say so cleanly rather than erroring.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/netflow.rs
git status --porcelain
git add crates/rupu-cli/
git commit -m "feat(cli): rupu netflow prune mirroring transcript prune"
```

---

### Task 3: `?from=` / `?to=` on the netflow API

**Files:**
- Modify: `crates/rupu-cp/src/api/netflow.rs`
- Test: `crates/rupu-cp/tests/netflow_api.rs`

**Interfaces:**
- Consumes: `TimeRange` and `read_flows_in_range` (Task 1).
- Produces: every netflow route accepts optional `from` and `to` RFC 3339 query parameters. Task 4's UI sends them.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn a_range_query_returns_only_flows_inside_the_window() {
    // Drive the real route with ?from=&to= and assert the out-of-window
    // flow is absent and the in-window one present.
}

#[tokio::test]
async fn absent_bounds_return_everything() {
    // No from/to → same result as before the feature existed.
}

#[tokio::test]
async fn a_malformed_timestamp_is_a_400_not_a_panic_and_not_a_silent_ignore() {
    // ?from=not-a-date must be rejected. Silently ignoring it would show
    // the user unfiltered data while their filter appears applied — worse
    // than an error.
}
```

Model the setup on the existing tests in this file.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp a_range_query`
Expected: FAIL — the route ignores the parameters.

- [ ] **Step 3: Implement**

Add a query struct deserialising `from`/`to` as `Option<String>`, parse each with `chrono::DateTime::parse_from_rfc3339` into `TimeRange`, and return `400` on a parse failure rather than defaulting to unbounded. Thread the range into every scope's read, replacing `read_flows_and_dropped` with `read_flows_in_range`.

Keep the rollup and graph derived from the **filtered** set, so the summary and topology agree with the table.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-cp` then `cargo build --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/api/netflow.rs
git status --porcelain
git add crates/rupu-cp/
git commit -m "feat(cp): time-range query on the netflow endpoints"
```

---

### Task 4: The time picker

**Files:**
- Create: `crates/rupu-cp/web/src/components/netflow/TimeRangePicker.tsx`, `TimeRangePicker.test.tsx`
- Modify: `crates/rupu-cp/web/src/lib/netflow.ts`, `pages/Netflow.tsx`, `components/project/ProjectNetworkTab.tsx`, `pages/RunDetail.tsx`

**Interfaces:**
- Consumes: the API from Task 3.
- Produces: `<TimeRangePicker value onChange />` and fetch helpers accepting an optional range.

Relative is the default because *"what did this reach in the last hour"* is the question actually asked; absolute is the fallback for when someone is investigating a specific incident.

- [ ] **Step 1: Write the failing test**

```tsx
describe('TimeRangePicker', () => {
  it('offers relative presets and defaults to all', () => {
    render(<TimeRangePicker value={{ preset: 'all' }} onChange={() => {}} />);
    expect(screen.getByRole('button', { name: /last hour/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /24h/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /7d/i })).toBeInTheDocument();
  });

  it('emits an absolute from/to when a custom range is entered', async () => {
    const onChange = vi.fn();
    render(<TimeRangePicker value={{ preset: 'all' }} onChange={onChange} />);
    // switch to custom, fill both fields, assert onChange carries RFC 3339
  });

  it('does not emit bounds for the all preset', async () => {
    // "all" must produce no from/to so the API returns everything.
  });
});
```

Include the jsdom boilerplate the other netflow component tests use — `vite.config.ts` defaults to the `node` environment, so a component test without it will not run.

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/netflow/TimeRangePicker.test.tsx`
Expected: FAIL — cannot resolve the module.

- [ ] **Step 3: Implement**

Build the picker with the repo's existing primitives, as the other netflow components do — read `NetflowSummary.tsx` for which ones and which Tailwind tokens. Do not hand-roll markup or invent CSS variables; earlier tasks in this arc were sent back for exactly that.

Extend the fetch helpers to take an optional range and append `from`/`to` only when present, then mount the picker on all three surfaces and thread its value into the fetches.

- [ ] **Step 4: Run to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run` then `npx tsc --noEmit`
Expected: PASS across the whole suite — this touches shared fetch helpers, so nothing may regress.

- [ ] **Step 5: Commit**

```bash
git status --porcelain
git add crates/rupu-cp/web/
git commit -m "feat(cp-web): time-range picker on the netflow surfaces"
```

---

## Done when

- `cargo test --workspace` and the full web suite pass.
- `rupu netflow prune --older-than 30d --dry-run` reports without deleting; without `--dry-run` it removes stale ledgers and leaves `.gitignore` and `archive/` alone.
- A range query returns only in-window flows, absent bounds return everything, and a malformed timestamp is a 400 rather than a silent unfiltered result.
- The summary and graph reflect the same filtered set as the table.
