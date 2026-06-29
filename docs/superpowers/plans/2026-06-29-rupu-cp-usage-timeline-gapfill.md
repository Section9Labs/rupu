# CP usage-timeline gap-fill — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the CP Dashboard "Token Spent" timeline a continuous per-day (or per-week) series that spans every bucket from the window start (clamped to the first-ever run) through *now*, by synthesizing empty buckets for zero-spend periods.

**Architecture:** Backend-only change in `crates/rupu-cp/src/api/usage.rs`. Two new pure helpers (`timeline_fill_start`, `enumerate_bucket_keys`) compute the gap-fill range and bucket keys; `build_timeline` gains `fill_start`/`fill_end` params and emits one bucket per enumerated key (empty `rows` when no data); `get_usage_timeline` wires the fill-start from the earliest run. The frontend stacked-area chart is unchanged — `toChartData` already seeds every model to 0 per bucket, so empty buckets render as zero points.

**Tech Stack:** Rust (axum handler, chrono `DateTime<Utc>`/`NaiveDate`), React + TypeScript + vitest (frontend confirming test only).

## Global Constraints

- Rust 2021; workspace deps only (never pin versions in crate `Cargo.toml`); `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- Libraries use `thiserror`; this change adds no new error types.
- API response shape (`UsageTimelineBucket { bucket: String, rows: Vec<UsageBreakdownRow> }`) is UNCHANGED — empty periods are buckets with an empty `rows` array.
- No change to aggregation math, the chart type, or the range selector.
- Never run package-wide `cargo fmt` (repo rustfmt-drift hazard); format only files you touched (`rustfmt --edition 2021 <file>` or `cargo fmt -p rupu-cp` is FORBIDDEN — it reflows the whole crate). Confirm `git status --short` shows only intended files before committing.
- Backend-only: `make cp-web` is NOT required; the embedded UI is unchanged.
- Frontend test commands run from `crates/rupu-cp/web`.

---

## File Structure

- **Modify** `crates/rupu-cp/src/api/usage.rs` — add `timeline_fill_start` + `enumerate_bucket_keys`; change `build_timeline` signature + body; wire `get_usage_timeline`; add/adjust tests. (Task 1)
- **Modify** `crates/rupu-cp/web/src/components/dashboard/UsageTimelineStacked.test.tsx` — add one test that an empty bucket becomes an all-zero datum. (Task 2)

---

## Task 1: Backend gap-fill in `build_timeline`

**Files:**
- Modify: `crates/rupu-cp/src/api/usage.rs` (helpers near `bucket_key`/`build_timeline` ~lines 122-182; tests in the `#[cfg(test)] mod tests` ~lines 184-347)
- Test: same file's `tests` module

**Interfaces:**
- Consumes: existing `Granularity` enum, `bucket_key(DateTime<Utc>, Granularity) -> String`, `UsageTimelineBucket { bucket: String, rows: Vec<crate::usage::UsageBreakdownRow> }`, `crate::usage::{breakdown, GroupBy}`, test helpers `dt(&str) -> DateTime<Utc>` and `urow(provider, model, input, output)`.
- Produces:
  - `fn timeline_fill_start(window_start: DateTime<Utc>, earliest_run: Option<DateTime<Utc>>) -> Option<DateTime<Utc>>`
  - `fn enumerate_bucket_keys(fill_start: DateTime<Utc>, fill_end: DateTime<Utc>, granularity: Granularity) -> Vec<String>`
  - `fn build_timeline(runs_with_rows: Vec<(DateTime<Utc>, Vec<rupu_transcript::UsageRow>)>, pricing: &rupu_config::PricingConfig, granularity: Granularity, fill_start: DateTime<Utc>, fill_end: DateTime<Utc>) -> Vec<UsageTimelineBucket>`

- [ ] **Step 1: Write failing tests for the two new helpers**

Add to the `tests` module in `crates/rupu-cp/src/api/usage.rs` (it already has `dt`, `urow`, `Duration`, `DateTime`, `Utc` in scope via `use super::*;`):

