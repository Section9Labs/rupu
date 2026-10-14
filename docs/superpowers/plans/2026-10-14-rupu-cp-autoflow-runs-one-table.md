# CP Autoflow Runs as One Table — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Runs → Autoflows' three tables (Runs / Cycles / Claims) with one Workflows-shaped run table — autoflow runs plus failed attempts, newest first — a problems strip above it, and claim actions on the run page.

**Architecture:** Every run row learns whether an autoflow launched it (`RunListRow.autoflow`, read from the run's `run_envelope.json` through a `FileCache`). A new endpoint lists the runs whose `autoflow` is set, merged with failed-attempt rows synthesized from the autoflow history; a second endpoint groups history failures that name no workflow. The web page reuses the Workflows columns and per-host loading.

**Tech Stack:** Rust (axum, serde, tokio) in `rupu-orchestrator`, `rupu-runtime`, `rupu-cp`, `rupu-cli`; React + TypeScript + vitest in `crates/rupu-cp/web`.

**Spec:** `docs/superpowers/specs/2026-10-14-rupu-cp-autoflow-runs-one-table-design.md`

## Global Constraints

- Workspace rules (CLAUDE.md): deps only in the root `Cargo.toml`; `#![deny(clippy::all)]`; no `unsafe`; `rupu-cli` stays thin (no business logic — it calls `rupu-cp` helpers).
- Integration tests: ONE binary per crate (`crates/<c>/tests/it/`); in-crate `#[cfg(test)] mod tests` is fine and is what `api/runs.rs` / `api/run_streams.rs` already use.
- Never run package-wide `cargo fmt`; format only the files you touched (`rustfmt --edition 2021 <file>`).
- Run only the tests you touched (`cargo test -p <crate> <module>::`), never a cold full-workspace `cargo test`.
- Web: use the CP's existing components (`SortableTable`, `FilterBar`, `FilterPills`, `SearchInput`, `HostSelect`, `StatusPill`, `CrewChip`, `Button`, `ErrorBanner`, `EmptyState`, `Spinner`, `PerHostStrip`/`PerHostFooter`, `usePerHostPagedList`). No new styling primitives.
- Git: never `git stash` / `git stash pop`; commit with `-c commit.gpgsign=false` if signing fails; end commit messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- `RunListRow` is the JSON contract of `rupu run list` and SSH `list_runs` — never hand-roll a parallel row shape.
- Wire compatibility: `/api/runs/autoflows` (cycles) and `/api/runs/autoflows/events` keep their shapes (one additive optional field on events, Task 6).

## Review Focus

1. A local run with NO `run_envelope.json` (runs from before envelopes existed) must list with `autoflow: null`, never crash or omit the row — Task 2 test `no_envelope_local_run_is_not_autoflow`.
2. A corrupt/unparseable `run_envelope.json` must not fail the whole listing — the row omits the `autoflow` key and a warning is logged — Task 2 test `unreadable_envelope_omits_the_key`.
3. A history `run_launched` event whose run DOES exist must not appear twice (once as a run, once as an attempt) — Task 5 test `attempt_with_recorded_run_is_not_listed`.
4. Paging after the merge: offset/limit cut the merged run+attempt list, so page 2 starts where page 1 ended even when attempts interleave — Task 6 test `merged_list_pages_after_merge`.
5. A remote host whose rows lack the `autoflow` key (older rupu) is a 501 on `?host=<it>`, shown "unavailable" by the web — Task 6 test `remote_without_autoflow_key_is_501`, Task 8 test `unavailable host slice`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-orchestrator/src/runs.rs` | `RunStore::read_run_envelope_cached` (Task 1) |
| `crates/rupu-cp/src/api/runs.rs` | `RunAutoflow`, `RunListRow.autoflow`, filled in `RunListRow::priced`; `read_filtered_runs` generalized to a row predicate (Tasks 2, 6) |
| `crates/rupu-cp/src/api/autoflow_claims.rs` | `ClaimIndex` — load claims once, fill a row's claim fields (Task 3) |
| `crates/rupu-cp/src/host/local.rs` | local `list_runs` applies `ClaimIndex` (Task 3) |
| `crates/rupu-cli/src/cmd/run.rs` | `rupu run list` applies `ClaimIndex` (Task 3) |
| `crates/rupu-runtime/src/autoflow_history.rs` | `AutoflowHistoryStore::list_events_between` (Task 4) |
| `crates/rupu-cp/src/api/autoflow_runs.rs` (new) | pure synthesis: attempt rows, problems; the two endpoints (Tasks 5, 6) |
| `crates/rupu-cp/src/api/run_resolve.rs` | `map_claim_status_str` → `pub(crate)` (Task 5) |
| `crates/rupu-cp/src/api/run_streams.rs` | `AutoflowEventRow.repo_ref` (Task 6) |
| `crates/rupu-cp/src/api/mod.rs` | register `autoflow_runs::routes()` (Task 6) |
| `crates/rupu-cp/web/src/lib/api.ts` | types + `getAutoflowRunList` / `getAutoflowProblems` (Task 7) |
| `crates/rupu-cp/web/src/components/runs/workflowRunColumns.tsx` (new) | Workflows columns shared by both pages (Task 7) |
| `crates/rupu-cp/web/src/pages/runs/WorkflowRuns.tsx` | import the shared columns (Task 7) |
| `crates/rupu-cp/web/src/components/runs/AutoflowProblems.tsx` (new) | the problems strip (Task 8) |
| `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx` | rewritten one-table page (Task 8) |
| `crates/rupu-cp/web/src/components/AutoflowPanel.tsx` | Requeue / Release (Task 9) |
| `crates/rupu-cp/web/src/pages/RunDetail.tsx` | pass `host` to `AutoflowPanel` (Task 9) |

**Delivery:** PR 1 = Tasks 1–3 (the other session rebases onto `RunListRow.autoflow`). PR 2 = Tasks 4–9.

**Parallelism:** Tasks 1, 4, 7, 9 have no dependencies on each other. 2 needs 1. 3 needs 2. 5 needs 2, 3, 4. 6 needs 5. 8 needs 7 (and the wire shapes from 2/5/6, which are fixed below).

---

### Task 1: Cached envelope read

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs` (next to `read_run_envelope` ~L2339 and `record_cache` ~L4473)
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `pub fn read_run_envelope_cached(&self, run_id: &str) -> Result<Option<std::sync::Arc<rupu_runtime::RunEnvelope>>, RunStoreError>` — `Ok(None)` when the run has no `run_envelope.json`; `Err` on an unreadable or unparseable file.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn read_run_envelope_cached_none_when_missing_and_value_when_present() {
    let tmp = TempDir::new().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    let rec = sample_record("run_env_cached"); // existing fixture in this module
    store.create(rec.clone(), "name: x\n").unwrap();
    assert!(store.read_run_envelope_cached(&rec.id).unwrap().is_none());

    let envelope = sample_envelope(&rec.id); // existing fixture used by write_run_envelope tests (~L4700)
    store.write_run_envelope(&rec.id, &envelope).unwrap();
    let got = store.read_run_envelope_cached(&rec.id).unwrap().expect("present");
    assert_eq!(got.run_id, rec.id);
}

#[test]
fn read_run_envelope_cached_errors_on_garbage() {
    let tmp = TempDir::new().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    let rec = sample_record("run_env_garbage");
    store.create(rec.clone(), "name: x\n").unwrap();
    std::fs::write(store.run_envelope_path(&rec.id), b"{not json").unwrap();
    assert!(store.read_run_envelope_cached(&rec.id).is_err());
}
```

(If the module's fixtures are named differently, use the ones the existing `write_run_envelope` tests at ~L4700 use.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-orchestrator read_run_envelope_cached`
Expected: FAIL — `no method named read_run_envelope_cached`.

- [ ] **Step 3: Implement**

```rust
/// [`Self::read_run_envelope`] through a process-wide [`FileCache`]: a
/// listing that asks every run for its envelope costs a `stat` per run once
/// the files have been read. `Ok(None)` when the run has none.
pub fn read_run_envelope_cached(
    &self,
    run_id: &str,
) -> Result<Option<std::sync::Arc<RunEnvelope>>, RunStoreError> {
    match envelope_cache().read(&self.run_envelope(run_id), |body| {
        serde_json::from_slice::<RunEnvelope>(body)
    }) {
        Ok(v) => Ok(v),
        Err(FileCacheError::Io(e)) => Err(e.into()),
        Err(FileCacheError::Parse(e)) => Err(e.into()),
    }
}
```

and beside `record_cache`:

```rust
/// Every `run_envelope.json` parse, process-wide (run lists ask each run).
fn envelope_cache() -> &'static FileCache<RunEnvelope> {
    static CACHE: std::sync::OnceLock<FileCache<RunEnvelope>> = std::sync::OnceLock::new();
    CACHE.get_or_init(FileCache::default)
}
```

- [ ] **Step 4: Run to verify it passes** — same command, expected PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/runs.rs
git commit -m "runs: cached run_envelope read for listings"
```

---

### Task 2: `RunListRow.autoflow`

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs` (`RunListRow` ~L787, `From<&RunRecord>` ~L815, `priced` ~L853)
- Test: `crates/rupu-cp/src/api/runs.rs` `mod tests` (~L2410)

**Interfaces:**
- Consumes: `RunStore::read_run_envelope_cached` (Task 1).
- Produces:
  ```rust
  #[derive(serde::Serialize, Clone, Debug, Default, PartialEq)]
  pub struct RunAutoflow {
      #[serde(skip_serializing_if = "Option::is_none")] pub issue_ref: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none")] pub issue_display_ref: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none")] pub issue_url: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none")] pub pr_url: Option<String>,
  }
  // on RunListRow:
  /// `None` = this host can't tell (no key on the wire); `Some(None)` = not
  /// launched by an autoflow (`null`); `Some(Some(_))` = an autoflow run.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub autoflow: Option<Option<RunAutoflow>>,
  pub fn autoflow_of(r: &RunRecord, store: &RunStore) -> Option<Option<RunAutoflow>>;
  ```
  Wire: key absent | `null` | `{issue_ref?, issue_display_ref?, issue_url?, pr_url?}`.

Rules for `autoflow_of`:
- envelope read `Ok(Some(env))` → `Some(env.autoflow.as_ref().map(|a| RunAutoflow { issue_ref: a.claim_id.clone().or_else(|| r.issue_ref.clone()), ..Default::default() }))`
- `Ok(None)` and `r.worker_id.is_none()` (a local run) → `Some(None)`
- `Ok(None)` and `r.worker_id.is_some()` (a mirrored worker run whose envelope this store may not hold) → `None`
- `Err(e)` → `tracing::warn!(run_id = %r.id, error = %e, "run envelope unreadable; autoflow unknown")` and `None`

- [ ] **Step 1: Write the failing tests** (in `runs.rs` `mod tests`; `terminal_record(id)` is the module's existing fixture)

```rust
fn autoflow_envelope(run_id: &str, claim: Option<&str>) -> rupu_runtime::RunEnvelope {
    use rupu_runtime::*;
    RunEnvelope {
        version: RunEnvelope::VERSION,
        run_id: run_id.into(),
        kind: RunKind::WorkflowRun,
        workflow: WorkflowBinding { name: "fix-issue".into(), source_path: "x.yaml".into(), fingerprint: "sha256:t".into() },
        repo: None,
        trigger: RunTrigger { source: RunTriggerSource::Autoflow, wake_id: None, event_id: None },
        inputs: Default::default(),
        context: None,
        execution: ExecutionRequest { backend: None, permission_mode: "bypass".into(), workspace_strategy: None, strict_templates: false, attach_ui: false },
        autoflow: Some(AutoflowEnvelope { name: "fix-issue".into(), claim_id: claim.map(Into::into), priority: 0 }),
        correlation: None,
        worker: None,
    }
}

fn listed(s: &AppState, id: &str) -> serde_json::Value {
    let rec = s.run_store.load(id).unwrap();
    let row = RunListRow::priced(&rec, None, &s.run_store, &mut crate::customers::FlatPricing(&s.pricing));
    serde_json::to_value(row).unwrap()
}

#[test]
fn autoflow_run_carries_its_issue() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = test_state(&tmp);
    s.run_store.create(terminal_record("run_af"), "name: x\n").unwrap();
    s.run_store.write_run_envelope("run_af", &autoflow_envelope("run_af", Some("github:acme/api/issues/7"))).unwrap();
    assert_eq!(listed(&s, "run_af")["autoflow"], serde_json::json!({"issue_ref": "github:acme/api/issues/7"}));
}

#[test]
fn manual_run_with_envelope_is_null() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = test_state(&tmp);
    s.run_store.create(terminal_record("run_m"), "name: x\n").unwrap();
    let mut env = autoflow_envelope("run_m", None);
    env.autoflow = None;
    s.run_store.write_run_envelope("run_m", &env).unwrap();
    let v = listed(&s, "run_m");
    assert!(v.as_object().unwrap().contains_key("autoflow"));
    assert_eq!(v["autoflow"], serde_json::Value::Null);
}

#[test]
fn no_envelope_local_run_is_not_autoflow() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = test_state(&tmp);
    s.run_store.create(terminal_record("run_old"), "name: x\n").unwrap();
    let v = listed(&s, "run_old");
    assert!(v.as_object().unwrap().contains_key("autoflow"));
    assert_eq!(v["autoflow"], serde_json::Value::Null);
}

#[test]
fn mirrored_run_without_envelope_omits_the_key() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = test_state(&tmp);
    let mut rec = terminal_record("run_mirror");
    rec.worker_id = Some("node_a".into());
    s.run_store.create(rec, "name: x\n").unwrap();
    assert!(!listed(&s, "run_mirror").as_object().unwrap().contains_key("autoflow"));
}

#[test]
fn unreadable_envelope_omits_the_key() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = test_state(&tmp);
    s.run_store.create(terminal_record("run_bad"), "name: x\n").unwrap();
    std::fs::write(s.run_store.run_envelope_path("run_bad"), b"{nope").unwrap();
    assert!(!listed(&s, "run_bad").as_object().unwrap().contains_key("autoflow"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cp api::runs::tests::autoflow_ api::runs::tests::manual_run_with_envelope api::runs::tests::no_envelope api::runs::tests::mirrored_run_without api::runs::tests::unreadable_envelope`
Expected: FAIL to compile (`RunAutoflow` / `autoflow` missing).

- [ ] **Step 3: Implement** — add the struct and field above; `From<&RunRecord>` sets `autoflow: None` (it is the pure, store-free conversion); in `priced`, after `with_usage`:

```rust
row.autoflow = autoflow_of(r, store);
```

and the free function `autoflow_of` with the four rules above. Fix every other `RunListRow { .. }` literal the compiler flags (tests included) with `autoflow: None`.

- [ ] **Step 4: Run to verify they pass** — same command, PASS. Then `cargo test -p rupu-cp api::runs::` to catch row-shape snapshots that now see the key (update an expected JSON only where the new `"autoflow": null` is the sole difference).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/runs.rs
git commit -m "cp: run rows say whether an autoflow launched them (RunListRow.autoflow)"
```

---

### Task 3: Claim fields on autoflow rows (`ClaimIndex`)

**Files:**
- Modify: `crates/rupu-cp/src/api/autoflow_claims.rs`
- Modify: `crates/rupu-cp/src/host/local.rs` (`list_runs` ~L208)
- Modify: `crates/rupu-cli/src/cmd/run.rs` (`list` ~L780)
- Test: `crates/rupu-cp/src/api/autoflow_claims.rs` (add `#[cfg(test)] mod tests` if absent)

**Interfaces:**
- Consumes: `RunListRow.autoflow`, `RunAutoflow` (Task 2).
- Produces:
  ```rust
  pub struct ClaimIndex { by_issue: std::collections::HashMap<String, AutoflowClaimRecord> }
  impl ClaimIndex {
      /// The claims under `<global>/autoflows/claims`; an unreadable store is
      /// an empty index plus one warning (a listing never fails on claims).
      pub fn load(global: &std::path::Path) -> Self;
      /// Fill `row.autoflow`'s issue_display_ref / issue_url from the claim for
      /// its issue_ref, and pr_url when that claim's last_run_id is this row.
      pub fn apply(&self, row: &mut crate::api::runs::RunListRow);
      /// Same, on an already-serialized row (`id` + `autoflow.issue_ref`).
      pub fn apply_json(&self, row: &mut serde_json::Value);
  }
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn claim_index_fills_display_url_and_pr_for_the_claims_last_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = AutoflowClaimStore { root: tmp.path().join("autoflows").join("claims") };
    let mut claim = AutoflowClaimRecord::new_for_test("github:acme/api/issues/7"); // see note
    claim.issue_display_ref = Some("acme/api#7".into());
    claim.issue_url = Some("https://example.test/7".into());
    claim.pr_url = Some("https://example.test/pr/9".into());
    claim.last_run_id = Some("run_latest".into());
    store.save(&claim).unwrap();
    let idx = ClaimIndex::load(tmp.path());

    let mut latest = serde_json::json!({"id": "run_latest", "autoflow": {"issue_ref": "github:acme/api/issues/7"}});
    idx.apply_json(&mut latest);
    assert_eq!(latest["autoflow"]["issue_display_ref"], "acme/api#7");
    assert_eq!(latest["autoflow"]["issue_url"], "https://example.test/7");
    assert_eq!(latest["autoflow"]["pr_url"], "https://example.test/pr/9");

    let mut older = serde_json::json!({"id": "run_older", "autoflow": {"issue_ref": "github:acme/api/issues/7"}});
    idx.apply_json(&mut older);
    assert_eq!(older["autoflow"]["issue_display_ref"], "acme/api#7");
    assert!(older["autoflow"].get("pr_url").is_none(), "a PR belongs to the claim's last run only");

    let mut manual = serde_json::json!({"id": "run_m", "autoflow": null});
    idx.apply_json(&mut manual);
    assert_eq!(manual["autoflow"], serde_json::Value::Null);
}
```

Note: build the `AutoflowClaimRecord` the way the existing claim tests in `rupu-workspace` (or `api/run_resolve.rs` tests ~L700) do — copy their literal; `new_for_test` is a placeholder name for "however those tests build one", not a new API.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp api::autoflow_claims::tests`
Expected: FAIL — `ClaimIndex` not found.

- [ ] **Step 3: Implement** `ClaimIndex` (`load` uses `AutoflowClaimStore { root: global.join("autoflows").join("claims") }.list()`; on `Err` → `tracing::warn!` + empty). `apply` delegates to the same logic as `apply_json` on the typed struct. Then:
  - `host/local.rs` `list_runs`: after `query_run_rows` returns rows, `let claims = ClaimIndex::load(&global); for r in &mut rows { claims.apply(r) }` inside the same `blocking_host` closure.
  - `rupu-cli` `cmd/run.rs` `list`: after building `rows`, `let claims = rupu_cp::api::autoflow_claims::ClaimIndex::load(&global); for r in &mut rows { claims.apply(r) }` (make `rows` `mut`; make the module `pub` in `api/mod.rs` if it isn't).

- [ ] **Step 4: Run to verify it passes** — `cargo test -p rupu-cp api::autoflow_claims::tests` PASS; `cargo build -p rupu-cli` succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/autoflow_claims.rs crates/rupu-cp/src/host/local.rs crates/rupu-cli/src/cmd/run.rs crates/rupu-cp/src/api/mod.rs
git commit -m "cp: autoflow rows carry their issue link and PR from the claim store"
```

**PR 1 ends here.** Open it (see "Delivery" in the spec) and message the unified-filter session the link.

---

### Task 4: History range read

**Files:**
- Modify: `crates/rupu-runtime/src/autoflow_history.rs` (next to `list_recent_events` ~L255)
- Test: same file's `mod tests`

**Interfaces:**
- Produces: `pub fn list_events_between(&self, since: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> Result<Vec<AutoflowHistoryEventRecord>, AutoflowHistoryStoreError>` — every event with `since <= at < until` (either bound open when `None`), newest first by `(at, event_id)`. Only day directories whose date can hold a match are read; parses go through the same `FileCache` `list_recent_events` uses (hoist its `static CACHE` into a `fn event_cache()`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn list_events_between_reads_only_the_range_across_days() {
    use chrono::TimeZone;
    let tmp = tempfile::TempDir::new().unwrap();
    let store = AutoflowHistoryStore::new(tmp.path().to_path_buf());
    let day = |d: u32, h: u32| Utc.with_ymd_and_hms(2026, 8, d, h, 0, 0).unwrap();
    let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, day(1, 0));
    for (d, h) in [(1, 12), (2, 1), (2, 23), (3, 5)] {
        store.append_cycle_event(&cycle, AutoflowCycleEvent { kind: AutoflowCycleEventKind::CycleFailed, ..Default::default() }, day(d, h)).unwrap();
    }
    let got = store.list_events_between(Some(day(2, 0)), Some(day(3, 0))).unwrap();
    let ats: Vec<_> = got.iter().map(|e| e.at.clone()).collect();
    assert_eq!(ats, vec![day(2, 23).to_rfc3339(), day(2, 1).to_rfc3339()]);
    assert_eq!(store.list_events_between(None, None).unwrap().len(), 4);
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p rupu-runtime autoflow_history::tests::list_events_between`; FAIL (no method).

- [ ] **Step 3: Implement** — iterate `self.event_day_dirs()?` (newest first), skip a day dir whose `YYYY-MM-DD` name parses to a date after `until`'s date or before `since`'s date (a dir name that doesn't parse is read, not skipped), read its `*.json` files through `event_cache()` exactly as `newest_records` does (skip `.tmp`, skip a file deleted mid-walk), keep records whose parsed `at` is in range, sort by `(at, event_id)` descending.

- [ ] **Step 4: Run to verify it passes** — PASS. Also run `cargo test -p rupu-runtime autoflow_history::` (the cache hoist must not break `list_recent_events`).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-runtime/src/autoflow_history.rs
git commit -m "autoflow history: read events for a time range"
```

---

### Task 5: Attempt rows and problems (pure synthesis)

**Files:**
- Create: `crates/rupu-cp/src/api/autoflow_runs.rs`
- Modify: `crates/rupu-cp/src/api/run_resolve.rs` (`fn map_claim_status_str` → `pub(crate) fn`)
- Modify: `crates/rupu-cp/src/api/mod.rs` (`pub mod autoflow_runs;`)
- Test: `crates/rupu-cp/src/api/autoflow_runs.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `RunAutoflow` (Task 2), `ClaimIndex` (Task 3), `AutoflowHistoryEventRecord` + `list_events_between` (Task 4), `run_resolve::map_claim_status_str`.
- Produces:
  ```rust
  #[derive(serde::Serialize, Clone, Debug, PartialEq)]
  pub struct AttemptInfo { pub detail: Option<String>, pub cycle_id: String, pub worker: Option<String>, pub claim_status: Option<String> }

  /// A failed attempt in RunListRow's shape (so the web renders it with the same columns).
  #[derive(serde::Serialize, Clone, Debug)]
  pub struct AttemptRow {
      pub id: String,                       // event's run_id, else its event_id
      pub workflow_name: String,
      pub status: rupu_orchestrator::runs::RunStatus,
      pub started_at: chrono::DateTime<chrono::Utc>,
      pub finished_at: Option<chrono::DateTime<chrono::Utc>>,   // = started_at
      pub trigger: &'static str,            // "event" when the event has a wake_event_id, else "cron"
      pub usage: crate::usage::UsageSummary, // default
      pub turns: u64,                       // 0
      pub duration_ms: Option<u64>,         // None
      pub codename: String,                 // crate::codename::named(None, &id, None)
      pub codename_derived: bool,
      pub autoflow: Option<crate::api::runs::RunAutoflow>, // Some(issue_ref = event.issue_ref, claim fields via ClaimIndex)
      pub attempt: AttemptInfo,
      #[serde(skip_serializing_if = "Option::is_none")] pub run_recorded: Option<bool>, // Some(false) when id is a run id with no record
  }

  /// Attempts among `events` (any order in, newest first out):
  /// - `CycleFailed` with `workflow: Some(_)` → status Failed
  /// - `RunLaunched` with `run_id: Some(id)` where `!run_exists(id)` (and `check_launched`) → status `map_claim_status_str(event.status or "")`
  /// An event whose run_id `run_exists` is never an attempt.
  pub fn attempt_rows(events: &[AutoflowHistoryEventRecord], run_exists: &dyn Fn(&str) -> bool, check_launched: bool, claims: &ClaimIndex) -> Vec<AttemptRow>;

  #[derive(serde::Serialize, Clone, Debug, PartialEq)]
  pub struct Problem { pub host_id: String, pub subject_kind: &'static str /* "repo" | "worker" */, pub subject: String, pub detail: Option<String>, pub first_seen: String, pub last_seen: String, pub count: usize }

  /// `CycleFailed` events with no workflow at or after `cutoff`, grouped by
  /// (repo_ref) else (worker_name, else worker_id, else "autoflow worker"); detail = the newest event's.
  pub fn problems(events: &[ProblemEvent], host_id: &str, cutoff: chrono::DateTime<chrono::Utc>) -> Vec<Problem>;
  /// The fields `problems` needs, from a local record or a remote event row JSON.
  pub struct ProblemEvent { pub at: String, pub repo_ref: Option<String>, pub worker: Option<String>, pub detail: Option<String> }
  ```
  Problems sort by `last_seen` descending.

- [ ] **Step 1: Write the failing tests**

```rust
use super::*;
use rupu_runtime::{AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord, AutoflowHistoryEventRecord};

fn ev(kind: AutoflowCycleEventKind, at: &str, f: impl FnOnce(&mut AutoflowCycleEvent)) -> AutoflowHistoryEventRecord {
    let at = chrono::DateTime::parse_from_rfc3339(at).unwrap().with_timezone(&chrono::Utc);
    let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, at);
    let mut e = AutoflowCycleEvent { kind, ..Default::default() };
    f(&mut e);
    AutoflowHistoryEventRecord::from_cycle_event(&cycle, e, at)
}

#[test]
fn cycle_failed_with_workflow_is_a_failed_attempt() {
    let events = [ev(AutoflowCycleEventKind::CycleFailed, "2026-10-01T10:00:00Z", |e| {
        e.workflow = Some("fix-issue".into());
        e.issue_ref = Some("github:acme/api/issues/7".into());
        e.detail = Some("dispatch failed: boom".into());
    })];
    let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].workflow_name, "fix-issue");
    assert_eq!(rows[0].status, rupu_orchestrator::runs::RunStatus::Failed);
    assert_eq!(rows[0].attempt.detail.as_deref(), Some("dispatch failed: boom"));
    assert_eq!(rows[0].autoflow.as_ref().unwrap().issue_ref.as_deref(), Some("github:acme/api/issues/7"));
}

#[test]
fn cycle_failed_without_workflow_is_not_an_attempt() {
    let events = [ev(AutoflowCycleEventKind::CycleFailed, "2026-10-01T10:00:00Z", |e| e.repo_ref = Some("github:acme/api".into()))];
    assert!(attempt_rows(&events, &|_| false, true, &ClaimIndex::default()).is_empty());
}

#[test]
fn attempt_with_recorded_run_is_not_listed() {
    let events = [ev(AutoflowCycleEventKind::RunLaunched, "2026-10-01T10:00:00Z", |e| {
        e.workflow = Some("fix-issue".into());
        e.run_id = Some("run_exists".into());
        e.status = Some("blocked".into());
    })];
    assert!(attempt_rows(&events, &|id| id == "run_exists", true, &ClaimIndex::default()).is_empty());
}

#[test]
fn launched_run_never_recorded_is_an_attempt_with_its_run_id() {
    let events = [ev(AutoflowCycleEventKind::RunLaunched, "2026-10-01T10:00:00Z", |e| {
        e.workflow = Some("fix-issue".into());
        e.run_id = Some("run_ghost".into());
        e.status = Some("blocked".into());
    })];
    let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
    assert_eq!(rows[0].id, "run_ghost");
    assert_eq!(rows[0].run_recorded, Some(false));
    assert_eq!(rows[0].status, rupu_orchestrator::runs::RunStatus::Failed);
    // remote mode never checks launches
    assert!(attempt_rows(&events, &|_| false, false, &ClaimIndex::default()).is_empty());
}

#[test]
fn problems_group_count_and_age_out() {
    let pe = |at: &str, repo: Option<&str>, worker: Option<&str>, d: &str| ProblemEvent {
        at: at.into(), repo_ref: repo.map(Into::into), worker: worker.map(Into::into), detail: Some(d.into()),
    };
    let events = vec![
        pe("2026-10-01T09:00:00Z", Some("github:acme/api"), None, "old"),
        pe("2026-10-01T10:10:00Z", Some("github:acme/api"), None, "config bad"),
        pe("2026-10-01T10:20:00Z", Some("github:acme/api"), None, "config still bad"),
        pe("2026-10-01T10:30:00Z", None, Some("mini"), "rate limited"),
    ];
    let cutoff = chrono::DateTime::parse_from_rfc3339("2026-10-01T09:45:00Z").unwrap().with_timezone(&chrono::Utc);
    let got = problems(&events, "local", cutoff);
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].subject_kind, got[0].subject.as_str(), got[0].count), ("worker", "mini", 1));
    assert_eq!((got[1].subject_kind, got[1].subject.as_str(), got[1].count), ("repo", "github:acme/api", 2));
    assert_eq!(got[1].detail.as_deref(), Some("config still bad"));
    assert_eq!(got[1].first_seen, "2026-10-01T10:10:00Z");
}
```

(`ClaimIndex::default()` — derive `Default` on `ClaimIndex` in Task 3 if not done.)

- [ ] **Step 2: Run to verify they fail** — `cargo test -p rupu-cp api::autoflow_runs::tests`; FAIL (module missing).

- [ ] **Step 3: Implement** the types and two functions per the interface notes. `attempt_rows` claim fields: build `RunAutoflow { issue_ref }`, then reuse `ClaimIndex`'s typed fill (expose `pub fn fill(&self, row_id: &str, af: &mut RunAutoflow)` from Task 3's `apply` and call it here). `claim_status` = `event.status`.

- [ ] **Step 4: Run to verify they pass** — PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/autoflow_runs.rs crates/rupu-cp/src/api/run_resolve.rs crates/rupu-cp/src/api/mod.rs
git commit -m "cp: synthesize autoflow failed-attempt rows and problems from history"
```

