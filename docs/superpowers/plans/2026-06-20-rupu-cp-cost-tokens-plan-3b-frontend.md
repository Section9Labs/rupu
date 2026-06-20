# CP Cost & Tokens — Plan 3b (Frontend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render the token + cost data that Plan 3a exposes — a compact usage chip on run / session / project / workflow surfaces and a usage panel on the Dashboard.

**Architecture:** A pure `lib/usage.ts` (types + `formatCost`/`formatTokens`, no price logic) and a presentational `UsageChip` component, wired into the existing list/detail pages. The Dashboard gains a usage panel fed by the new `GET /api/usage`. All computation stays server-side; the frontend only formats and displays.

**Tech Stack:** React 18 + TypeScript (strict) + Vite + Tailwind + recharts (already a dep, used only in `Dashboard.tsx`).

**Prerequisite:** Plan 3a is merged (or stacked beneath this). The API now returns a `usage` object on run/session/project/workflow responses and serves `GET /api/usage`.

**Conventions (enforced — read before starting):**
- Work on branch `feat-cp-cost-tokens` (continues Plan 3a). NEVER touch `main`.
- No `any` (TS strict). STATIC Tailwind only — never interpolate class names (`bg-${x}`); use static class strings / maps + inline `style` for dynamic colors (matches `Dashboard.tsx`'s `STATUS_FILL` pattern).
- `npm run build` must stay strict-clean; the main entry chunk must stay ~48 KB (recharts and markdown remain in their own lazy chunks).
- Stage ONLY the files you changed (`git add <specific paths>`, never `-A`).
- GUI rendering is validated by matt before merge; build + tests are the automatable gate.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- All work is under `crates/rupu-cp/web/`. Run npm commands from there.

---

## File structure

| File | Responsibility | Task |
|---|---|---|
| `src/lib/usage.ts` (new) + `usage.test.ts` | `UsageSummary`/`UsageBreakdownRow`/`UsageOverview` types + `formatCost`/`formatTokens` | 1 |
| `src/components/UsageChip.tsx` (new) + test | the inline `· N tok · $X` chip | 2 |
| `src/lib/api.ts` | add `usage` to existing types + `UsageOverview` + `getUsage` | 3 |
| `src/pages/runs/WorkflowRuns.tsx`, `runs/AgentRuns.tsx`, `ProjectRuns.tsx` | per-run chip | 4 |
| `src/pages/RunDetail.tsx` | token/cost breakdown | 5 |
| `src/pages/SessionDetail.tsx`, `Sessions.tsx`, `ProjectSessions.tsx` | per-session chip | 6 |
| `src/pages/ProjectDetail.tsx` | project rollup stat | 7 |
| `src/pages/Dashboard.tsx` | usage panel (total spend + top models) | 8 |

---

## Task 1: `lib/usage.ts` — types + formatters

**Files:**
- Create: `src/lib/usage.ts`, `src/lib/usage.test.ts`

Mirror the backend DTOs and provide dependency-free formatters (same style as `lib/time.ts`). `formatTokens` compacts large counts; `formatCost` shows `—` when there's no price, more decimals for sub-dollar amounts.

- [ ] **Step 1: Write the failing test**

Create `src/lib/usage.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { formatTokens, formatCost } from './usage';

describe('formatTokens', () => {
  it('renders small counts with thousands separators', () => {
    expect(formatTokens(0)).toBe('0');
    expect(formatTokens(4210)).toBe('4,210');
    expect(formatTokens(999999)).toBe('999,999');
  });
  it('compacts millions and billions', () => {
    expect(formatTokens(1_200_000)).toBe('1.2M');
    expect(formatTokens(3_400_000_000)).toBe('3.4B');
  });
});

describe('formatCost', () => {
  it('renders an em-dash when unpriced (null)', () => {
    expect(formatCost(null)).toBe('—');
  });
  it('shows 4 decimals under a dollar, 2 at or above', () => {
    expect(formatCost(0.0312)).toBe('$0.0312');
    expect(formatCost(12.5)).toBe('$12.50');
    expect(formatCost(0)).toBe('$0.0000');
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- --run usage`
Expected: FAIL — module `./usage` not found.

- [ ] **Step 3: Implement**

Create `src/lib/usage.ts`:

```ts
// Token + cost types (mirror of the rupu-cp `usage` DTOs) and dependency-free
// formatters. No price logic lives here — the backend computes all cost; this
// only formats numbers for display.

export interface UsageSummary {
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  total_tokens: number;
  /** null when no contributing model was priced (a partial total when `priced` is false). */
  cost_usd: number | null;
  /** false when at least one contributing model lacked a price. */
  priced: boolean;
  runs: number;
}

export interface UsageBreakdownRow {
  provider: string;
  model: string;
  agent: string;
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  total_tokens: number;
  cost_usd: number | null;
  priced: boolean;
  runs: number;
}

export interface UsageOverview {
  summary: UsageSummary;
  breakdown: UsageBreakdownRow[];
}

/** Compact a token count: `4,210` / `1.2M` / `3.4B`. */
export function formatTokens(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  return n.toLocaleString('en-US');
}

/** Format a USD cost. `null` → em-dash. Sub-dollar amounts get 4 decimals
 *  (small per-run costs stay legible); larger amounts get 2. */
export function formatCost(cost: number | null): string {
  if (cost === null || cost === undefined) return '—';
  return `$${cost.toFixed(cost < 1 ? 4 : 2)}`;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- --run usage`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lib/usage.ts src/lib/usage.test.ts
git commit -m "feat(cp/web): usage types + formatCost/formatTokens

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `UsageChip` component

**Files:**
- Create: `src/components/UsageChip.tsx`, `src/components/UsageChip.test.tsx`

A compact inline chip: `· 4,210 tok · $0.03`. Shows `—` for cost when unpriced; a subtle `*` + `title` when the cost is a partial (priced but `priced === false`). Renders a dim `—` for tokens when `total_tokens === 0`.

- [ ] **Step 1: Write the failing test**

Create `src/components/UsageChip.test.tsx`:

```tsx
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import UsageChip from './UsageChip';
import type { UsageSummary } from '../lib/usage';

const base: UsageSummary = {
  input_tokens: 0, output_tokens: 0, cached_tokens: 0,
  total_tokens: 0, cost_usd: null, priced: true, runs: 0,
};

describe('UsageChip', () => {
  it('shows tokens and cost when priced', () => {
    render(<UsageChip usage={{ ...base, total_tokens: 4210, cost_usd: 0.03, priced: true }} />);
    expect(screen.getByText(/4,210 tok/)).toBeInTheDocument();
    expect(screen.getByText(/\$0\.0300/)).toBeInTheDocument();
  });
  it('renders an em-dash for unpriced cost', () => {
    render(<UsageChip usage={{ ...base, total_tokens: 100, cost_usd: null, priced: false }} />);
    expect(screen.getByText('—')).toBeInTheDocument();
  });
  it('marks a partial cost', () => {
    render(<UsageChip usage={{ ...base, total_tokens: 100, cost_usd: 3, priced: false }} />);
    expect(screen.getByText(/\*/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- --run UsageChip`
Expected: FAIL — `./UsageChip` not found.

- [ ] **Step 3: Implement**

Create `src/components/UsageChip.tsx`:

```tsx
import type { UsageSummary } from '../lib/usage';
import { formatTokens, formatCost } from '../lib/usage';

/**
 * Compact inline usage chip: `· 4,210 tok · $0.03`.
 * - Cost shows `—` when unpriced (`cost_usd === null`).
 * - A partial cost (some models unpriced, `priced === false` but a cost exists)
 *   is suffixed with `*` and a hover title.
 */
export default function UsageChip({
  usage,
  className = '',
}: {
  usage: UsageSummary;
  className?: string;
}) {
  const partial = usage.cost_usd !== null && !usage.priced;
  const costTitle = partial
    ? 'Partial — some models have no price configured'
    : undefined;
  return (
    <span className={`inline-flex items-center gap-1.5 text-[11px] text-ink-mute tabular-nums ${className}`}>
      <span>{formatTokens(usage.total_tokens)} tok</span>
      <span className="text-border">·</span>
      <span title={costTitle} className={partial ? 'text-amber-600' : undefined}>
        {formatCost(usage.cost_usd)}{partial ? '*' : ''}
      </span>
    </span>
  );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- --run UsageChip`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
git add src/components/UsageChip.tsx src/components/UsageChip.test.tsx
git commit -m "feat(cp/web): UsageChip — inline tokens + cost

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Extend API types + `getUsage`

**Files:**
- Modify: `src/lib/api.ts`

Add the `usage` field to the existing response types and a `getUsage` method for the overview. Re-export the usage types through `api.ts` (the same way transcript types are re-exported at the top of the file).

- [ ] **Step 1: Re-export usage types**

Near the top of `src/lib/api.ts`, beside the existing `export type { TranscriptEvent, TranscriptResponse } from './transcript';` (line 11), add:

```ts
export type { UsageSummary, UsageBreakdownRow, UsageOverview } from './usage';
```

And add an import for the type used in method return positions:

```ts
import type { UsageOverview } from './usage';
import type { UsageSummary } from './usage';
```

(If the file groups imports at the top, add these there; otherwise the `export type` re-export above already brings the names into scope for annotations — in that case skip the duplicate `import type` and reference `UsageSummary` via the re-export. Keep TS strict happy: a single `import type { UsageSummary, UsageOverview } from './usage';` plus the `export type { … }` re-export is the clean shape.)

- [ ] **Step 2: Add `usage` to existing interfaces**

- `RunListRow` (line ~260) — add:
  ```ts
    usage: UsageSummary;
  ```
- `SessionSummary` (line ~425) — add the token fields the backend now serializes plus the usage object:
  ```ts
    provider_name?: string;
    total_tokens_in?: number;
    total_tokens_out?: number;
    total_tokens_cached?: number;
    usage?: UsageSummary;
  ```
- `ProjectDetail` (line ~590) — add:
  ```ts
    usage: UsageSummary;
  ```
- `WorkflowDetail` (line ~415) — add:
  ```ts
    usage?: UsageSummary;
  ```

- [ ] **Step 3: Update `getRun` return type + add `getUsage`**

Change `getRun` (line ~631) to include usage:

```ts
  getRun(id: string): Promise<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }> {
    return request<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }>(
      `/api/runs/${encodeURIComponent(id)}`,
    );
  },
```

Add a `getUsage` method in the `api` object (next to `getDashboard`):

```ts
  getUsage(params?: { since?: string; until?: string; groupBy?: 'provider' | 'model' | 'agent' }): Promise<UsageOverview> {
    const q = new URLSearchParams();
    if (params?.since) q.set('since', params.since);
    if (params?.until) q.set('until', params.until);
    if (params?.groupBy) q.set('group_by', params.groupBy);
    const qs = q.toString();
    return request<UsageOverview>(`/api/usage${qs ? `?${qs}` : ''}`);
  },
```

- [ ] **Step 4: Add an api test (mirror existing `api.test.ts` style)**

Read `src/lib/api.test.ts` for its fetch-mock pattern, then add a test asserting `getUsage({ groupBy: 'model' })` requests `/api/usage?group_by=model`. Example shape (adapt to the file's existing mock helper):

```ts
it('getUsage builds the group_by query', async () => {
  const spy = mockFetchOnce({ summary: {}, breakdown: [] });
  await api.getUsage({ groupBy: 'model' });
  expect(spy).toHaveBeenCalledWith(expect.stringContaining('/api/usage?group_by=model'), expect.anything());
});
```

- [ ] **Step 5: Typecheck + test + build**

Run: `npm test -- --run api` then `npm run build`
Expected: PASS / strict build exits 0.

- [ ] **Step 6: Commit**

```bash
git add src/lib/api.ts src/lib/api.test.ts
git commit -m "feat(cp/web): API types for usage + getUsage

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Per-run usage chip on the run-stream pages

**Files:**
- Modify: `src/pages/runs/WorkflowRuns.tsx`, `src/pages/runs/AgentRuns.tsx`, `src/pages/ProjectRuns.tsx`

Show a `UsageChip` on each run row. The `RunListRow` now carries `usage` (Task 3).

- [ ] **Step 1: WorkflowRuns — import + render**

In `src/pages/runs/WorkflowRuns.tsx`, add the import:

```ts
import UsageChip from '../../components/UsageChip';
```

Find the run-row JSX (the row renders `workflow_name`, `shortId`, `StatusPill`, etc.). Add the chip beside the existing meta line — locate the element that shows duration / relative time for a row `r: RunListRow` and append:

```tsx
            <UsageChip usage={r.usage} className="ml-2" />
```

(Place it in the row's metadata cluster — next to the `relativeTime` / `durationBetween` text — so it reads `… started 3m ago · 4,210 tok · $0.03`.)

- [ ] **Step 2: AgentRuns — chip where a run row is rendered**

`AgentRunRow` from `/api/runs/agents` does NOT carry a `usage` field (it's a different DTO). Skip AgentRuns for the chip **unless** the row links to a `RunListRow`-shaped record. To avoid inventing data: in `src/pages/runs/AgentRuns.tsx`, do NOT add a chip (the per-run usage there would require a separate fetch). Leave a one-line code comment where a future per-agent-run usage could attach:

```tsx
{/* Per-run token/cost: AgentRunRow has no usage field (different DTO); shown on the run detail page instead. */}
```

- [ ] **Step 3: ProjectRuns — import + render**

`src/pages/ProjectRuns.tsx` renders `RunListRow[]` from `getProjectRuns` (which Plan 3a fills with usage). Add the same import and chip as WorkflowRuns:

```ts
import UsageChip from '../components/UsageChip';
```

and in its run row:

```tsx
            <UsageChip usage={r.usage} className="ml-2" />
```

- [ ] **Step 4: Build + test**

Run: `npm run build` then `npm test -- --run`
Expected: strict build exits 0; tests green.

- [ ] **Step 5: Commit**

```bash
git add src/pages/runs/WorkflowRuns.tsx src/pages/runs/AgentRuns.tsx src/pages/ProjectRuns.tsx
git commit -m "feat(cp/web): per-run usage chip on run-stream pages

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Token/cost breakdown on Run detail

**Files:**
- Modify: `src/pages/RunDetail.tsx`

`getRun` now returns `usage`. Show a small breakdown (input / output / cached / total tokens + cost) in the run header.

- [ ] **Step 1: Implement**

In `src/pages/RunDetail.tsx`, add the import:

```ts
import { formatTokens, formatCost } from '../lib/usage';
```

The page already destructures the `getRun` result (`run`, `steps`). Capture `usage` too (e.g. `const { run, steps, usage } = data;` where `data` is the fetched object — match the file's existing state shape). Render a compact stat row in the header area:

```tsx
{usage && (
  <div className="flex items-center gap-4 text-xs text-ink-dim tabular-nums">
    <span><span className="text-ink-mute">in</span> {formatTokens(usage.input_tokens)}</span>
    <span><span className="text-ink-mute">out</span> {formatTokens(usage.output_tokens)}</span>
    {usage.cached_tokens > 0 && (
      <span><span className="text-ink-mute">cached</span> {formatTokens(usage.cached_tokens)}</span>
    )}
    <span><span className="text-ink-mute">total</span> {formatTokens(usage.total_tokens)}</span>
    <span className="font-medium text-ink">
      {formatCost(usage.cost_usd)}{usage.cost_usd !== null && !usage.priced ? '*' : ''}
    </span>
  </div>
)}
```

(If the file holds the fetched object in a single state variable rather than destructured fields, read `data.usage`. Match the existing render structure — place the row under the run title / status pill.)

- [ ] **Step 2: Build + test**

Run: `npm run build` then `npm test -- --run`
Expected: strict build exits 0; tests green.

- [ ] **Step 3: Commit**

```bash
git add src/pages/RunDetail.tsx
git commit -m "feat(cp/web): token/cost breakdown on run detail

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Per-session usage chip

**Files:**
- Modify: `src/pages/SessionDetail.tsx`, `src/pages/Sessions.tsx`, `src/pages/ProjectSessions.tsx`

`SessionSummary` now carries an optional `usage` object (Task 3). Show a `UsageChip` where each session is rendered, guarding the optional with a fallback.

- [ ] **Step 1: Sessions list — import + render**

In `src/pages/Sessions.tsx`, add:

```ts
import UsageChip from '../components/UsageChip';
```

In the session row JSX (where `model`, `total_turns`, `relativeTime` render), add (guarded — `usage` is optional):

```tsx
            {session.usage && <UsageChip usage={session.usage} className="ml-2" />}
```

- [ ] **Step 2: SessionDetail — header chip**

In `src/pages/SessionDetail.tsx`, add the same import and render the chip in the header next to the model/turns metadata:

```tsx
            {session.usage && <UsageChip usage={session.usage} className="ml-2" />}
```

- [ ] **Step 3: ProjectSessions — import + render**

In `src/pages/ProjectSessions.tsx`, add the same import + guarded chip in the session row.

- [ ] **Step 4: Build + test**

Run: `npm run build` then `npm test -- --run`
Expected: strict build exits 0; tests green.

- [ ] **Step 5: Commit**

```bash
git add src/pages/SessionDetail.tsx src/pages/Sessions.tsx src/pages/ProjectSessions.tsx
git commit -m "feat(cp/web): per-session usage chip

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Project rollup stat

**Files:**
- Modify: `src/pages/ProjectDetail.tsx`

`getProject` now returns a top-level `usage` rollup. Show total tokens + total cost as a stat in the project overview header.

- [ ] **Step 1: Implement**

In `src/pages/ProjectDetail.tsx`, add:

```ts
import { formatTokens, formatCost } from '../lib/usage';
```

Where the project rollup stats render (the runs / sessions / coverage summary cluster), add a usage stat reading `data.usage` (match the file's variable name for the `ProjectDetail` response):

```tsx
{data.usage && (
  <div className="text-xs text-ink-dim tabular-nums">
    <span className="text-ink-mute">usage</span>{' '}
    {formatTokens(data.usage.total_tokens)} tok ·{' '}
    <span className="font-medium text-ink">
      {formatCost(data.usage.cost_usd)}{data.usage.cost_usd !== null && !data.usage.priced ? '*' : ''}
    </span>
  </div>
)}
```

- [ ] **Step 2: Build + test**

Run: `npm run build` then `npm test -- --run`
Expected: strict build exits 0; tests green.

- [ ] **Step 3: Commit**

```bash
git add src/pages/ProjectDetail.tsx
git commit -m "feat(cp/web): project usage rollup stat

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Dashboard usage panel

**Files:**
- Modify: `src/pages/Dashboard.tsx`

Add a usage panel: a "spend (30d)" stat tile + a top-models-by-cost horizontal bar (recharts — already imported in this file). Fetch `GET /api/usage` alongside the existing dashboard load. This fills the page's current "NO token/cost data available" gap (update that header comment).

- [ ] **Step 1: Add the fetch + state**

In `src/pages/Dashboard.tsx`:

1. Update the file header comment (lines 1-5) — remove the "NO … token/cost data" claim; note the usage panel sources `GET /api/usage`.
2. Add to the recharts import the bar primitives:

```ts
import {
  Bar,
  BarChart,
  Cell,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
```

3. Import usage helpers:

```ts
import { formatCost, formatTokens, type UsageOverview } from '../lib/usage';
```

4. Add state + load the overview in the existing `load` callback:

```ts
  const [usage, setUsage] = useState<UsageOverview | null>(null);
```

In `load`, after `const d = await api.getDashboard();`:

```ts
      const u = await api.getUsage({ groupBy: 'model' }).catch(() => null);
      setUsage(u);
```

(The `.catch(() => null)` keeps the dashboard resilient if the usage endpoint errs — the panel just hides.)

- [ ] **Step 2: Add the panel component**

Add a `UsagePanel` function in `Dashboard.tsx` (above the page component):

```tsx
// Top-models-by-cost bar + total-spend summary for the usage panel.
function UsagePanel({ overview }: { overview: UsageOverview }) {
  const { summary, breakdown } = overview;
  // Top 6 priced models by cost for the bar; ignore unpriced rows in the chart.
  const bars = breakdown
    .filter((r) => r.cost_usd !== null)
    .slice(0, 6)
    .map((r) => ({ name: r.model || r.provider || r.agent || '—', cost: r.cost_usd ?? 0 }));

  return (
    <div className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
      <div className="flex items-baseline gap-4 mb-3">
        <div>
          <p className="text-xs text-ink-dim font-medium uppercase tracking-wide">Spend (30d)</p>
          <p className="mt-1 text-2xl font-semibold text-ink tabular-nums">
            {formatCost(summary.cost_usd)}{summary.cost_usd !== null && !summary.priced ? '*' : ''}
          </p>
        </div>
        <div>
          <p className="text-xs text-ink-dim font-medium uppercase tracking-wide">Tokens</p>
          <p className="mt-1 text-2xl font-semibold text-ink tabular-nums">
            {formatTokens(summary.total_tokens)}
          </p>
        </div>
      </div>
      {bars.length === 0 ? (
        <p className="text-xs text-ink-mute py-6 text-center">No priced usage in the last 30 days</p>
      ) : (
        <div style={{ width: '100%', height: 28 * bars.length + 8 }}>
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={bars} layout="vertical" margin={{ left: 8, right: 16, top: 0, bottom: 0 }}>
              <XAxis type="number" hide />
              <YAxis type="category" dataKey="name" width={120} tick={{ fontSize: 11, fill: '#64748b' }} />
              <Tooltip
                contentStyle={tooltipStyle}
                formatter={(value) => [formatCost(typeof value === 'number' ? value : 0), 'cost']}
              />
              <Bar dataKey="cost" radius={[0, 4, 4, 0]}>
                {bars.map((b) => (
                  <Cell key={b.name} fill="#6366f1" />
                ))}
              </Bar>
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}
    </div>
  );
}
```

(`tooltipStyle` is already defined in this file and reused.)

- [ ] **Step 3: Render the panel + a spend stat tile**

In the page body, add a usage `section` after the "Run status distribution" section:

```tsx
          {/* ── Usage (tokens + cost) ── */}
          {usage && (
            <section>
              <h2 className="text-sm font-semibold text-ink-dim mb-3">Usage — last 30 days</h2>
              <UsagePanel overview={usage} />
            </section>
          )}
```

- [ ] **Step 4: Build + test + chunk check**

Run: `npm run build`
Expected: strict build exits 0. Confirm the main `index-*.js` chunk stays ~48 KB (recharts is already in the Dashboard route chunk, so adding the bar chart does NOT change the main entry). Paste the chunk-size line.

Run: `npm test -- --run`
Expected: tests green.

- [ ] **Step 5: Commit**

```bash
git add src/pages/Dashboard.tsx
git commit -m "feat(cp/web): dashboard usage panel (spend + top models)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Whole-slice build gate + visual handoff

**Files:** none (verification only)

- [ ] **Step 1: Strict build + full test suite**

Run: `npm run build && npm test -- --run`
Expected: build exits 0 (strict TS); all tests pass. Paste the test summary line + the main `index-*.js` chunk size (must be ~48 KB; recharts/markdown stay in their own lazy chunks — confirm with `grep -c recharts dist/assets/index-*.js` → 0).

- [ ] **Step 2: No `any` / static Tailwind audit**

Run: `grep -rn ": any\|as any\| bg-\${\| text-\${" src/lib/usage.ts src/components/UsageChip.tsx src/pages/Dashboard.tsx`
Expected: no matches.

- [ ] **Step 3: Visual validation checklist (matt runs the app)**

Hand off to matt with this checklist:
- Run rows on Workflow Runs + a project's Runs show `· N tok · $X`.
- Run detail shows the in/out/cached/total + cost breakdown.
- Sessions (list + detail) show a usage chip; an unpriced model shows `—` not `$0`.
- Project overview shows the rollup stat.
- Dashboard "Usage — last 30 days" panel shows spend + tokens + a top-models bar; empty state reads cleanly when there's no priced usage.
- A partial cost (mixed priced/unpriced) shows the `*` marker + hover title.

---

## Done criteria (whole plan)

- `npm run build` strict-clean; main entry chunk ~48 KB (recharts/markdown lazy).
- `npm test -- --run` green (usage formatters + UsageChip + api).
- No `any`; static Tailwind only.
- Cost is shown server-computed; unpriced → `—`; partial → `*` + title. Never a fabricated `$0`.
- All surfaces from the spec render usage: run rows + detail, session list + detail, project rollup, Dashboard panel. (Workflow detail's `usage` is exposed by 3a; surfacing it on `WorkflowDetail.tsx` is a trivial follow-up if matt wants it — noted, not silently dropped.)
