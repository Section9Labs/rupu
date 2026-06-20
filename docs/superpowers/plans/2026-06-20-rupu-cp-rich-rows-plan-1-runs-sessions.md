# CP Rich Rows — Plan 1 (Runs & Sessions) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A shared metric-strip row + a per-run stacked-bar list graph + a per-turn run-detail timeline, wired across the run/session lists, fed by small run-row metric additions.

**Architecture:** Backend adds `turns`/`duration_ms` to run-row DTOs via one `RunMetrics` helper (computed on the paginated page, A.4-style). Frontend gets `MetricRow`, `UsageBarChart` (recharts, from the rows' existing `usage`), and `RunUsageTimeline` (recharts, from transcript events the page already has). recharts stays in its lazy `charts` chunk.

**Tech Stack:** Rust 2021 / axum / `rupu-cp::usage` (A.3/A.4); React 18 + TS strict + recharts.

**Conventions (enforced — READ before starting):**
- Branch `feat-cp-rich-rows-p1` (created off `main`). NEVER touch `main`.
- Rust: `#![deny(clippy::all)]` incl `--all-targets`. **DO NOT run `rustfmt`/`cargo fmt`** (worktree Rust 1.95 vs pinned 1.88 → spurious drift); match style by hand; before each commit `git status --short` and `git checkout --` any drift. Stage only your files (never `-A`; untracked `.rupu/*` stays uncommitted). `rupu-cp` is clean on 1.95 — its tests + clippy are real gates.
- Frontend: NO `any`; STATIC Tailwind only (dynamic colors via inline `style`/recharts `fill`, not interpolated classes); `npm run build` strict + `npm test -- --run`; main `index-*.js` chunk must stay ~48 KB (recharts/markdown lazy). Match the project's jsdom test opt-in (`// @vitest-environment jsdom` + `import '@testing-library/jest-dom/vitest'`, see `UsageChip.test.tsx`).
- GUI rendering is matt-validated; build+tests are the automatable gate.
- End commits with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

## File structure

| File | Responsibility | Task |
|---|---|---|
| `crates/rupu-cp/src/usage.rs` | `RunMetrics` (usage + turns + duration) | 1 |
| `crates/rupu-cp/src/api/runs.rs` + `projects.rs` | `turns`/`duration_ms` on RunListRow | 2 |
| `crates/rupu-cp/src/api/run_streams.rs` | `turns`/`duration_ms` on AgentRunRow | 3 |
| `web/src/lib/duration.ts` + `components/lists/MetricRow.tsx` | shared row | 4 |
| `web/src/components/transcript/turnSeries.ts` + `components/charts/RunUsageTimeline.tsx` | per-turn timeline | 5 |
| `web/src/components/charts/UsageBarChart.tsx` | general list bar graph | 6 |
| `web/src/lib/api.ts` | `turns`/`duration_ms` types | 7 |
| `web/src/pages/runs/*` | wire run lists (rows + bar graph) | 8 |
| `web/src/pages/Sessions.tsx`, `ProjectRuns.tsx`, `ProjectSessions.tsx` | wire session/project lists | 9 |
| `web/src/pages/RunDetail.tsx` | run-detail timeline | 10 |

---

## Task 1: `RunMetrics` backend helper

**Files:** Modify `crates/rupu-cp/src/usage.rs`

Add a `RunMetrics { usage, turns, duration_ms }` computed from a run's transcripts. Turns = count of `Usage` events (one per turn); duration = the transcript's `RunComplete.duration_ms`. Reads via `rupu_transcript::JsonlReader::iter`.

- [ ] **Step 1: failing tests** — append to `usage.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn run_metrics_counts_turns_and_duration() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rupu-cp-metrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tpath = dir.join("t.jsonl");
        let mut f = std::fs::File::create(&tpath).unwrap();
        writeln!(f, r#"{{"type":"run_start","data":{{"run_id":"r1","workspace_id":"w","agent":"a","provider":"anthropic","model":"claude-sonnet-4-6","started_at":"2026-01-01T00:00:00Z","mode":"ask"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":1000,"output_tokens":200,"cached_tokens":0}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":800,"output_tokens":150,"cached_tokens":50}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"run_complete","data":{{"run_id":"r1","status":"ok","total_tokens":2150,"duration_ms":38000}}}}"#).unwrap();
        drop(f);

        let m = run_metrics_paths(&[tpath], &PricingConfig::default());
        assert_eq!(m.turns, 2);
        assert_eq!(m.duration_ms, Some(38000));
        assert_eq!(m.usage.input_tokens, 1800);
        assert_eq!(m.usage.output_tokens, 350);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

Run: `cargo test -p rupu-cp run_metrics` → FAIL.

- [ ] **Step 2: implement** — add to `usage.rs` (top imports already have `RunStore`, `PathBuf`; add `use rupu_transcript::{Event, JsonlReader};` near the other transcript imports):

```rust
/// Token usage + turn count + duration for one run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunMetrics {
    pub usage: UsageSummary,
    /// Number of LLM turns (counted from `Usage` events).
    pub turns: u64,
    /// Wall-clock duration from the transcript's `RunComplete`, if present.
    pub duration_ms: Option<u64>,
}