---

### Task 6: The two endpoints

**Files:**
- Modify: `crates/rupu-cp/src/api/autoflow_runs.rs` (routes + handlers)
- Modify: `crates/rupu-cp/src/api/runs.rs` (`read_filtered_runs` / `remote_run_window` take a row predicate; make `local_run_rows`-style helpers you need `pub(crate)`)
- Modify: `crates/rupu-cp/src/api/run_streams.rs` (`AutoflowEventRow.repo_ref: Option<String>`, `#[serde(skip_serializing_if = "Option::is_none")]`, filled from `rec.event.repo_ref`)
- Modify: wherever `run_streams::routes()` is merged into the app router (grep `run_streams::routes`) — merge `autoflow_runs::routes()` the same way
- Test: `crates/rupu-cp/src/api/autoflow_runs.rs` `mod tests`

**Interfaces:**
- Consumes: Tasks 2–5.
- Produces (HTTP):
  - `GET /api/runs/autoflows/runs?host=&lifecycle=&since=&until=&customer=&workflow=&offset=&limit=` → `Vec<serde_json::Value>`, each a run row (`RunListRow` + `host_id`) or an attempt row (`AttemptRow` + `host_id`), newest first by `started_at`, paged AFTER the merge. Error mapping identical to `/api/runs/workflows` (`host_list_error`: 501 unavailable / 502 offline / 404 unknown host). Header `X-Rupu-Hosts-Without-Customer` as Workflows.
  - `GET /api/runs/autoflows/problems?host=` → `Vec<Problem>`.
