# Coverage Collapsible Concerns — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the wall-of-text coverage tabs with collapsed accordion concern rows (Gap, Catalog, Audit) plus severity/file filters, expand/collapse-all, and a per-concern "show all" cap, via shared components.

**Architecture:** A small set of shared presentational components (`CollapsibleRow`, `SeverityChip`, `CappedList`, `ConcernControls`) plus pure filter helpers (`coverageFilter.ts`). Each of the three tabs owns a `Set<concernId>` of open rows and composes the shared pieces with a tab-specific header + body. Frontend-only; the audit/gap payload already contains all files.

**Tech Stack:** React 18 + TypeScript + Vite + Vitest + Tailwind.

Spec: `docs/superpowers/specs/2026-06-26-rupu-cp-coverage-collapsible-concerns-design.md`

## Global Constraints

- All files under `crates/rupu-cp/web/`.
- Pure-logic tests run in node env; component tests use `// @vitest-environment jsdom`.
- Reuse existing `SectionHeader`/`ListCard` and `normFindingSeverity`/`sevRank`
  from `../lib/api`. `SectionHeader` tones: `good|progress|warn|bad|critical|low|muted`.
- Verify each task with `npx tsc --noEmit` before commit; `npm run build` at the end.
- Existing severity-pill markup (duplicated in the tabs) is:
  `inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200`.

---

## Task 1: Shared presentational components (`SeverityChip`, `CappedList`, `CollapsibleRow`)

**Files:**
- Create: `crates/rupu-cp/web/src/components/coverage/SeverityChip.tsx`
- Create: `crates/rupu-cp/web/src/components/coverage/CappedList.tsx`
- Create: `crates/rupu-cp/web/src/components/coverage/CollapsibleRow.tsx`
- Create: `crates/rupu-cp/web/src/components/coverage/CappedList.test.tsx`
- Create: `crates/rupu-cp/web/src/components/coverage/CollapsibleRow.test.tsx`

**Interfaces:**
- Produces:
  - `SeverityChip({ severity }: { severity: string })`
  - `CappedList({ items, cap }: { items: string[]; cap?: number })` (default cap 10)
  - `CollapsibleRow({ open, onToggle, header, children }: { open: boolean; onToggle: () => void; header: React.ReactNode; children: React.ReactNode })`

- [ ] **Step 1: Write the failing tests**

`CappedList.test.tsx`:

```tsx
// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import CappedList from './CappedList';

describe('CappedList', () => {
  it('shows only the first `cap` items until expanded', () => {
    const items = Array.from({ length: 5 }, (_, i) => `file-${i}.rs`);
    render(<CappedList items={items} cap={2} />);
    expect(screen.getByText('file-0.rs')).toBeInTheDocument();
    expect(screen.getByText('file-1.rs')).toBeInTheDocument();
    expect(screen.queryByText('file-2.rs')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText(/show all 5/i));
    expect(screen.getByText('file-2.rs')).toBeInTheDocument();
    expect(screen.getByText('file-4.rs')).toBeInTheDocument();
  });

  it('shows no toggle when items fit under the cap', () => {
    render(<CappedList items={['only.rs']} cap={10} />);
    expect(screen.getByText('only.rs')).toBeInTheDocument();
    expect(screen.queryByText(/show all/i)).not.toBeInTheDocument();
  });
});
```

`CollapsibleRow.test.tsx`:

```tsx
// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import CollapsibleRow from './CollapsibleRow';

describe('CollapsibleRow', () => {
  it('renders children only when open', () => {
    const { rerender } = render(
      <CollapsibleRow open={false} onToggle={() => {}} header={<span>Head</span>}>
        <span>Body</span>
      </CollapsibleRow>,
    );
    expect(screen.getByText('Head')).toBeInTheDocument();
    expect(screen.queryByText('Body')).not.toBeInTheDocument();

    rerender(
      <CollapsibleRow open={true} onToggle={() => {}} header={<span>Head</span>}>
        <span>Body</span>
      </CollapsibleRow>,
    );
    expect(screen.getByText('Body')).toBeInTheDocument();
  });

  it('calls onToggle when the header is clicked', () => {
    const onToggle = vi.fn();
    render(
      <CollapsibleRow open={false} onToggle={onToggle} header={<span>Head</span>}>
        <span>Body</span>
      </CollapsibleRow>,
    );
    fireEvent.click(screen.getByText('Head'));
    expect(onToggle).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/coverage/CappedList.test.tsx src/components/coverage/CollapsibleRow.test.tsx`
