# rupu CP — collapsible concern display for coverage tabs

Date: 2026-06-26
Status: approved (design)

## Problem

The coverage **Gap** tab renders every concern's full `gap_files` list
unconditionally (`CoverageGapTab.tsx` — a `<ul>` of all files, always expanded).
With many concerns each holding dozens-to-hundreds of file paths, the page
becomes a wall of thousands of monospace lines. The **Catalog** tab has a
related issue (every concern's full block renders at once). The fix is to
collapse detail and reveal it on demand, with filtering aids.

## Decisions (from brainstorming)

- **Interaction:** collapsed accordion rows — each concern is a one-line
  summary; click to expand its detail inline.
- **Scope:** all three concern-listing tabs (Gap, Catalog, Audit) via a shared
  collapsible component.
- **Controls:** severity filter, file text-filter, expand/collapse-all,
  per-concern "show all" cap.
- Audit rows become expandable to reveal asserted/gap file lists — a small
  feature add on top of today's collapsed summary.

## Shared building blocks (`crates/rupu-cp/web/src/components/coverage/`)

- **`CollapsibleRow.tsx`** — generic presentational row: a chevron + clickable
  `header` (always visible) + collapsible `children` (detail). Open state is
  *controlled* by the parent so expand/collapse-all works. Props:
  `{ open: boolean; onToggle: () => void; header: ReactNode; children: ReactNode }`.
- **`SeverityChip.tsx`** — the severity pill currently duplicated inline in each
  tab, extracted once. Props: `{ severity: string }`.
- **`CappedList.tsx`** — renders a `string[]` showing the first N (default 10)
  then a `… show all {total}` toggle. Props:
  `{ items: string[]; cap?: number }`.
- **`ConcernControls.tsx`** — the filter bar: severity filter (chips/select) +
  optional file text-filter + expand-all/collapse-all buttons. The file-filter
  slot is omitted where there are no files (Catalog). Props:
  `{ severity, onSeverity, fileQuery?, onFileQuery?, onExpandAll, onCollapseAll, total }`.
- **Pure helpers** in `crates/rupu-cp/web/src/lib/coverageFilter.ts` (tested):
  - `filterConcerns<T extends { severity: string }>(rows, opts)` — filter by
    severity; generic over row shape.
  - For the Gap tab specifically, a `filterGapRows(rows, { severity, fileQuery })`
    that also narrows each row's `gap_files` to substring matches and drops rows
    with zero matching files when a query is active.

## Per-tab application

### Gap (`CoverageGapTab.tsx`)
- Controls: severity + file filter + expand/collapse-all.
- Each concern → `CollapsibleRow`. Header: `SeverityChip` · name · id ·
  `{matching}/{total} files`. Body: `CappedList(matchingGapFiles)`.
- A file query narrows each concern's files to substring matches and hides
  concerns with none.

### Catalog (`CoverageCatalogTab.tsx`)
- Controls: severity + expand/collapse-all (no file filter — Catalog has no
  per-concern files).
- Each concern → `CollapsibleRow`. Header: `SeverityChip` · name · id · source
  (`cat.sources[id] ?? 'inline'`). Body: description, applicable globs,
  min-strength, tags, references.

### Audit (`CoverageAuditTab.tsx`)
- Keep the totals strip, cross-model, and serendipitous sections unchanged.
- Controls for the per-concern matrix: severity + expand/collapse-all.
- Each `ConcernCoverage` → `CollapsibleRow`. Collapsed header = today's summary
  (progress bar + `clean/finding/examined/n-a` counts + gap badge). Expanded
  body: **asserted files** and **gap files**, each a `CappedList`.

## Open-state ownership

Each tab owns a `Set<string>` of open concern ids (keyed by `concern_id`).
`CollapsibleRow.onToggle` flips membership. Expand-all sets the set to all
*currently visible* (post-filter) ids; collapse-all clears it.

## Out of scope

- Server-side pagination of files (payload already contains all files; this is a
  rendering change only). Revisit only if payloads become a perf problem.
- Diff tab (no per-concern file dump; unchanged).
- Master-detail / drawer layouts (accordion chosen).

## Testing

- vitest:
  - `filterConcerns` — severity filter keeps/drops by severity.
  - `filterGapRows` — file-substring narrows files and hides no-match concerns;
    severity + query combined.
  - `CappedList` slice behavior (cap respected; show-all reveals the rest).
  - `CollapsibleRow` toggle smoke test (body shown only when `open`).
- Existing `coverageGap.gapRows` test stays green.