- `read_filtered_runs(conn, kind, lifecycle, keep: &(dyn Fn(&serde_json::Value) -> Option<bool> + Sync), page_size, want)` — `keep` returns `None` when the row can't be judged (→ `FilteredRuns::CannotReport`). The customer path passes `|row| customer_keep(row, f)` built from `filter_remote_rows`'s per-row logic; behavior of `/api/runs` and `/api/runs/workflows` must not change (their existing tests stay green).

Local path (`host=local`), inside `blocking`:
1. `query_run_rows_windowed(&store, None, lifecycle, false /* all runs */, None, &mut prices, &range, RowCustomers { lookup, filter, unreportable })` → keep rows with `matches!(row.autoflow, Some(Some(_)))` and, with `workflow`, `row.workflow_name == workflow`; `ClaimIndex::load(&global).apply(row)` each.
2. Attempts only when `filter.is_none()` and `lifecycle` is `None` or `Some("failed")`: `AutoflowHistoryStore::new(global.join("autoflows").join("history")).list_events_between(range.since(), range.until())` (missing dir → empty), `attempt_rows(&events, &|id| store.exists(id), true, &claims)`, `workflow` filter applied.
3. Serialize both, set `host_id: "local"`, sort by `started_at` string descending (both serialize `DateTime<Utc>` via serde — `Z` suffix — so lexicographic order is correct; do NOT use `to_rfc3339()`), then `crate::pagination::paginate`.

