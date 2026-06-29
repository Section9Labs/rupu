# CP dashboard usage timeline — gap-fill design

- **Status:** Approved (2026-06-29)
- **Author:** rupu agent (paired with matt)
- **Scope:** Make the Dashboard "Token Spent" timeline a continuous per-day series
  that spans every day in the selected window through *today*, instead of only the
  days that happened to have runs.

## 1. Motivation

The Dashboard usage chart (`UsageTimelineStacked`, a recharts stacked area by model)
looks cumulative and stops weeks in the past (last day 6/15). Investigation shows
the underlying data is **not** cumulative — `build_timeline`
(`crates/rupu-cp/src/api/usage.rs:138`) already produces independent per-bucket
sums. The real defects:

1. **No gap-fill.** Buckets are created only for days that had runs
   (`usage.rs:143-148`, a `BTreeMap` keyed by bucket). Days with zero spend are
   absent, so the series is discontinuous and simply ends at the last active day.
2. **Never reaches today.** Same cause — the X-axis ends at the last run's day, not
   "now."

The "cumulative" appearance is a separate, intentional artifact of the stacked
area chart (areas pile up by model); matt wants to keep that chart style. The fix
is purely to emit a continuous, gap-filled series.

## 2. Goals / non-goals

**Goals**
- Emit a continuous run of buckets from the window start through `now`, at the
  request's granularity (daily for 7d/30d, weekly for `all`), synthesizing empty
  buckets (`rows: []`) for periods with zero spend.
- The last bucket is always *today* (the `now` bucket), so the curve reaches the
  present.
- Keep the change server-side; the frontend stacked-area chart is unchanged.

**Non-goals**
- No change to the aggregation math, the cumulative-vs-per-day semantics (already
  per-day), the API response shape, the chart type, or the range selector.
- No new "show empty range before the project existed" behavior (see §3 fill rule).

## 3. Design

All changes are in `crates/rupu-cp/src/api/usage.rs`. The frontend is untouched.

### 3.1 The fill range

Given the resolved window `[start, end]` (`resolve_window`, where `end` defaults to
`now`) and the full run list, compute:

```
earliest_overall = min(started_at) over ALL runs in the store   // not window-filtered
fill_start       = max(start, earliest_overall)                 // clamp the leading edge
fill_end         = end                                          // = now for live dashboards
```

If the store has **no runs at all**, return an empty series (nothing to plot).

This single rule covers every case the range selector produces:
- **7d / 30d** (`start` is recent): `earliest_overall` is normally older than
  `start`, so `fill_start = start` → the full selected window is drawn, including
  all-zero days (e.g. an idle 6/22–6/29 week now shows as flat-zero instead of
  vanishing).
- **all** (`start` = epoch): `fill_start = earliest_overall` → the series starts at
  the first-ever run, avoiding thousands of empty weeks back to year 0.
- **Younger than the window** (e.g. 30d window but the first run was 10 days ago):
  `fill_start = earliest_overall` → no flat-zero lead-in for days before any data
  existed.

### 3.2 Enumerating buckets

`build_timeline` gains explicit `fill_start` and `fill_end` parameters (replacing
its reliance on only the runs it was handed) so the synthesized range is
deterministic and testable. New signature:

```rust
fn build_timeline(
    runs_with_rows: Vec<(DateTime<Utc>, Vec<rupu_transcript::UsageRow>)>,
    pricing: &rupu_config::PricingConfig,
    granularity: Granularity,
    fill_start: DateTime<Utc>,
    fill_end: DateTime<Utc>,
) -> Vec<UsageTimelineBucket>
```

Behavior:
1. Group the windowed `runs_with_rows` by `bucket_key` into a `BTreeMap`
   (unchanged).
2. Enumerate every bucket key from `fill_start` to `fill_end` inclusive at the
   granularity, stepping one day (`Day`) or one week (`Week`, snapping
   `fill_start` down to its ISO-Monday first via the existing `bucket_key`
   logic, then stepping 7 days).
3. For each enumerated key, emit a `UsageTimelineBucket` with the grouped rows for
   that key, or `rows: []` when the key has no data.

The result stays chronologically ordered and the response shape
(`UsageTimelineBucket { bucket, rows }`) is unchanged — empty days are just
buckets with an empty `rows` array, which the existing frontend `toChartData`
already renders as a zero point.

### 3.3 Handler wiring

`get_usage_timeline` (`usage.rs:158`):
- After listing `runs`, compute `earliest_overall = runs.iter().map(|r| r.started_at).min()`.
- If `None` (no runs), return `Json(vec![])`.
- Otherwise compute `fill_start = start.max(earliest_overall)`, keep the existing
  window-filtered `runs_with_rows`, and call `build_timeline(.., fill_start, end)`.

`build_timeline`'s own callers in tests update to the new signature.

## 4. Edge cases

| Case | Behavior |
|---|---|
| No runs in store | Empty series (`[]`). |
| Runs exist but none in the selected window | Full window drawn as all-zero buckets (since `fill_start = start`, `fill_end = now`). |
| `all` range, runs since epoch unbounded | Series starts at first-ever run, weekly. |
| Window younger than first run | Starts at first run, no pre-existence zeros. |
| `fill_start == fill_end` (single day) | Exactly one bucket (today). |
| Week granularity | `fill_start` snapped to ISO-Monday; one bucket per week through the week containing `now`. |

## 5. Testing

Rust unit tests in `usage.rs` (extend the existing `tests` module; all use fixed
`DateTime`s, no `Utc::now()`):
- **Daily gap-fill:** runs on 6/10 and 6/12, `fill_start = 6/10`, `fill_end = 6/15`
  → six buckets 6/10..6/15, with 6/11/6/13/6/14/6/15 empty and 6/10/6/12 populated;
  last bucket equals `fill_end`'s day.
- **Reaches end with no recent activity:** runs only on 6/10, `fill_end = 6/20` →
  buckets continue through 6/20, all empty after 6/10.
- **Weekly gap-fill (`all`-style):** runs in two non-adjacent weeks → continuous
  weekly buckets between them (intervening empty week present), `fill_start` snapped
  to Monday.
- **Empty store:** handler returns `[]` (covered via a small handler-level or
  `build_timeline` guard test).
- Update the two existing tests (`build_timeline_buckets_by_day_with_per_model_breakdown`,
  `build_timeline_week_collapses_days_into_one_bucket`) to the new signature, passing
  `fill_start`/`fill_end` that match their data so their assertions still hold (plus
  the now-present continuous range).

Frontend (`UsageTimelineStacked.test.tsx`): add one case asserting a bucket with
`rows: []` becomes a zero-valued chart point (continuity), confirming the existing
transform already handles empty buckets. No component change expected.

## 6. Out of scope / notes

- The stacked-area-by-model chart style is intentionally retained (matt's choice);
  this change only makes its X-axis continuous and current.
- `make cp-web` is **not** required — this is a backend-only change; the embedded
  UI is unchanged. (If the frontend test prompts any `.tsx` edit, it does not, so
  no rebuild.)