/// Count turns (one per `Usage` event) and capture `RunComplete.duration_ms`
/// across the given transcripts. Tolerates unreadable/partial files.
fn turns_and_duration(paths: &[PathBuf]) -> (u64, Option<u64>) {
    let mut turns = 0u64;
    let mut duration_ms = None;
    for path in paths {
        let Ok(iter) = JsonlReader::iter(path) else { continue };
        for ev in iter.flatten() {
            match ev {
                Event::Usage { .. } => turns += 1,
                Event::RunComplete { duration_ms: d, .. } => duration_ms = Some(d),
                _ => {}
            }
        }
    }
    (turns, duration_ms)
}

/// Full per-run metrics (usage + turns + duration) from transcript paths.
pub fn run_metrics_paths(paths: &[PathBuf], pricing: &PricingConfig) -> RunMetrics {
    let usage = summarize_paths(paths, pricing);
    let (turns, duration_ms) = turns_and_duration(paths);
    RunMetrics { usage, turns, duration_ms }
}

/// Full per-run metrics for a run in the store.
pub fn run_metrics(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> RunMetrics {
    run_metrics_paths(&run_transcript_paths(store, run_id), pricing)
}
```

(If `JsonlReader::iter` returns `Result<impl Iterator<Item=Result<Event,_>>,_>`, the `iter.flatten()` above drops parse errors — correct. Confirm the signature in `crates/rupu-transcript/src/reader.rs`; adapt the `for ev in` loop to the actual iterator shape, keeping error-tolerance.)

- [ ] **Step 3:** `cargo test -p rupu-cp run_metrics` → PASS; `cargo test -p rupu-cp` green; `cargo clippy -p rupu-cp --all-targets` clean.
- [ ] **Step 4: commit** `git add crates/rupu-cp/src/usage.rs` → `feat(cp): RunMetrics — usage + turn count + duration`

---

## Task 2: `turns`/`duration_ms` on `RunListRow`

**Files:** Modify `crates/rupu-cp/src/api/runs.rs`, `crates/rupu-cp/src/api/projects.rs`

`RunListRow::with_usage` currently fills `usage` via `summarize_run`. Extend to fill `turns` + `duration_ms` via `run_metrics`.

- [ ] **Step 1: failing test** — in `runs.rs` tests, extend `run_list_row_serializes_usage` (or add a test) asserting the serialized row has `turns` and `duration_ms` keys (default 0 / null). Add the fields to the struct literal in the test.
- [ ] **Step 2: implement** —
  1. Add to `RunListRow`: `pub(crate) turns: u64,` and `pub(crate) duration_ms: Option<u64>,`.
  2. `From<&RunRecord>` defaults them (`turns: 0, duration_ms: None`).
  3. `with_usage` becomes metric-filling:
  ```rust
  pub(crate) fn with_usage(
      r: &RunRecord,
      store: &rupu_orchestrator::runs::RunStore,
      pricing: &rupu_config::PricingConfig,
  ) -> Self {
      let mut row = Self::from(r);
      let m = crate::usage::run_metrics(store, &r.id, pricing);
      row.usage = m.usage;
      row.turns = m.turns;
      // Prefer the run record's wall-clock when both timestamps exist; else the transcript duration.
      row.duration_ms = match (r.finished_at, r.started_at) {
          (Some(fin), start) => Some((fin - start).num_milliseconds().max(0) as u64),
          _ => m.duration_ms,
      };
      row
  }
  ```
  (`RunRecord.started_at: DateTime<Utc>`, `finished_at: Option<DateTime<Utc>>` — chrono `Duration::num_milliseconds`.)
- [ ] **Step 3:** `cargo test -p rupu-cp` green; `cargo clippy -p rupu-cp --all-targets` clean; `cargo build -p rupu-cp` builds (projects.rs `recent_runs` uses `From` default — still compiles). `git status --short` only runs.rs (projects.rs unchanged here — `project_runs` already uses `with_usage`, so it gets metrics free).
- [ ] **Step 4: commit** `git add crates/rupu-cp/src/api/runs.rs` → `feat(cp): turns + duration on run list rows`

---

## Task 3: `turns`/`duration_ms` on `AgentRunRow`

**Files:** Modify `crates/rupu-cp/src/api/run_streams.rs`

Agent runs fill usage via `summarize_paths(&[transcript_path])`; switch to `run_metrics_paths` to also get turns + duration.

- [ ] **Step 1:** Add fields to `AgentRunRow` (after `usage`): `turns: u64,` and `duration_ms: Option<u64>,`. Default them in both collect helpers (`turns: 0, duration_ms: None`).
- [ ] **Step 2:** In `list_agent_runs`, change the per-row fill loop:
```rust
    for row in &mut page_rows {
        if let Some(tp) = &row.transcript_path {
            let m = crate::usage::run_metrics_paths(&[std::path::PathBuf::from(tp)], &s.pricing);
            row.usage = m.usage;
            row.turns = m.turns;
            row.duration_ms = m.duration_ms;
        }
    }
```
- [ ] **Step 3:** `cargo test -p rupu-cp` green; `cargo clippy -p rupu-cp --all-targets` clean; `git status --short` only run_streams.rs.
- [ ] **Step 4: commit** `git add crates/rupu-cp/src/api/run_streams.rs` → `feat(cp): turns + duration on agent-run rows`

---

## Task 4: `formatDuration` + `MetricRow` component

**Files:** Create `web/src/lib/duration.ts`, `web/src/lib/duration.test.ts`, `web/src/components/lists/MetricRow.tsx`, `web/src/components/lists/MetricRow.test.tsx`

- [ ] **Step 1: failing test** for `formatDuration` (`duration.test.ts`):
```ts
import { describe, it, expect } from 'vitest';
import { formatDuration } from './duration';
describe('formatDuration', () => {
  it('formats ms', () => {
    expect(formatDuration(38000)).toBe('38s');
    expect(formatDuration(1500)).toBe('1.5s');
    expect(formatDuration(125000)).toBe('2m 5s');
    expect(formatDuration(null)).toBe('—');
    expect(formatDuration(450)).toBe('450ms');
  });
});
```
- [ ] **Step 2: implement** `duration.ts`:
```ts
/** Human duration from milliseconds. `null`/undefined → em-dash. */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null) return '—';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  if (s < 10) return `${(Math.round(s * 10) / 10)}s`;
  if (s < 60) return `${Math.round(s)}s`;
  const m = Math.floor(s / 60);
  const rem = Math.round(s % 60);
  return `${m}m ${rem}s`;
}
```
- [ ] **Step 3: failing test** for `MetricRow` (`MetricRow.test.tsx`, jsdom):
```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import MetricRow from './MetricRow';