Remote path (`host=<id>`): runs via `remote_run_window(conn, host_id, RunKind::All, lifecycle, PageQuery { offset: 0, limit: want }, keep_autoflow_and_customer, FAN_OUT_LIMIT)` where `keep` returns `None` if the row has no `autoflow` key, else `Some(autoflow non-null && workflow matches && customer matches)`; attempts (same lifecycle/customer gate) via `conn.list_autoflow_events()` → keep `kind == "cycle_failed"` with a non-null `workflow`, build `AttemptRow`s from the JSON fields (`event_id`, `cycle_id`, `at`, `workflow`, `issue_display_ref`, `run_id`, `status`, `worker_name`, `detail`) — `Unsupported` from `list_autoflow_events` means "no attempts", not an error; then merge, tag `host_id`, page.

No `host`: fan out over `s.hosts.list_hosts()` (local + each remote, each asked for `offset + limit` rows), skip a host whose call fails (warn), merge newest first, page — same shape as `fan_out_list_runs`.

Problems: local → `list_events_between(Some(now - 1h), None)` mapped to `ProblemEvent { at, repo_ref: event.repo_ref, worker: worker_name.or(worker_id), detail }`, filter `kind == CycleFailed && workflow.is_none()`; remote → `list_autoflow_events()` rows with `kind == "cycle_failed"` and null `workflow` → `ProblemEvent { at, repo_ref: row["repo_ref"], worker: row["worker_name"], detail: row["detail"] }`; `problems(&evs, host_id, now - 1h)`. No `host` → all hosts concatenated, sorted by `last_seen` desc.