Expected: FAIL — modules not found.

- [ ] **Step 3: Implement the components**

`SeverityChip.tsx`:

```tsx
// Shared severity pill, used across the coverage concern tabs.
export default function SeverityChip({ severity }: { severity: string }) {
  return (
    <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
      {severity}
    </span>
  );
}
```

`CappedList.tsx`:

```tsx
// Renders a list of strings (file paths), showing the first `cap` then a
// "show all N" toggle so one huge concern can't flood the view.
import { useState } from 'react';

export default function CappedList({ items, cap = 10 }: { items: string[]; cap?: number }) {
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? items : items.slice(0, cap);
  return (
    <div>
      <ul className="space-y-0.5">
        {shown.map((f) => (
          <li key={f} className="text-[11px] font-mono text-ink-mute break-all">
            {f}
          </li>
        ))}
      </ul>
      {items.length > cap && (
        <button
          onClick={() => setExpanded((v) => !v)}
          className="mt-1 text-[11px] font-medium text-brand-700 hover:text-brand-500"
        >
          {expanded ? 'show less' : `show all ${items.length}`}
        </button>
      )}
    </div>
  );
}
```

`CollapsibleRow.tsx`:

```tsx
// Generic accordion row: a clickable header (always visible) + collapsible
// body. Open state is controlled by the parent so expand/collapse-all works.
import { ChevronRight } from 'lucide-react';
import { cn } from '../../lib/cn';

export default function CollapsibleRow({
  open,
  onToggle,
  header,
  children,
}: {
  open: boolean;
  onToggle: () => void;
  header: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="px-4 py-3">
      <button onClick={onToggle} className="flex w-full items-start gap-2 text-left">
        <ChevronRight
          size={14}
          className={cn('mt-0.5 shrink-0 text-ink-mute transition-transform', open && 'rotate-90')}
        />
        <span className="min-w-0 flex-1">{header}</span>
      </button>
      {open && <div className="mt-2 pl-6">{children}</div>}
    </div>
  );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/coverage/CappedList.test.tsx src/components/coverage/CollapsibleRow.test.tsx`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/coverage/SeverityChip.tsx crates/rupu-cp/web/src/components/coverage/CappedList.tsx crates/rupu-cp/web/src/components/coverage/CollapsibleRow.tsx crates/rupu-cp/web/src/components/coverage/CappedList.test.tsx crates/rupu-cp/web/src/components/coverage/CollapsibleRow.test.tsx
git commit -m "feat(cp/web): shared collapsible/severity/capped-list coverage components"
```

---

## Task 2: Pure filter helpers (`coverageFilter.ts`)

**Files:**
- Create: `crates/rupu-cp/web/src/lib/coverageFilter.ts`
- Create: `crates/rupu-cp/web/src/lib/coverageFilter.test.ts`

**Interfaces:**
- Consumes: `GapRow` from `./coverageGap`.
- Produces:
  - `filterConcerns<T extends { severity: string }>(rows: T[], severity: string): T[]`
    — `severity === 'all'` keeps everything; otherwise keep rows whose severity
    matches (case-insensitive).
  - `filterGapRows(rows: GapRow[], opts: { severity: string; fileQuery: string }): GapRow[]`
    — apply severity filter; when `fileQuery` is non-empty, narrow each row's
    `gap_files` to case-insensitive substring matches and drop rows with none.

- [ ] **Step 1: Write the failing test**

`coverageFilter.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { filterConcerns, filterGapRows } from './coverageFilter';
import type { GapRow } from './coverageGap';