describe('MetricRow', () => {
  it('renders the header and non-null metrics, omits null', () => {
    render(
      <MemoryRouter>
        <MetricRow
          to="/runs/x"
          header={<span>oracle-assessor</span>}
          metrics={[
            { label: 'in', value: '3,180' },
            { label: 'cost', value: '$0.03' },
            { label: 'turns', value: null },
          ]}
        />
      </MemoryRouter>,
    );
    expect(screen.getByText('oracle-assessor')).toBeInTheDocument();
    expect(screen.getByText('3,180')).toBeInTheDocument();
    expect(screen.getByText('cost')).toBeInTheDocument();
    expect(screen.queryByText('turns')).not.toBeInTheDocument();
  });
});
```
- [ ] **Step 4: implement** `MetricRow.tsx`:
```tsx
import { Link } from 'react-router-dom';
import type { ReactNode } from 'react';

export interface Metric {
  label: string;
  /** null/undefined → the metric is omitted (genuinely-absent ≠ zero). */
  value: string | null | undefined;
}

/**
 * Shared list row (the "metric strip" design): a header line (identity +
 * chips + a trailing node such as a status pill) above a labeled stat strip.
 * Wrap in a Link when `to` is set, else a plain div.
 */
export default function MetricRow({
  header,
  trailing,
  metrics,
  to,
}: {
  header: ReactNode;
  trailing?: ReactNode;
  metrics: Metric[];
  to?: string;
}) {
  const body = (
    <div className="px-4 py-2.5">
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1 flex items-center gap-2 flex-wrap">{header}</div>
        {trailing}
      </div>
      <div className="mt-1.5 flex items-end gap-4 flex-wrap">
        {metrics
          .filter((m) => m.value != null)
          .map((m) => (
            <span key={m.label} className="inline-flex flex-col leading-tight">
              <span className="text-[13px] font-semibold text-ink tabular-nums">{m.value}</span>
              <span className="text-[9px] uppercase tracking-wide text-ink-mute">{m.label}</span>
            </span>
          ))}
      </div>
    </div>
  );
  return to ? (
    <Link to={to} className="block hover:bg-slate-50 transition-colors">
      {body}
    </Link>
  ) : (
    <div className="hover:bg-slate-50 transition-colors">{body}</div>
  );
}
```
- [ ] **Step 5:** `npm test -- --run duration MetricRow` → pass; `npm run build` strict exit 0.
- [ ] **Step 6: commit** `git add web/src/lib/duration.ts web/src/lib/duration.test.ts web/src/components/lists/MetricRow.tsx web/src/components/lists/MetricRow.test.tsx` → `feat(cp/web): MetricRow + formatDuration`

---

## Task 5: `buildTurnSeries` + `RunUsageTimeline`

**Files:** Create `web/src/components/transcript/turnSeries.ts`, `turnSeries.test.ts`, `web/src/components/charts/RunUsageTimeline.tsx`

`buildTurnSeries` is pure: from the transcript events (the `TranscriptEvent` type in `lib/transcript.ts` — `usage` events carry `input_tokens`/`output_tokens`/`cached_tokens`), produce ordered per-turn points. Each `usage` event = one turn.

- [ ] **Step 1: failing test** (`turnSeries.test.ts`):
```ts
import { describe, it, expect } from 'vitest';
import { buildTurnSeries } from './turnSeries';
import type { TranscriptEvent } from '../../lib/transcript';