- [ ] **Step 1: Write the failing tests** (local path; the remote paths with a stub connector modeled on the ones in `runs.rs` tests ~L3199)

```rust
#[tokio::test]
async fn lists_autoflow_runs_not_manual_ones_with_attempts_newest_first() {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = crate::state::AppState::new(tmp.path().to_path_buf(), rupu_config::PricingConfig::default());
    // autoflow run at 10:00, manual run at 11:00, failed attempt at 12:00
    seed_run(&s, "run_af", "2026-10-01T10:00:00Z", Some("github:acme/api/issues/7"));
    seed_run(&s, "run_manual", "2026-10-01T11:00:00Z", None);
    seed_attempt(&s, "2026-10-01T12:00:00Z", "fix-issue", "boom");
    let rows = call_list(&s, "host=local").await;
    let ids: Vec<_> = rows.iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 2);
    assert!(rows[0]["attempt"].is_object());
    assert_eq!(rows[1]["id"], "run_af");
    assert!(!ids.contains(&"run_manual".to_string()));
}

#[tokio::test]
async fn merged_list_pages_after_merge() {
    // runs at 10:00 and 12:00, attempts at 11:00 and 13:00 → limit=2 gives [13:00 attempt, 12:00 run]; offset=2 gives [11:00 attempt, 10:00 run]
}

#[tokio::test]
async fn completed_filter_and_customer_scope_drop_attempts() {
    // lifecycle=completed → no attempt rows; customer=none → no attempt rows
}

#[tokio::test]
async fn workflow_param_narrows_runs_and_attempts() {}

#[tokio::test]
async fn remote_without_autoflow_key_is_501() {
    // stub connector whose list_runs returns rows without "autoflow" → status 501
}

#[tokio::test]
async fn problems_endpoint_groups_recent_failures_without_workflow() {}
```