```rust
    #[test]
    fn timeline_fill_start_clamps_to_window_then_first_run() {
        let start = dt("2026-06-01T00:00:00Z");
        // No runs at all → None (caller returns an empty series).
        assert_eq!(timeline_fill_start(start, None), None);
        // First run older than the window → clamp up to the window start.
        assert_eq!(
            timeline_fill_start(start, Some(dt("2026-05-10T00:00:00Z"))),
            Some(start)
        );
        // First run inside the window → start at the first run (no flat lead-in).
        let first = dt("2026-06-10T08:00:00Z");
        assert_eq!(timeline_fill_start(start, Some(first)), Some(first));
    }

    #[test]
    fn enumerate_bucket_keys_daily_is_inclusive_continuous() {
        let keys = enumerate_bucket_keys(
            dt("2026-06-10T23:00:00Z"),
            dt("2026-06-13T01:00:00Z"),
            Granularity::Day,
        );
        assert_eq!(keys, vec!["2026-06-10", "2026-06-11", "2026-06-12", "2026-06-13"]);
    }

    #[test]
    fn enumerate_bucket_keys_weekly_snaps_to_monday_and_steps_weeks() {
        // 2026-06-24 is a Wednesday (week of 06-22); end 2026-07-06 is a Monday.
        let keys = enumerate_bucket_keys(
            dt("2026-06-24T10:00:00Z"),
            dt("2026-07-06T10:00:00Z"),
            Granularity::Week,
        );
        assert_eq!(keys, vec!["2026-06-22", "2026-06-29", "2026-07-06"]);
    }
```

- [ ] **Step 2: Run the helper tests to verify they fail**

Run: `cargo test -p rupu-cp --lib api::usage::tests::timeline_fill_start_clamps_to_window_then_first_run api::usage::tests::enumerate_bucket_keys_daily_is_inclusive_continuous api::usage::tests::enumerate_bucket_keys_weekly_snaps_to_monday_and_steps_weeks`
Expected: FAIL — `cannot find function timeline_fill_start` / `enumerate_bucket_keys`.

- [ ] **Step 3: Implement the two helpers**

Insert into `crates/rupu-cp/src/api/usage.rs` immediately after `bucket_key` (around line 133), before `build_timeline`:

```rust
/// Gap-fill start for the timeline, or `None` when the store has no runs at all
/// (caller returns an empty series). Clamps the window start up to the
/// first-ever run: bounded windows (7d/30d) draw the full window with zeros;
/// the unbounded `all` window starts at the first run instead of the epoch.
fn timeline_fill_start(
    window_start: DateTime<Utc>,
    earliest_run: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    earliest_run.map(|earliest| window_start.max(earliest))
}

/// Every bucket key from `fill_start` to `fill_end` inclusive, at the granularity.
/// `Day` → one key per calendar day; `Week` → one key per ISO week, starting from
/// the Monday on or before `fill_start`. Produces `YYYY-MM-DD` keys identical in
/// form to [`bucket_key`], so they align with grouped run buckets.
fn enumerate_bucket_keys(
    fill_start: DateTime<Utc>,
    fill_end: DateTime<Utc>,
    granularity: Granularity,
) -> Vec<String> {
    let mut cursor = match granularity {
        Granularity::Day => fill_start.date_naive(),
        Granularity::Week => {
            let d = fill_start.date_naive();
            d - Duration::days(d.weekday().num_days_from_monday() as i64)
        }
    };
    let end = fill_end.date_naive();
    let step = match granularity {
        Granularity::Day => Duration::days(1),
        Granularity::Week => Duration::days(7),
    };
    let mut keys = Vec::new();
    while cursor <= end {
        keys.push(cursor.format("%Y-%m-%d").to_string());
        cursor = cursor + step;
    }
    keys
}
```

(`chrono::NaiveDate + chrono::Duration` is supported and returns a `NaiveDate`; `Duration` and `Datelike`/`weekday()` are already used by `bucket_key` in this file.)

- [ ] **Step 4: Run the helper tests to verify they pass**

Run: `cargo test -p rupu-cp --lib api::usage::tests::timeline_fill_start_clamps_to_window_then_first_run api::usage::tests::enumerate_bucket_keys_daily_is_inclusive_continuous api::usage::tests::enumerate_bucket_keys_weekly_snaps_to_monday_and_steps_weeks`
Expected: PASS (3 tests).