describe('buildTurnSeries', () => {
  it('maps usage events to ordered per-turn points', () => {
    const events = [
      { type: 'run_start', data: {} },
      { type: 'usage', data: { input_tokens: 1000, output_tokens: 200, cached_tokens: 0 } },
      { type: 'usage', data: { input_tokens: 800, output_tokens: 150, cached_tokens: 50 } },
    ] as unknown as TranscriptEvent[];
    const s = buildTurnSeries(events);
    expect(s).toEqual([
      { turn: 1, tokens_in: 1000, tokens_out: 200, tokens_cached: 0 },
      { turn: 2, tokens_in: 800, tokens_out: 150, tokens_cached: 50 },
    ]);
  });
  it('returns [] when no usage events', () => {
    expect(buildTurnSeries([{ type: 'run_start', data: {} }] as unknown as TranscriptEvent[])).toEqual([]);
  });
});
```
- [ ] **Step 2: implement** `turnSeries.ts`:
```ts
import type { TranscriptEvent } from '../../lib/transcript';

export interface TurnUsagePoint {
  turn: number;
  tokens_in: number;
  tokens_out: number;
  tokens_cached: number;
}

/** Build an ordered per-turn token series from a transcript's events.
 *  One `usage` event = one turn; x-axis is the 1-based turn index. */