Write each test body fully, following the first one: `seed_run(s, id, started_at, issue)` = `run_store.create(record with that started_at)` + `write_run_envelope(id, autoflow envelope)` when `issue` is `Some` (reuse Task 2's `autoflow_envelope` — move it to a `#[cfg(test)] pub(crate) mod test_support` in `api/runs.rs` so both modules share it); `seed_attempt(s, at, workflow, detail)` = `AutoflowHistoryStore::append_cycle_event` of a `CycleFailed` with `workflow`; `call_list(s, query)` = invoke the handler with `Query` parsed from the string and return the JSON rows (assert on the status code for the 501 test).

- [ ] **Step 2: Run to verify they fail** — `cargo test -p rupu-cp api::autoflow_runs::tests`; FAIL.

- [ ] **Step 3: Implement** handlers + predicate refactor + `repo_ref` per the notes above.

- [ ] **Step 4: Run to verify they pass** — `cargo test -p rupu-cp api::autoflow_runs:: api::runs:: api::run_streams::` PASS (runs/run_streams to prove the predicate refactor and event row change broke nothing). Then `cargo clippy -p rupu-cp --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/
git commit -m "cp: /api/runs/autoflows/runs and /problems"
```

---

### Task 7: Web types, client, shared Workflows columns

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`
- Create: `crates/rupu-cp/web/src/components/runs/workflowRunColumns.tsx`
- Modify: `crates/rupu-cp/web/src/pages/runs/WorkflowRuns.tsx`
- Test: `crates/rupu-cp/web/src/components/runs/workflowRunColumns.test.tsx`

**Interfaces:**
- Produces (TS):
  ```ts
  export interface RunAutoflow { issue_ref?: string; issue_display_ref?: string; issue_url?: string; pr_url?: string }
  // RunListRow gains:
  /** Absent: the host can't tell. null: not an autoflow run. */
  autoflow?: RunAutoflow | null;
  export interface AutoflowAttempt { detail: string | null; cycle_id: string; worker: string | null; claim_status: string | null }
  export type AutoflowRunRow = RunListRow & { attempt?: AutoflowAttempt; run_recorded?: boolean };
  export interface AutoflowProblem { host_id: string; subject_kind: 'repo' | 'worker'; subject: string; detail: string | null; first_seen: string; last_seen: string; count: number }
  api.getAutoflowRunList(params?: ListParams & Cancellable & { lifecycle?: 'active' | 'completed' | 'failed'; host?: string; workflow?: string; customer?: CustomerScope; onHostsWithoutCustomer?: HostsWithoutCustomerSink }): Promise<AutoflowRunRow[]>  // GET /api/runs/autoflows/runs
  api.getAutoflowProblems(params?: Cancellable & { host?: string }): Promise<AutoflowProblem[]>  // GET /api/runs/autoflows/problems
  ```
  `workflowRunColumns.tsx` exports `WORKFLOW_RUN_COLUMNS: Column<RunListRow>[]`, `TriggerChip`, `runDurationMs` — moved verbatim from `WorkflowRuns.tsx` (L84–102 and L438–end); `WorkflowRuns.tsx` imports them. Its rendered output must not change.

- [ ] **Step 1: Write the failing test**

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { WORKFLOW_RUN_COLUMNS, runDurationMs } from './workflowRunColumns';

describe('workflowRunColumns', () => {
  it('keeps the Workflows column order', () => {
    expect(WORKFLOW_RUN_COLUMNS.map((c) => c.key)).toEqual([
      'status', 'workflow', 'run', 'trigger', 'host', 'in', 'out', 'cached', 'cost', 'turns', 'duration', 'started',
    ]);
  });
  it('derives a duration from start/finish when duration_ms is absent', () => {
    expect(runDurationMs({ duration_ms: null, started_at: '2026-10-01T10:00:00Z', finished_at: '2026-10-01T10:00:05Z' } as never)).toBe(5000);
  });
});
```

- [ ] **Step 2: Run to verify it fails** — `cd crates/rupu-cp/web && npx vitest run src/components/runs/workflowRunColumns.test.tsx`; FAIL (module missing).

- [ ] **Step 3: Implement** — move the code, add the types and client methods (mirror `getWorkflowRuns` exactly, plus `workflow` → `q.set('workflow', …)`).

- [ ] **Step 4: Run to verify it passes** — the new test plus `npx vitest run src/pages/runs/WorkflowRuns` PASS; `npx tsc --noEmit` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/components/runs/ crates/rupu-cp/web/src/pages/runs/WorkflowRuns.tsx
git commit -m "web: share the Workflows run columns; autoflow run/problem API client"
```

---

### Task 8: The one-table Autoflows page + problems strip

**Files:**
- Create: `crates/rupu-cp/web/src/components/runs/AutoflowProblems.tsx`
- Rewrite: `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx`
- Rewrite: `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.test.tsx`

**Interfaces:**
- Consumes: Task 7's types, client, `WORKFLOW_RUN_COLUMNS`.
- Produces: default export `AutoflowRuns` (route unchanged: `/runs/autoflows`); `AutoflowProblems({ host }: { host: string | null })`.

Page shape — copy `WorkflowRuns.tsx`'s structure (header + Refresh, `FilterBar` with the lifecycle `FilterPills` incl. Archived, `SearchInput`, `HostSelect allowAll`, `PerHostStrip`, customer banner + note, `ErrorBanner`, loading / waiting / empty states, `UsageBarChart`, `SortableTable`, `PerHostFooter`) with these differences:
- Title "Autoflow Runs", subtitle "Workflow runs launched by autoflows, and attempts that failed before a run started."
- Fetch: `api.getAutoflowRunList({ lifecycle: tab, offset, limit, host, signal, workflow, customer… })`; Archived uses `api.getArchivedRuns('workflow')` filtered client-side to rows with a non-null `autoflow`.
- No trigger pills (all rows are cron/event).
- `workflow` filter state: clicking a workflow name sets it; shown as a removable chip in the `FilterBar` `filters` slot (a `Button variant="ring"` reading `workflow: <name> ×`); it's in the hook's `deps`.
- Columns: `WORKFLOW_RUN_COLUMNS` with the `workflow` column replaced by one that (a) renders the name as a `<button>` that sets the filter (`interactive: true`, `preventDefault`+`stopPropagation` like the action column), and (b) for an attempt row adds `<div className="text-note text-err truncate">{r.attempt.detail}</div>` under it; an `issue` column inserted after it: `autoflow.issue_display_ref ?? autoflow.issue_ref` linked to `issue_url` when present (`target="_blank" rel="noreferrer"`), plus `PR ↗` linked to `pr_url`; `—` when none. Action column (Archive / Delete) only for rows without `attempt`; `withCustomerColumn` as Workflows.
- `rowHref`: `runHref(r)` for a run row and for an attempt with `run_recorded === false` (the run page renders unpersisted runs); `renderDetail`: for an attempt whose `id` is not a run id (no `run_recorded`), return the detail panel — the full `attempt.detail` in a `<pre className="text-note font-mono whitespace-pre-wrap">`, cycle id (`shortId`), worker, claim status, and Requeue / Release buttons (only when `host_id` is local; `window.confirm` first; call `api.requeueClaim(issue_ref)` / `api.releaseClaim(issue_ref)`; on success `refreshHost('local')`; errors into `actionError`). Return `null` for every other row.
- Customer scope: under a scope, a one-line `text-note text-ink-mute` note: "Failed attempts aren't listed while a customer is selected — they record no customer."
- Find box matches `workflow_name`, `id`, `host_id`, `codename`, `autoflow.issue_display_ref`, `autoflow.issue_ref`, `attempt.detail`.
- `<AutoflowProblems host={…} />` between the filter bar and the table.

`AutoflowProblems`: per-host load of `api.getAutoflowProblems({ host })` (for All hosts, call once per registered host from `api.getRegisteredHosts()` and concatenate; a failing host is skipped), polled every 30 s; renders nothing when empty; else a `rounded-xl border border-warn/30 bg-warn-bg` block, one row per problem: `⚠` (lucide `AlertTriangle`, `text-warn`), "repo `<subject>`: autoflow cycle failed" / "worker `<subject>`: cycle crashed", the `detail` truncated (`title` = full), and `since {relativeTime(first_seen)} · {count} cycles · {host_id}` in `text-ink-dim`.

- [ ] **Step 1: Write the failing tests** (replace the old file; keep its `LOCAL_HOST`/`REMOTE_HOST` fixtures and `renderPage`)

```tsx
const RUN: AutoflowRunRow = {
  id: 'run-9', workflow_name: 'fix-issue', status: 'completed', started_at: '2026-06-01T00:00:00Z', finished_at: '2026-06-01T00:05:00Z',
  trigger: 'event', turns: 3, duration_ms: 300000, codename: 'cobalt-harbor/heron', codename_derived: false, host_id: 'local',
  usage: { input_tokens: 100, output_tokens: 50, cached_tokens: 0, total_tokens: 150, cost_usd: null, priced: false, runs: 1 },
  autoflow: { issue_ref: 'github:acme/widgets/issues/42', issue_display_ref: 'acme/widgets#42', issue_url: 'https://example.test/42', pr_url: 'https://example.test/pr/7' },
} as AutoflowRunRow;
const ATTEMPT: AutoflowRunRow = {
  ...RUN, id: 'afe_1', status: 'failed', turns: 0, duration_ms: null, autoflow: { issue_ref: 'github:acme/widgets/issues/43' },
  attempt: { detail: 'dispatch failed: branch exists', cycle_id: 'cyc_1', worker: 'local', claim_status: 'retry_backoff' },
};
function stub(rows: AutoflowRunRow[], problems: AutoflowProblem[] = []) {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
  vi.spyOn(api, 'getAutoflowRunList').mockImplementation(onlyHost('local', rows));
  vi.spyOn(api, 'getAutoflowProblems').mockResolvedValue(problems);
}

it('shows one table: no Runs/Cycles/Claims switcher', async () => {
  stub([RUN]);
  renderPage();
  await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());
  expect(screen.queryByText('Cycles')).not.toBeInTheDocument();
  expect(screen.queryByText('Claims')).not.toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'acme/widgets#42' })).toHaveAttribute('href', 'https://example.test/42');
  expect(screen.getByRole('link', { name: /PR/ })).toHaveAttribute('href', 'https://example.test/pr/7');
});