const rows: GapRow[] = [
  { concern_id: 'a', name: 'A', severity: 'high', gap_files: ['src/api/x.rs', 'src/db/y.rs'] },
  { concern_id: 'b', name: 'B', severity: 'low', gap_files: ['src/api/z.rs'] },
  { concern_id: 'c', name: 'C', severity: 'high', gap_files: ['lib/util.rs'] },
];

describe('filterConcerns', () => {
  it('keeps all when severity is "all"', () => {
    expect(filterConcerns(rows, 'all')).toHaveLength(3);
  });
  it('filters by severity case-insensitively', () => {
    expect(filterConcerns(rows, 'high').map((r) => r.concern_id)).toEqual(['a', 'c']);
  });
});

describe('filterGapRows', () => {
  it('narrows files to substring matches and drops empty rows', () => {
    const out = filterGapRows(rows, { severity: 'all', fileQuery: 'api' });
    expect(out.map((r) => r.concern_id)).toEqual(['a', 'b']);
    expect(out[0].gap_files).toEqual(['src/api/x.rs']);
  });
  it('combines severity + file query', () => {
    const out = filterGapRows(rows, { severity: 'high', fileQuery: 'api' });
    expect(out.map((r) => r.concern_id)).toEqual(['a']);
  });
  it('no query keeps all files', () => {
    const out = filterGapRows(rows, { severity: 'all', fileQuery: '' });
    expect(out).toHaveLength(3);
    expect(out[0].gap_files).toHaveLength(2);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/coverageFilter.test.ts`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement**

`coverageFilter.ts`:

```ts
import type { GapRow } from './coverageGap';

/** Keep rows matching `severity` ('all' keeps everything), case-insensitive. */
export function filterConcerns<T extends { severity: string }>(rows: T[], severity: string): T[] {
  if (severity === 'all') return rows;
  const want = severity.toLowerCase();
  return rows.filter((r) => r.severity.toLowerCase() === want);
}

/**
 * Apply the severity filter, then (when `fileQuery` is non-empty) narrow each
 * row's `gap_files` to case-insensitive substring matches, dropping rows that
 * end up with no matching files.
 */
export function filterGapRows(
  rows: GapRow[],
  opts: { severity: string; fileQuery: string },
): GapRow[] {
  const bySeverity = filterConcerns(rows, opts.severity);
  const q = opts.fileQuery.trim().toLowerCase();
  if (!q) return bySeverity;
  return bySeverity
    .map((r) => ({ ...r, gap_files: r.gap_files.filter((f) => f.toLowerCase().includes(q)) }))
    .filter((r) => r.gap_files.length > 0);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/coverageFilter.test.ts`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/coverageFilter.ts crates/rupu-cp/web/src/lib/coverageFilter.test.ts
git commit -m "feat(cp/web): pure severity/file filter helpers for coverage concerns"
```

---

## Task 3: `ConcernControls` filter bar

**Files:**
- Create: `crates/rupu-cp/web/src/components/coverage/ConcernControls.tsx`

**Interfaces:**
- Produces: `ConcernControls(props)` where
  `props = { severity: string; onSeverity: (s: string) => void; fileQuery?: string; onFileQuery?: (s: string) => void; onExpandAll: () => void; onCollapseAll: () => void; total: number }`.
  The file-filter input renders only when `onFileQuery` is provided.

- [ ] **Step 1: Implement (presentational; covered by tab smoke + manual)**

`ConcernControls.tsx`:

```tsx
// Filter/control bar shared by the coverage concern tabs: severity dropdown,
// optional file text-filter, and expand/collapse-all.
const SEVERITIES = ['all', 'critical', 'high', 'medium', 'low', 'info'];

export default function ConcernControls({
  severity,
  onSeverity,
  fileQuery,
  onFileQuery,
  onExpandAll,
  onCollapseAll,
  total,
}: {
  severity: string;
  onSeverity: (s: string) => void;
  fileQuery?: string;
  onFileQuery?: (s: string) => void;
  onExpandAll: () => void;
  onCollapseAll: () => void;
  total: number;
}) {
  return (
    <div className="mb-3 flex flex-wrap items-center gap-2">
      <span className="text-[11px] text-ink-mute tabular-nums">{total} concerns</span>
      <select
        value={severity}
        onChange={(e) => onSeverity(e.target.value)}
        className="rounded-md border border-border bg-panel px-2 py-1 text-xs text-ink"
      >
        {SEVERITIES.map((s) => (
          <option key={s} value={s}>
            {s === 'all' ? 'all severities' : s}
          </option>
        ))}
      </select>
      {onFileQuery && (
        <input
          value={fileQuery ?? ''}
          onChange={(e) => onFileQuery(e.target.value)}
          placeholder="filter files…"
          className="rounded-md border border-border bg-panel px-2 py-1 text-xs text-ink"
        />
      )}
      <div className="ml-auto flex gap-1">
        <button
          onClick={onExpandAll}
          className="rounded-md border border-border px-2 py-1 text-xs text-ink-dim hover:bg-slate-100"
        >
          expand all
        </button>
        <button
          onClick={onCollapseAll}
          className="rounded-md border border-border px-2 py-1 text-xs text-ink-dim hover:bg-slate-100"
        >
          collapse all
        </button>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Typecheck**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cp/web/src/components/coverage/ConcernControls.tsx
git commit -m "feat(cp/web): ConcernControls filter bar"
```

---

## Task 4: Gap tab — collapsible + filters

**Files:**
- Modify: `crates/rupu-cp/web/src/components/coverage/CoverageGapTab.tsx`

**Interfaces:**
- Consumes: `gapRows` (`../../lib/coverageGap`), `filterGapRows` (`../../lib/coverageFilter`),
  `CollapsibleRow`, `SeverityChip`, `CappedList`, `ConcernControls`.

- [ ] **Step 1: Rewrite the tab body**

Replace the entire contents of `CoverageGapTab.tsx` with:

```tsx
// Gap tab — concerns whose in-scope files weren't all assessed. Collapsed
// accordion rows with severity + file filters and expand/collapse-all.
import { useEffect, useMemo, useState } from 'react';
import { api, type AuditReport } from '../../lib/api';
import { gapRows } from '../../lib/coverageGap';
import { filterGapRows } from '../../lib/coverageFilter';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';
import CollapsibleRow from './CollapsibleRow';
import SeverityChip from './SeverityChip';
import CappedList from './CappedList';
import ConcernControls from './ConcernControls';

export default function CoverageGapTab({ target, wsId }: { target: string; wsId?: string }) {
  const [report, setReport] = useState<AuditReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [severity, setSeverity] = useState('all');
  const [fileQuery, setFileQuery] = useState('');
  const [open, setOpen] = useState<Set<string>>(new Set());

  useEffect(() => {
    let cancelled = false;
    setReport(null);
    setError(null);
    api
      .getCoverageAudit(target, wsId)
      .then((d) => {
        if (!cancelled) setReport(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load gaps');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  const rows = useMemo(
    () => (report ? filterGapRows(gapRows(report), { severity, fileQuery }) : []),
    [report, severity, fileQuery],
  );

  function toggle(id: string) {
    setOpen((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  }

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!report) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;
  if (gapRows(report).length === 0)
    return <p className="mt-4 text-sm text-ink-dim">No gaps — every in-scope file assessed.</p>;

  return (
    <section className="mt-6">
      <SectionHeader tone="bad" label="Coverage gaps" count={rows.length} hint="concerns with unassessed files" />
      <ConcernControls
        severity={severity}
        onSeverity={setSeverity}
        fileQuery={fileQuery}
        onFileQuery={setFileQuery}
        onExpandAll={() => setOpen(new Set(rows.map((r) => r.concern_id)))}
        onCollapseAll={() => setOpen(new Set())}
        total={rows.length}
      />
      {rows.length === 0 ? (
        <p className="text-sm text-ink-dim pl-1">No concerns match the current filters.</p>
      ) : (
        <ListCard>
          {rows.map((r) => (
            <CollapsibleRow
              key={r.concern_id}
              open={open.has(r.concern_id)}
              onToggle={() => toggle(r.concern_id)}
              header={
                <span className="flex items-center gap-2 flex-wrap">
                  <span className="text-sm font-medium text-ink">{r.name}</span>
                  <span className="text-[11px] font-mono text-ink-mute">{r.concern_id}</span>
                  <SeverityChip severity={r.severity} />
                  <span className="text-[10px] text-amber-700 font-medium tabular-nums">
                    {r.gap_files.length} files
                  </span>
                </span>
              }
            >
              <CappedList items={r.gap_files} />
            </CollapsibleRow>
          ))}
        </ListCard>
      )}
    </section>
  );
}
```

- [ ] **Step 2: Typecheck + run gap-related tests**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run src/lib/coverageGap.test.ts src/lib/coverageFilter.test.ts`
Expected: clean + all pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cp/web/src/components/coverage/CoverageGapTab.tsx
git commit -m "feat(cp/web): collapsible Gap tab with severity/file filters"
```

---

## Task 5: Catalog tab — collapsible

**Files:**
- Modify: `crates/rupu-cp/web/src/components/coverage/CoverageCatalogTab.tsx`

**Interfaces:**
- Consumes: `filterConcerns` (`../../lib/coverageFilter`), `CollapsibleRow`,
  `SeverityChip`, `ConcernControls`, `api.getCoverageCatalog`, `FlatCatalog`.

- [ ] **Step 1: Rewrite the tab body**

Replace the entire contents of `CoverageCatalogTab.tsx` with:

```tsx
// Catalog tab — the effective concern catalog snapshot, as collapsed rows.
import { useEffect, useMemo, useState } from 'react';
import { api, type FlatCatalog } from '../../lib/api';
import { filterConcerns } from '../../lib/coverageFilter';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';
import CollapsibleRow from './CollapsibleRow';
import SeverityChip from './SeverityChip';
import ConcernControls from './ConcernControls';

export default function CoverageCatalogTab({ target, wsId }: { target: string; wsId?: string }) {
  const [cat, setCat] = useState<FlatCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [severity, setSeverity] = useState('all');
  const [open, setOpen] = useState<Set<string>>(new Set());

  useEffect(() => {
    let cancelled = false;
    setCat(null);
    setError(null);
    api
      .getCoverageCatalog(target, wsId)
      .then((d) => {
        if (!cancelled) setCat(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load catalog');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  const concerns = useMemo(
    () => (cat ? filterConcerns(cat.concerns, severity) : []),
    [cat, severity],
  );

  function toggle(id: string) {
    setOpen((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  }

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!cat) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;
  if (cat.concerns.length === 0)
    return <p className="mt-4 text-sm text-ink-dim">No catalog snapshot for this target.</p>;

  return (
    <section className="mt-6">
      <SectionHeader tone="muted" label="Catalog concerns" count={concerns.length} />
      <ConcernControls
        severity={severity}
        onSeverity={setSeverity}
        onExpandAll={() => setOpen(new Set(concerns.map((c) => c.id)))}
        onCollapseAll={() => setOpen(new Set())}
        total={concerns.length}
      />
      <ListCard>
        {concerns.map((c) => (
          <CollapsibleRow
            key={c.id}
            open={open.has(c.id)}
            onToggle={() => toggle(c.id)}
            header={
              <span className="flex items-center gap-2 flex-wrap">
                <span className="text-sm font-medium text-ink">{c.name}</span>
                <span className="text-[11px] font-mono text-ink-mute">{c.id}</span>
                <SeverityChip severity={c.severity} />
                <span className="text-[10px] text-ink-mute">{cat.sources[c.id] ?? 'inline'}</span>
              </span>
            }
          >
            {c.description && (
              <p className="text-xs text-ink-dim leading-snug">{c.description}</p>
            )}
            <p className="mt-1 text-[11px] text-ink-mute font-mono break-all">
              globs: {c.applicable_globs.join(', ')}
            </p>
            <p className="mt-1 text-[11px] text-ink-mute">min strength: {c.min_strength}</p>
            {c.tags.length > 0 && (
              <p className="mt-1 text-[11px] text-ink-mute">tags: {c.tags.join(', ')}</p>
            )}
            {c.references.length > 0 && (
              <ul className="mt-1 space-y-0.5">
                {c.references.map((ref) => (
                  <li key={ref} className="text-[11px] text-ink-mute break-all">{ref}</li>
                ))}
              </ul>
            )}
          </CollapsibleRow>
        ))}
      </ListCard>
    </section>
  );
}
```

- [ ] **Step 2: Typecheck**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cp/web/src/components/coverage/CoverageCatalogTab.tsx
git commit -m "feat(cp/web): collapsible Catalog tab"
```

---

## Task 6: Audit tab — collapsible per-concern with file detail

**Files:**
- Modify: `crates/rupu-cp/web/src/components/coverage/CoverageAuditTab.tsx`

**Interfaces:**
- Consumes: `filterConcerns`, `CollapsibleRow`, `SeverityChip`, `CappedList`,
  `ConcernControls`, plus existing `sevRank`/`normFindingSeverity`.

- [ ] **Step 1: Update imports + per-concern section**

In `CoverageAuditTab.tsx`, replace the imports block and the per-concern section
so the matrix uses collapsible rows. Keep the totals strip, cross-model, and
serendipitous sections unchanged.

Replace the import block at the top with:

```tsx
import { useEffect, useMemo, useState } from 'react';
import {
  api,
  normFindingSeverity,
  sevRank,
  type AuditReport,
  type ConcernCoverage,
} from '../../lib/api';
import { filterConcerns } from '../../lib/coverageFilter';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';
import CollapsibleRow from './CollapsibleRow';
import SeverityChip from './SeverityChip';
import CappedList from './CappedList';
import ConcernControls from './ConcernControls';
```

Add filter + open state inside the component (next to the existing `report`
state):

```tsx
  const [severity, setSeverity] = useState('all');
  const [open, setOpen] = useState<Set<string>>(new Set());
```

Change the `concerns` memo to also apply the severity filter (keep the existing
critical→info sort):

```tsx
  const concerns = useMemo(
    () =>
      filterConcerns(
        [...(report?.concerns ?? [])].sort(
          (a, b) =>
            sevRank(normFindingSeverity(a.severity)) - sevRank(normFindingSeverity(b.severity)),
        ),
        severity,
      ),
    [report, severity],
  );
```

Add the toggle helper (inside the component):

```tsx
  function toggle(id: string) {
    setOpen((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  }
```

Replace the "Per-concern coverage" `<section>` (the one that maps
`concerns.map((c) => <ConcernRow .../>)` inside a `<ListCard>`) with:

```tsx
      <section>
        <SectionHeader tone="progress" label="Per-concern coverage" count={concerns.length} />
        {concerns.length === 0 ? (
          <p className="text-sm text-ink-dim pl-1 mt-1">No catalog → no audit matrix.</p>
        ) : (
          <>
            <ConcernControls
              severity={severity}
              onSeverity={setSeverity}
              onExpandAll={() => setOpen(new Set(concerns.map((c) => c.concern_id)))}
              onCollapseAll={() => setOpen(new Set())}
              total={concerns.length}
            />
            <ListCard>
              {concerns.map((c) => (
                <ConcernRow
                  key={c.concern_id}
                  c={c}
                  open={open.has(c.concern_id)}
                  onToggle={() => toggle(c.concern_id)}
                />
              ))}
            </ListCard>
          </>
        )}
      </section>
```

- [ ] **Step 2: Rewrite the `ConcernRow` helper**

Replace the existing `ConcernRow` function with a collapsible version whose
collapsed header is the existing summary and whose body reveals asserted/gap
files:

```tsx
function ConcernRow({
  c,
  open,
  onToggle,
}: {
  c: ConcernCoverage;
  open: boolean;
  onToggle: () => void;
}) {
  const assessed = c.asserted_files.length;
  const inScope = c.in_scope_files.length;
  const pct = inScope === 0 ? 0 : Math.round((assessed / inScope) * 100);
  return (
    <CollapsibleRow
      open={open}
      onToggle={onToggle}
      header={
        <span className="block">
          <span className="flex items-center gap-2 flex-wrap">
            <span className="text-sm font-medium text-ink">{c.name}</span>
            <span className="text-[11px] font-mono text-ink-mute">{c.concern_id}</span>
            <SeverityChip severity={c.severity} />
            {c.gap_files.length > 0 && (
              <span className="text-[10px] text-amber-700 font-medium">
                {c.gap_files.length} gap
              </span>
            )}
          </span>
          <span className="mt-1.5 flex items-center gap-2">
            <span className="h-1.5 flex-1 rounded bg-slate-100 overflow-hidden">
              <span className="block h-full bg-brand-500" style={{ width: `${pct}%` }} />
            </span>
            <span className="text-[11px] text-ink-mute tabular-nums w-24 text-right">
              {assessed}/{inScope} files
            </span>
          </span>
          <span className="mt-1 block text-[11px] text-ink-mute tabular-nums">
            clean {c.clean} · finding {c.findings} · examined {c.examined} · n/a{' '}
            {c.not_applicable}
          </span>
        </span>
      }
    >
      {c.asserted_files.length > 0 && (
        <div className="mb-2">
          <p className="text-[11px] font-medium text-ink-dim mb-0.5">Asserted</p>
          <CappedList items={c.asserted_files} />
        </div>
      )}
      {c.gap_files.length > 0 && (
        <div>
          <p className="text-[11px] font-medium text-amber-700 mb-0.5">Gap</p>
          <CappedList items={c.gap_files} />
        </div>
      )}
      {c.asserted_files.length === 0 && c.gap_files.length === 0 && (
        <p className="text-[11px] text-ink-mute">No in-scope files.</p>
      )}
    </CollapsibleRow>
  );
}
```

- [ ] **Step 3: Typecheck**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/web/src/components/coverage/CoverageAuditTab.tsx
git commit -m "feat(cp/web): collapsible Audit per-concern rows with file detail"
```

---

## Task 7: Full verification + PR

- [ ] **Step 1: Frontend checks**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit` → clean.
Run: `cd crates/rupu-cp/web && npx vitest run` → all pass (incl. CappedList,
CollapsibleRow, coverageFilter, coverageGap).
Run: `cd crates/rupu-cp/web && npm run build` → success.

- [ ] **Step 2: Manual smoke (recommended)**

`rupu cp serve`; open a coverage target with gaps → Gap tab (rows collapsed;
expand one, filter by severity, type a file substring, expand/collapse-all);
Catalog tab (collapsed concerns); Audit tab (expand a concern → asserted/gap
files).

- [ ] **Step 3: Open PR**

```bash
gh pr create --title "feat(cp): collapsible concern display for coverage tabs" --body "…"
```

---

## Self-review notes (author)

- Spec coverage: shared components (Task 1), filter helpers (Task 2), controls
  (Task 3), Gap (Task 4), Catalog (Task 5), Audit incl. asserted/gap file detail
  (Task 6), verify+PR (Task 7).
- Type consistency: `CollapsibleRow`/`CappedList`/`SeverityChip` signatures used
  identically across Tasks 4–6; `filterConcerns`/`filterGapRows` signatures match
  Task 2 definitions; `GapRow` reused from `coverageGap`.
- Open-state: each tab owns `Set<string>`; expand-all sets to *post-filter*
  visible ids, collapse-all clears — consistent across the three tabs.
- The existing `coverageGap.gapRows` test is untouched and stays green.
```