export function buildTurnSeries(events: TranscriptEvent[]): TurnUsagePoint[] {
  const out: TurnUsagePoint[] = [];
  for (const ev of events) {
    if (ev.type === 'usage') {
      const d = ev.data;
      out.push({
        turn: out.length + 1,
        tokens_in: d.input_tokens ?? 0,
        tokens_out: d.output_tokens ?? 0,
        tokens_cached: d.cached_tokens ?? 0,
      });
    }
  }
  return out;
}
```
(Confirm the `usage` variant's field shape in `lib/transcript.ts` — the A.2 `TranscriptEvent` union has `{ type: 'usage'; data: { input_tokens; output_tokens; cached_tokens } }`. Narrow on `ev.type === 'usage'` so TS knows `data`'s shape — no `any`.)
- [ ] **Step 3: implement** `RunUsageTimeline.tsx` (recharts stacked area over turn index):
```tsx
import { Area, AreaChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import type { TurnUsagePoint } from '../transcript/turnSeries';
import { formatTokens } from '../../lib/usage';

const tooltipStyle: React.CSSProperties = {
  background: '#fff', border: '1px solid #e2e8f0', borderRadius: 6, fontSize: 11, padding: '6px 10px',
};

/** Per-turn token timeline (in/out/cached stacked) for the run-detail page. */
export default function RunUsageTimeline({ series }: { series: TurnUsagePoint[] }) {
  if (series.length === 0) {
    return <div className="text-xs text-ink-mute py-6 text-center">No per-turn usage yet</div>;
  }
  return (
    <div style={{ width: '100%', height: 120 }}>
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={series} margin={{ top: 4, right: 8, bottom: 0, left: 0 }}>
          <XAxis dataKey="turn" tick={{ fontSize: 10, fill: '#94a3b8' }} />
          <YAxis tick={{ fontSize: 10, fill: '#94a3b8' }} width={36}
            tickFormatter={(v) => formatTokens(typeof v === 'number' ? v : 0)} />
          <Tooltip contentStyle={tooltipStyle}
            formatter={(v, name) => [formatTokens(typeof v === 'number' ? v : 0), String(name)]}
            labelFormatter={(l) => `turn ${l}`} />
          <Area type="monotone" dataKey="tokens_in" name="in" stackId="1" stroke="#1860f2" fill="#1860f2" fillOpacity={0.18} />
          <Area type="monotone" dataKey="tokens_out" name="out" stackId="1" stroke="#22c55e" fill="#22c55e" fillOpacity={0.18} />
          <Area type="monotone" dataKey="tokens_cached" name="cached" stackId="1" stroke="#f59e0b" fill="#f59e0b" fillOpacity={0.18} />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}
```
- [ ] **Step 4:** `npm test -- --run turnSeries` → pass; `npm run build` strict exit 0; confirm main chunk ~48 KB + `grep -c recharts dist/assets/index-*.js` → 0.
- [ ] **Step 5: commit** `git add web/src/components/transcript/turnSeries.ts web/src/components/transcript/turnSeries.test.ts web/src/components/charts/RunUsageTimeline.tsx` → `feat(cp/web): per-turn RunUsageTimeline + buildTurnSeries`

---

## Task 6: `UsageBarChart` (general list graph)

**Files:** Create `web/src/components/charts/UsageBarChart.tsx`, `UsageBarChart.test.tsx`

A stacked bar per row (in/out/cached), built from the list's `usage`. Input is a generic `UsageBar[]`.

- [ ] **Step 1: implement** `UsageBarChart.tsx`:
```tsx
import { Bar, BarChart, Cell, ResponsiveContainer, Tooltip, XAxis } from 'recharts';
import { useNavigate } from 'react-router-dom';
import { formatTokens, formatCost } from '../../lib/usage';

export interface UsageBar {
  id: string;
  label: string;
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  cost_usd: number | null;
  to?: string;
}

const tooltipStyle: React.CSSProperties = {
  background: '#fff', border: '1px solid #e2e8f0', borderRadius: 6, fontSize: 11, padding: '6px 10px',
};

/** Per-run stacked token bars (in/out/cached) summarising the loaded list. */
export default function UsageBarChart({ bars }: { bars: UsageBar[] }) {
  const navigate = useNavigate();
  const total = bars.reduce((a, b) => a + b.input_tokens + b.output_tokens + b.cached_tokens, 0);
  if (bars.length === 0 || total === 0) {
    return <div className="text-xs text-ink-mute py-6 text-center">No token usage in this list yet</div>;
  }
  return (
    <div style={{ width: '100%', height: 96 }}>
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={bars} margin={{ top: 4, right: 4, bottom: 0, left: 0 }} barCategoryGap={2}>
          <XAxis dataKey="label" hide />
          <Tooltip
            contentStyle={tooltipStyle}
            formatter={(v, name) => [formatTokens(typeof v === 'number' ? v : 0), String(name)]}
            labelFormatter={(_l, payload) => {
              const p = payload?.[0]?.payload as UsageBar | undefined;
              return p ? `${p.label} · ${formatCost(p.cost_usd)}` : '';
            }}
          />
          <Bar dataKey="input_tokens" name="in" stackId="t" fill="#1860f2"
            onClick={(d) => { const b = d?.payload as UsageBar; if (b?.to) navigate(b.to); }}>
            {bars.map((b) => <Cell key={b.id} cursor={b.to ? 'pointer' : 'default'} />)}
          </Bar>
          <Bar dataKey="output_tokens" name="out" stackId="t" fill="#22c55e" />
          <Bar dataKey="cached_tokens" name="cached" stackId="t" fill="#f59e0b" />
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}
```
- [ ] **Step 2: test** (`UsageBarChart.test.tsx`, jsdom) — renders an empty state for `[]`, and renders (no crash) for a couple of bars. (recharts needs a sized container; in jsdom assert the empty-state path + that a non-empty render does not throw.)
```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import UsageBarChart from './UsageBarChart';

describe('UsageBarChart', () => {
  it('shows empty state when no usage', () => {
    render(<MemoryRouter><UsageBarChart bars={[]} /></MemoryRouter>);
    expect(screen.getByText(/No token usage/)).toBeInTheDocument();
  });
  it('renders without crashing for non-empty', () => {
    const bars = [{ id: 'a', label: 'a', input_tokens: 100, output_tokens: 20, cached_tokens: 0, cost_usd: 0.01 }];
    render(<MemoryRouter><UsageBarChart bars={bars} /></MemoryRouter>);
  });
});
```
- [ ] **Step 3:** `npm test -- --run UsageBarChart` → pass; `npm run build` strict exit 0; main chunk ~48 KB.
- [ ] **Step 4: commit** `git add web/src/components/charts/UsageBarChart.tsx web/src/components/charts/UsageBarChart.test.tsx` → `feat(cp/web): UsageBarChart general list graph`

---

## Task 7: API types (`turns`/`duration_ms`)

**Files:** Modify `web/src/lib/api.ts`

- [ ] Add to `RunListRow` interface: `turns: number;` and `duration_ms?: number | null;`. Add the same two to `AgentRunRow`. Build (`npm run build` strict exit 0) + `npm test -- --run api` green.
- [ ] **commit** `git add web/src/lib/api.ts` → `feat(cp/web): turns + duration_ms row types`

---

## Task 8: Wire the run lists (rows + bar graph)

**Files:** Modify `web/src/pages/runs/WorkflowRuns.tsx`, `runs/AgentRuns.tsx`

Replace each row's hand-rolled markup with `MetricRow`, and add a `UsageBarChart` above the list (built from the loaded rows). The lists already accumulate `runs`/rows via the A.4 infinite-scroll.

- [ ] **WorkflowRuns:** READ the file. Above the grouped list (but inside the data branch), add:
```tsx
import UsageBarChart from '../../components/charts/UsageBarChart';
// ...
{runs.length > 0 && (
  <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
    <UsageBarChart bars={runs.map((r) => ({
      id: r.id, label: r.workflow_name, to: `/runs/${encodeURIComponent(r.id)}`,
      input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
      cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
    }))} />
  </div>
)}
```
Replace `WorkflowRunRow` body with `MetricRow`:
```tsx
function WorkflowRunRow({ run }: { run: RunListRow }) {
  return (
    <MetricRow
      to={`/runs/${encodeURIComponent(run.id)}`}
      header={<>
        <span className="text-sm font-medium text-ink truncate">{run.workflow_name}</span>
        <span className="text-[11px] text-ink-mute font-mono">{shortId(run.id)}</span>
        <TriggerChip trigger={run.trigger} />
      </>}
      trailing={<StatusPill status={run.status} />}
      metrics={[
        { label: 'in', value: formatTokens(run.usage.input_tokens) },
        { label: 'out', value: formatTokens(run.usage.output_tokens) },
        { label: 'cached', value: run.usage.cached_tokens ? formatTokens(run.usage.cached_tokens) : null },
        { label: 'cost', value: formatCost(run.usage.cost_usd) },
        { label: 'duration', value: run.duration_ms != null ? formatDuration(run.duration_ms) : durationBetween(run.started_at, run.finished_at) },
        { label: 'turns', value: run.turns ? String(run.turns) : null },
      ]}
    />
  );
}
```
Imports: `MetricRow` from `../../components/lists/MetricRow`, `formatTokens`/`formatCost` from `../../lib/usage`, `formatDuration` from `../../lib/duration`. (Keep `shortId`, `TriggerChip`, `StatusPill`, `durationBetween`.)
- [ ] **AgentRuns:** same treatment — `UsageBarChart` above (label = `run.agent ?? run.run_id`, `to` = the transcript or run link the row already uses), `AgentRunEntry` → `MetricRow` (header = agent + id + SourceChip + status badge; metrics = in/out/cached/cost/duration(from `duration_ms`)/turns; the "View transcript →" affordance can move into the header trailing slot).
- [ ] **Verify:** `npm run build` strict exit 0; `npm test -- --run` green; main chunk ~48 KB.
- [ ] **commit** `git add web/src/pages/runs/WorkflowRuns.tsx web/src/pages/runs/AgentRuns.tsx` → `feat(cp/web): metric rows + usage bar graph on run lists`

(Autoflow rows are left as-is in Plan 1 — cycles/events have a different shape; revisit only if matt asks. Note this in the report.)

---

## Task 9: Wire Sessions + project lists

**Files:** Modify `web/src/pages/Sessions.tsx`, `web/src/pages/ProjectRuns.tsx`, `web/src/pages/ProjectSessions.tsx`

- [ ] **Sessions:** `SessionRow` → `MetricRow` (header = status dot+label + agent + id; trailing = the active-run pill; metrics = in/out/cached/cost from `session.usage` when present, `total_turns` as turns, duration from `created_at`→`updated_at`). Add `UsageBarChart` above (bars from `session.usage`, label = agent name, `to` = the session link); guard for sessions lacking `usage`.
- [ ] **ProjectRuns / ProjectSessions:** same `MetricRow` treatment as WorkflowRuns / Sessions respectively (they render `RunListRow` / `SessionSummary`), plus a `UsageBarChart` above each.
- [ ] **Verify:** `npm run build` strict exit 0; `npm test -- --run` green.
- [ ] **commit** the three files → `feat(cp/web): metric rows + usage graph on sessions + project lists`

---

## Task 10: Run-detail per-turn timeline

**Files:** Modify `web/src/pages/RunDetail.tsx`

The page fetches the run graph + (for the transcript) the events. Build the per-turn series from the transcript events and render `RunUsageTimeline` at the top of the run detail. READ the page to see how it obtains transcript events: if it doesn't already fetch them, fetch via `api.getTranscript(path)` for the run's transcript (the step results carry transcript paths; use the primary/first one) OR, simplest, render the timeline inside the existing transcript panel area. Pick the lowest-friction wiring and describe it.

- [ ] Add `import RunUsageTimeline from '../components/charts/RunUsageTimeline';` + `import { buildTurnSeries } from '../components/transcript/turnSeries';`. Build `const series = buildTurnSeries(events)` from whatever events the page has, and render `<RunUsageTimeline series={series} />` in a panel near the run header (under the token/cost breakdown from A.3). For a LIVE run, the events stream already updates → the chart re-renders. If the page has no events handy, fetch the primary transcript's events once for completed runs and skip for live (note the choice).
- [ ] **Verify:** `npm run build` strict exit 0; `npm test -- --run` green; main chunk ~48 KB.
- [ ] **commit** `git add web/src/pages/RunDetail.tsx` → `feat(cp/web): per-turn usage timeline on run detail`

---

## Task 11: Whole-slice gate

- [ ] `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` clean; `npm run build` strict exit 0 + `npm test -- --run` (paste counts) + main `index-*.js` chunk ~48 KB + `grep -c recharts dist/assets/index-*.js` → 0; `grep -rn ": any\|as any\| bg-\${" web/src/components/lists/MetricRow.tsx web/src/components/charts/` → none; `git status --short` clean. Visual handoff checklist for matt: run/session lists show metric-strip rows + the per-run bar graph on top; run detail shows the per-turn timeline (animating on a live run).

---

## Done criteria
- Run-row DTOs carry `turns` + `duration_ms`; computed on the paginated page only.
- `MetricRow`, `UsageBarChart`, `RunUsageTimeline` shipped + wired across the run/session/project lists + run detail.
- No `rupu-cli` dep added; no rustfmt drift; main chunk ~48 KB; no `any`; static Tailwind. Aggregate areas (projects/agents/workflows entity lists) are Plan 2.