it('an attempt row shows its error and expands to Requeue / Release', async () => {
  stub([ATTEMPT]);
  const requeue = vi.spyOn(api, 'requeueClaim').mockResolvedValue({ wake_id: 'w' });
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  renderPage();
  await waitFor(() => expect(screen.getByText('dispatch failed: branch exists')).toBeInTheDocument());
  fireEvent.click(screen.getByText('dispatch failed: branch exists'));
  fireEvent.click(await screen.findByRole('button', { name: 'Requeue claim' }));
  await waitFor(() => expect(requeue).toHaveBeenCalledWith('github:acme/widgets/issues/43'));
});

it('clicking a workflow name refetches narrowed to it', async () => {
  stub([RUN]);
  renderPage();
  fireEvent.click(await screen.findByRole('button', { name: 'fix-issue' }));
  await waitFor(() => expect((api.getAutoflowRunList as Mock).mock.calls.some((c) => c[0]?.workflow === 'fix-issue')).toBe(true));
  expect(screen.getByRole('button', { name: /workflow: fix-issue/ })).toBeInTheDocument();
});

it('renders the problems strip and hides it when empty', async () => {
  stub([RUN], [{ host_id: 'local', subject_kind: 'repo', subject: 'github:acme/api', detail: 'config bad', first_seen: '2026-06-01T00:00:00Z', last_seen: '2026-06-01T00:30:00Z', count: 24 }]);
  renderPage();
  expect(await screen.findByText(/24 cycles/)).toBeInTheDocument();
});