- [ ] **Step 5: Write the failing gap-fill tests for `build_timeline` (new signature)**

Add to the `tests` module:

```rust
    #[test]
    fn build_timeline_gap_fills_empty_days_between_and_after_runs() {
        let pricing = rupu_config::PricingConfig::default();
        let runs = vec![
            (dt("2026-06-10T10:00:00Z"), vec![urow("anthropic", "claude-sonnet-4-6", 1000, 0)]),
            (dt("2026-06-12T10:00:00Z"), vec![urow("anthropic", "claude-sonnet-4-6", 2000, 0)]),
        ];
        // Fill from the first run through 06-15 (e.g. "now").
        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Day,
            dt("2026-06-10T10:00:00Z"),
            dt("2026-06-15T00:00:00Z"),
        );
        let keys: Vec<&str> = buckets.iter().map(|b| b.bucket.as_str()).collect();
        assert_eq!(
            keys,
            vec!["2026-06-10", "2026-06-11", "2026-06-12", "2026-06-13", "2026-06-14", "2026-06-15"]
        );
        // Populated days carry rows; gap days are empty.
        assert!(!buckets[0].rows.is_empty()); // 06-10
        assert!(buckets[1].rows.is_empty()); // 06-11
        assert!(!buckets[2].rows.is_empty()); // 06-12
        assert!(buckets[3].rows.is_empty()); // 06-13
        assert!(buckets[5].rows.is_empty()); // 06-15 (reaches the end)
        assert_eq!(buckets[2].rows[0].input_tokens, 2000);
    }

    #[test]
    fn build_timeline_reaches_end_when_no_recent_activity() {
        let pricing = rupu_config::PricingConfig::default();
        let runs = vec![(
            dt("2026-06-10T10:00:00Z"),
            vec![urow("anthropic", "claude-sonnet-4-6", 1000, 0)],
        )];
        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Day,
            dt("2026-06-10T10:00:00Z"),
            dt("2026-06-13T00:00:00Z"),
        );
        assert_eq!(buckets.len(), 4); // 06-10..06-13 inclusive
        assert_eq!(buckets.last().unwrap().bucket, "2026-06-13");
        assert!(buckets.last().unwrap().rows.is_empty());
    }
```

- [ ] **Step 6: Run the gap-fill tests to verify they fail to compile**

Run: `cargo test -p rupu-cp --lib api::usage::tests::build_timeline_gap_fills_empty_days_between_and_after_runs`
Expected: FAIL — `build_timeline` takes 3 args, not 5 (compile error).

- [ ] **Step 7: Change `build_timeline` to the new signature + gap-fill body**

Replace the existing `build_timeline` (lines ~138-156) with:

```rust
/// Group per-run `(started_at, rows)` by bucket key, then emit a CONTINUOUS run
/// of buckets from `fill_start` to `fill_end` inclusive at the granularity —
/// synthesizing an empty bucket (`rows: []`) for every period with no runs, so
/// the timeline has no gaps and reaches `fill_end`. Buckets are chronological.
fn build_timeline(
    runs_with_rows: Vec<(DateTime<Utc>, Vec<rupu_transcript::UsageRow>)>,
    pricing: &rupu_config::PricingConfig,
    granularity: Granularity,
    fill_start: DateTime<Utc>,
    fill_end: DateTime<Utc>,
) -> Vec<UsageTimelineBucket> {
    let mut grouped: std::collections::BTreeMap<String, Vec<rupu_transcript::UsageRow>> =
        std::collections::BTreeMap::new();
    for (started_at, rows) in runs_with_rows {
        let key = bucket_key(started_at, granularity);
        grouped.entry(key).or_default().extend(rows);
    }
    enumerate_bucket_keys(fill_start, fill_end, granularity)
        .into_iter()
        .map(|bucket| {
            let rows = grouped
                .get(&bucket)
                .map(|rows| crate::usage::breakdown(rows, pricing, crate::usage::GroupBy::Model))
                .unwrap_or_default();
            UsageTimelineBucket { rows, bucket }
        })
        .collect()
}
```

- [ ] **Step 8: Wire `get_usage_timeline` to compute the fill range**

