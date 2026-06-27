# rupu CP web — pages use the browser width (smart fluid)

Date: 2026-06-27
Status: approved (design)

## Problem

The app shell is fluid (`Layout.tsx` `<main className="flex-1 overflow-auto">`),
but every page caps its root content at `max-w-5xl` (1024px) — `<div className="p-8 max-w-5xl">` — with no centering. On a wide browser the content stays
~1024px, left-aligned, with the rest of the window empty. The UI doesn't grow
with the window.

## Decision (from brainstorming): smart fluid

- **Data-heavy pages → full width** (drop the cap): lists, dashboard, run
  tables, coverage, definition browsers benefit from width.
- **Reading-heavy pages → comfortable centered cap** (keep a max-width + center):
  chat / transcript / long prompt, so line length stays readable on wide
  monitors.

## Change (CSS-only — page-root classNames)

### Fluid (drop the max-width cap → `p-8`, full width)
- `Dashboard.tsx` — `p-8 max-w-6xl` → `p-8`.
- `Projects.tsx`, `Sessions.tsx`, `Workflows.tsx`, `Agents.tsx`, `Workers.tsx`,
  `Findings.tsx`, `Coverage.tsx`, `CoverageDetail.tsx`, `CoverageTemplates.tsx`,
  `AutoflowsDefs.tsx`, `WorkflowDetail.tsx`, `runs/AgentRuns.tsx`,
  `runs/WorkflowRuns.tsx`, `runs/AutoflowRuns.tsx` — `p-8 max-w-5xl` → `p-8`.
- `ProjectDetail.tsx` — all root states: `p-8 max-w-5xl` → `p-8`, and
  `p-8 max-w-5xl space-y-6` → `p-8 space-y-6`.
- `ProjectDefinitions.tsx` — `max-w-5xl` → remove the cap (its container becomes
  full-width; keep any other classes / `<div>` if none remain).

### Readable (keep cap + center: add `mx-auto`)
- `AgentDetail.tsx` — `p-8 max-w-5xl` → `p-8 max-w-5xl mx-auto`.
- `RunTranscript.tsx` — `flex flex-col p-8 max-w-5xl gap-4` → add `mx-auto`.
- `SessionDetail.tsx` — **no change**: already readable (`mx-auto max-w-3xl`
  chat column).

### Untouched
- `RunDetail.tsx` — its own graph + transcript layout (not a `max-w-5xl` page).
- Small fixed widths (`max-w-xs` search boxes, `max-w-2xl/3xl/md` inner blocks) —
  intentional per-element constraints, left as-is.

## Testing
CSS-only; no unit tests. Gate: `npx tsc --noEmit` clean + `npm run build`
succeeds. Manual: open a few pages wide (Dashboard, a list, a run table) →
content fills the window; open a transcript / agent definition → stays a
centered readable column.

## Out of scope
- Per-content max-widths inside fluid pages (e.g. constraining a single prose
  block within an otherwise-fluid page) — revisit only if a specific page reads
  poorly at full width.