it('unavailable host slice: a 501 from one host is shown, not an error', async () => {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
  vi.spyOn(api, 'getAutoflowProblems').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowRunList').mockImplementation((p) =>
    p?.host === 'host_prod' ? Promise.reject(new ApiError(501, 'unsupported')) : Promise.resolve([RUN]));
  renderPage();
  await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());
  expect(screen.getByText(/unavailable/i)).toBeInTheDocument();
});

it('All hosts by default: the list is fetched per registered host', async () => {
  stub([]);
  renderPage();
  await waitFor(() => expect(callsFor(api.getAutoflowRunList as never, 'local')).toHaveLength(1));
  expect(callsFor(api.getAutoflowRunList as never, 'host_prod')).toHaveLength(1);
});
```

(Adjust the `ApiError` constructor call and the "unavailable" text to whatever `ApiError` / `PerHostStrip` actually take and render — check `lib/api.ts` and `components/lists/PerHostStatus.tsx`.)

- [ ] **Step 2: Run to verify they fail** — `npx vitest run src/pages/runs/AutoflowRuns.test.tsx`; FAIL.

- [ ] **Step 3: Implement** the strip component and the page per the shape above. Delete the old page's events/cycles/claims code, `KIND_*`, `ModeChip`, `CycleDetail`, `VIEW_OPTIONS`.

- [ ] **Step 4: Run to verify they pass** — the test file PASS; `npx tsc --noEmit` clean; `npx vitest run src/pages/runs src/components/runs` PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx crates/rupu-cp/web/src/pages/runs/AutoflowRuns.test.tsx crates/rupu-cp/web/src/components/runs/AutoflowProblems.tsx
git commit -m "web: Runs → Autoflows is one Workflows-shaped table with a problems strip"
```

---

### Task 9: Claim actions on the run page

**Files:**
- Modify: `crates/rupu-cp/web/src/components/AutoflowPanel.tsx`
- Modify: `crates/rupu-cp/web/src/pages/RunDetail.tsx` (L1339: `<AutoflowPanel context={autoflowCtx} host={host} />`)
- Test: `crates/rupu-cp/web/src/components/AutoflowPanel.test.tsx` (new)

**Interfaces:**
- Produces: `AutoflowPanel({ context, host }: { context: AutoflowRunContext; host?: string | null })` — Requeue / Release show when `context.claim` exists and `!host || host === 'local'`; otherwise, with a claim, a `text-note text-ink-mute` line "Claim actions are available on the host that ran this autoflow." The "View autoflow history" link is renamed "All autoflow runs" (same `/runs/autoflows` target).

- [ ] **Step 1: Write the failing test**

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowRunContext } from '../lib/api';
import AutoflowPanel from './AutoflowPanel';

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const CTX: AutoflowRunContext = {
  repo_ref: 'github:acme/widgets', issue_ref: 'github:acme/widgets/issues/42', entity: '42', workflow_name: 'fix-issue',
  status: 'failed', failure: null, cycle_id: 'cyc_1', workspace_path: null, host_id: null, prior_cycles: [],
  claim: { issue_ref: 'github:acme/widgets/issues/42', issue_display_ref: 'acme/widgets#42', repo_ref: 'github:acme/widgets', issue_title: null,
    issue_url: null, workflow: 'fix-issue', status: 'blocked', last_run_id: 'run-9', last_error: null, last_summary: null, pr_url: null,
    claim_owner: null, lease_expires_at: null, updated_at: '2026-06-01T00:00:00Z' },
};

describe('AutoflowPanel claim actions', () => {
  it('requeues and releases a local run’s claim after confirmation', async () => {
    const requeue = vi.spyOn(api, 'requeueClaim').mockResolvedValue({ wake_id: 'w' });
    const release = vi.spyOn(api, 'releaseClaim').mockResolvedValue({ released: true });
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(<MemoryRouter><AutoflowPanel context={CTX} /></MemoryRouter>);
    fireEvent.click(screen.getByRole('button', { name: 'Requeue' }));
    await waitFor(() => expect(requeue).toHaveBeenCalledWith('github:acme/widgets/issues/42'));
    fireEvent.click(screen.getByRole('button', { name: 'Release' }));
    await waitFor(() => expect(release).toHaveBeenCalledWith('github:acme/widgets/issues/42'));
  });

  it('hides claim actions for a remote run', () => {
    render(<MemoryRouter><AutoflowPanel context={CTX} host="host_prod" /></MemoryRouter>);
    expect(screen.queryByRole('button', { name: 'Requeue' })).not.toBeInTheDocument();
    expect(screen.getByText(/available on the host that ran/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails** — `npx vitest run src/components/AutoflowPanel.test.tsx`; FAIL.

- [ ] **Step 3: Implement** — buttons (`Button variant="ring"` Requeue, `variant="ring-danger"` Release) in the Claim cell, `window.confirm` with the same wording the old Claims table used (copy it from the old `AutoflowRuns.tsx` before Task 8 deletes it — `git show HEAD~:…` if needed), an inline `ErrorBanner` on failure, a "Requeued" / "Released" note on success.

- [ ] **Step 4: Run to verify it passes** — PASS; `npx vitest run src/pages/RunDetail` PASS; `npx tsc --noEmit` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/AutoflowPanel.tsx crates/rupu-cp/web/src/components/AutoflowPanel.test.tsx crates/rupu-cp/web/src/pages/RunDetail.tsx
git commit -m "web: requeue / release an autoflow claim from the run page"
```

---

## Finish

- [ ] `make cp-web` (rebuild the embedded UI), `cargo build -p rupu-cli`, then run the CP against an isolated `RUPU_HOME` seeded with an autoflow run, a failed attempt and a repo-config failure; screenshot Runs → Autoflows and an autoflow run's page for matt.
- [ ] Open PR 2; matt checks the page with real data before merge.