In `get_usage_timeline` (lines ~158-182), after `let runs = s.run_store.list()...?;` and before building `runs_with_rows`, insert the fill-start computation and the empty-store guard; then pass `fill_start`/`end` to `build_timeline`. The function becomes:

```rust
async fn get_usage_timeline(
    State(s): State<AppState>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<Vec<UsageTimelineBucket>>> {
    let (start, end) = resolve_window(q.since.as_deref(), q.until.as_deref(), Utc::now())
        .map_err(ApiError::bad_request)?;
    let granularity = Granularity::parse(q.bucket.as_deref()).map_err(ApiError::bad_request)?;

    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // Clamp the fill start to the first-ever run; no runs at all → empty series.
    let earliest_overall = runs.iter().map(|r| r.started_at).min();
    let Some(fill_start) = timeline_fill_start(start, earliest_overall) else {
        return Ok(Json(Vec::new()));
    };

    let mut runs_with_rows: Vec<(DateTime<Utc>, Vec<rupu_transcript::UsageRow>)> = Vec::new();
    for r in runs
        .iter()
        .filter(|r| r.started_at >= start && r.started_at <= end)
    {
        let paths = crate::usage::run_transcript_paths(&s.run_store, &r.id);
        let rows = rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default());
        runs_with_rows.push((r.started_at, rows));
    }

    Ok(Json(build_timeline(
        runs_with_rows,
        &s.pricing,
        granularity,
        fill_start,
        end,
    )))
}
```

- [ ] **Step 9: Update the two existing `build_timeline` tests to the new signature**

In `build_timeline_buckets_by_day_with_per_model_breakdown`, change the call (line ~301) and assertions. The runs span 06-24..06-25, so pass `fill_start = dt("2026-06-24T10:00:00Z")`, `fill_end = dt("2026-06-25T09:00:00Z")` to keep the result exactly the two populated days:

```rust
        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Day,
            dt("2026-06-24T10:00:00Z"),
            dt("2026-06-25T09:00:00Z"),
        );
        assert_eq!(buckets.len(), 2);
```
(The rest of that test's assertions are unchanged — both days are populated, so no empty buckets appear.)

In `build_timeline_week_collapses_days_into_one_bucket`, change the call (line ~342) so the fill range is the single ISO week:

```rust
        let buckets = build_timeline(
            runs,
            &pricing,
            Granularity::Week,
            dt("2026-06-24T10:00:00Z"),
            dt("2026-06-25T10:00:00Z"),
        );
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].bucket, "2026-06-22");
        assert_eq!(buckets[0].rows[0].input_tokens, 3000);
```

- [ ] **Step 10: Run the full `usage` test module + verify it passes**

Run: `cargo test -p rupu-cp --lib api::usage::tests`
Expected: PASS — all existing tests plus the 5 new ones (`timeline_fill_start_*`, `enumerate_bucket_keys_daily_*`, `enumerate_bucket_keys_weekly_*`, `build_timeline_gap_fills_*`, `build_timeline_reaches_end_*`).

- [ ] **Step 11: Build the crate, format the one file, clippy**

Run: `cargo build -p rupu-cp && rustfmt --edition 2021 crates/rupu-cp/src/api/usage.rs && cargo clippy -p rupu-cp --all-targets 2>&1 | grep -E "usage.rs" || echo "no usage.rs clippy warnings"`
Expected: build succeeds; no clippy warnings referencing `usage.rs`.

- [ ] **Step 12: Confirm scope, then commit**

Run: `git status --short` (expect only `crates/rupu-cp/src/api/usage.rs`)

```bash
git add crates/rupu-cp/src/api/usage.rs
git commit -m "fix(cp): gap-fill the usage timeline through now

build_timeline now emits a continuous run of buckets from the window
start (clamped to the first-ever run) through fill_end, synthesizing
empty buckets for zero-spend periods. No runs → empty series. Response
shape unchanged."
```

---

## Task 2: Frontend confirming test for empty-bucket continuity

**Files:**
- Modify: `crates/rupu-cp/web/src/components/dashboard/UsageTimelineStacked.test.tsx`

**Interfaces:**
- Consumes: the exported `toChartData(buckets, metric)` from `UsageTimelineStacked.tsx` (already imported by the existing test). Bucket/row fixture shape: mirror the existing test's helper for building a `UsageTimelineBucket`/row.

- [ ] **Step 1: Read the existing test to match its fixture style**

Open `crates/rupu-cp/web/src/components/dashboard/UsageTimelineStacked.test.tsx`. Note how it constructs `UsageTimelineBucket` objects and breakdown rows (the row fields `toChartData` reads are `model`/`provider`/`agent`, `cost_usd`, `total_tokens`). Reuse the same construction style for the new test.

- [ ] **Step 2: Write the failing test (empty bucket → all-zero datum, continuous)**

Add this test (adapt the row fixture to match the file's existing helper/shape; `total_tokens` and `cost_usd` are the fields used):

```ts
  it('renders an empty bucket as a continuous zero datum (gap-fill)', () => {
    const buckets = [
      { bucket: '2026-06-10', rows: [{ model: 'claude-sonnet-4-6', provider: 'anthropic', agent: '', input_tokens: 0, output_tokens: 0, total_tokens: 1000, cost_usd: 0.5, priced: true, runs: 1 }] },
      { bucket: '2026-06-11', rows: [] }, // zero-spend day
      { bucket: '2026-06-12', rows: [{ model: 'claude-sonnet-4-6', provider: 'anthropic', agent: '', input_tokens: 0, output_tokens: 0, total_tokens: 3000, cost_usd: 1.5, priced: true, runs: 1 }] },
    ];

    const { models, data } = toChartData(buckets as never, 'tokens');

    expect(models).toEqual(['claude-sonnet-4-6']);
    expect(data).toHaveLength(3);
    // The empty middle bucket is present and carries an explicit 0 for the model,
    // so the stacked area stays continuous (no gap / dropped point).
    expect(data[1].bucket).toBe('2026-06-11');
    expect(data[1]['claude-sonnet-4-6']).toBe(0);
    expect(data[0]['claude-sonnet-4-6']).toBe(1000);
    expect(data[2]['claude-sonnet-4-6']).toBe(3000);
  });
```

(If the existing test imports a typed `UsageTimelineBucket`, build the fixtures with that type instead of `as never`, matching the file's convention.)

- [ ] **Step 3: Run the test to verify it passes (behavior already supported)**

Run (from `crates/rupu-cp/web`): `npx vitest run src/components/dashboard/UsageTimelineStacked.test.tsx`
Expected: PASS — this is a characterization test; `toChartData` already seeds every model to 0 per bucket (`UsageTimelineStacked.tsx:41`), so the empty bucket yields a 0 datum.

(Note: this test asserts existing behavior to lock in the continuity guarantee the backend gap-fill relies on. If it unexpectedly FAILS, stop and report — it means the frontend does not zero-fill and a component change is needed, contradicting the design.)

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/web/src/components/dashboard/UsageTimelineStacked.test.tsx
git commit -m "test(cp/web): lock in empty-bucket zero-point continuity for the usage timeline"
```

---

## Final verification (after both tasks)

- [ ] **Backend tests + build**

Run: `cargo test -p rupu-cp --lib api::usage && cargo build -p rupu-cp`
Expected: green.

- [ ] **Frontend test suite**

Run (from `crates/rupu-cp/web`): `npx vitest run src/components/dashboard/UsageTimelineStacked.test.tsx`
Expected: green.

- [ ] **Manual smoke (matt, recommended)**

`rupu cp serve`, open the Dashboard, toggle the range to 7d / 30d / all. Confirm the Token Spent curve is continuous, includes zero-spend days, and the last point is today.

---

## Self-review notes

- Spec §3.1 fill rule → `timeline_fill_start` (Task 1 Step 3) + handler wiring (Step 8). Spec §3.2 enumeration → `enumerate_bucket_keys` + `build_timeline` (Steps 3, 7). Spec §3.3 wiring → Step 8. Spec §4 edge cases → covered by helper tests (None/clamp), gap-fill tests (continuous, reaches end), and the empty-store guard. Spec §5 testing → Steps 1/5/9 (Rust) + Task 2 (frontend). All spec sections map to a task.
- `make cp-web` intentionally omitted (backend-only; no embedded-UI change), per spec §6.
