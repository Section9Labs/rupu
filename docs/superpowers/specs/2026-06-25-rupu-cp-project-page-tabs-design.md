# rupu-cp — Tabbed Project detail page — Design

**Date:** 2026-06-25
**Surface:** `crates/rupu-cp/web` (frontend only — no backend change)
**Status:** awaiting matt's spec review

## Goal
Redesign the Project detail page (`/projects/:wsId`, `ProjectDetail.tsx`) from one long scroll into a **tabbed page** with a persistent header, so findings live in their own tab and each facet (runs / findings / sessions / coverage) gets focused navigation + filtering. Absorb the placeholder sub-pages (`ProjectRuns`/`ProjectSessions`/`ProjectCoverage`) into tab bodies.

## Approved shape (from discussion)
Persistent chrome (always visible, every tab): **identity header** + **5 rollup tiles** (Runs · Sessions · Coverage · Findings · Usage). Below it a **TabBar** (rupu's `TabBar`/`TabButton` pill style, matching `RunDetail`) with five tabs:

`Overview · Runs · Findings · Sessions · Coverage`

## Tabs are URL-addressable
Tab state lives in the URL (deep-linkable, back-button friendly, preserves existing "see all" links):
- `/projects/:wsId` → **Overview**
- `/projects/:wsId/runs` → **Runs**
- `/projects/:wsId/findings` → **Findings** (new route)
- `/projects/:wsId/sessions` → **Sessions**
- `/projects/:wsId/coverage` → **Coverage**

`ProjectDetail` becomes the shell for all five: it reads the active tab from the route, renders the persistent header + tiles + TabBar (TabButtons are `<Link>`s/`navigate` to the tab routes) + the active tab body. The existing standalone routes for `/runs`, `/sessions`, `/coverage` now render the tabbed `ProjectDetail` (active tab derived from the path) instead of the separate placeholder pages. **Definitions stays its own page** (`/projects/:wsId/definitions`, `ProjectDefinitions.tsx` — it already has its own internal sub-tabs), linked from the Overview tab (not a top-level tab here, to avoid nested tab bars).

`ProjectDetail` already blocks-renders on `getProject(wsId)`; the header + tiles use that. Each non-overview tab body lazy-fetches its own data when first opened (mirrors `RunDetail`'s ref-guarded lazy load), so switching tabs doesn't refetch everything up front.

## Tab bodies

### Overview (`/projects/:wsId`)
The "at a glance" landing: **Recent runs** (top ~5 from `detail.recent_runs`, each linking to `/runs/:id`, "see all →" to the Runs tab) + a **Coverage summary** card (targets · assessed % · findings, "open →" Coverage tab) + a **Sessions summary** card (total · active, "see all →" Sessions tab) + a **Definitions** link card (→ `/projects/:wsId/definitions`). This is essentially today's page minus the inline full Findings list (which moves to its own tab) and minus the rollup tiles (now persistent chrome).

### Runs (`/projects/:wsId/runs`)
Full paginated runs list via `getProjectRuns(wsId, { offset, limit })` (load-more, the existing `ProjectRuns` body). Adds **client-side filter chips**: **status** (all / running / completed / failed) on `RunListRow.status` and **trigger** (all / manual / event / cron) on `RunListRow.trigger`. Rows reuse `MetricRow` + `StatusPill` + `TriggerChip` + `UsageBarChart` as today.

### Findings (`/projects/:wsId/findings`)
`getFindings({ wsId })` → `FindingMetrics` strip in **interactive mode** (`onSelect` → client-side severity filter, the `active` tile highlighted; clicking Total/active clears) + the severity-ordered `FindingRow` list (each row carries its `project`/`targetId` provenance chip + the CWE chip already built). Loading / error / empty states.

### Sessions (`/projects/:wsId/sessions`)
Full paginated sessions list via `getProjectSessions(wsId, { offset, limit })` (the existing `ProjectSessions` body). Adds a **client-side scope filter** (all / active / archived) on the session's scope/status. Rows reuse `MetricRow` + the existing session-status helpers + active-run button.

### Coverage (`/projects/:wsId/coverage`)
Coverage targets list via `getProjectCoverage(wsId)` → `ProjectCoverageRow[]` (the existing `ProjectCoverage` body: target id · assertion lines · findings count, linking to `/coverage`). v1: no filter (small list); keep simple.

## Filtering summary
- **Findings** — severity (via `FindingMetrics onSelect`, already built; client-side).
- **Runs** — status + trigger chips (client-side over the loaded pages).
- **Sessions** — scope (all/active/archived) chips (client-side).
- **Coverage** — none in v1.
All filters are client-side over already-fetched data (backend only filters findings); chips are static-Tailwind toggle buttons consistent with the existing lifecycle-chip style.

## Components & files
**New (tab bodies — extracted/refactored from the placeholder pages):**
- `components/project/ProjectRunsTab.tsx` — runs list + status/trigger chips (from `ProjectRuns.tsx`).
- `components/project/ProjectFindingsTab.tsx` — findings metrics+list (from the current ProjectDetail findings section + interactive severity filter).
- `components/project/ProjectSessionsTab.tsx` — sessions list + scope chips (from `ProjectSessions.tsx`).
- `components/project/ProjectCoverageTab.tsx` — coverage list (from `ProjectCoverage.tsx`).
- `components/project/ProjectOverviewTab.tsx` — recent runs + coverage/sessions/definitions summary cards (from today's ProjectDetail body).

**Modified:**
- `pages/ProjectDetail.tsx` — becomes the tabbed shell (header + tiles + TabBar + active tab body from route). The local `RollupTile`/`SectionTitle`/`TriggerChip` helpers stay or move to shared as needed (lift `TriggerChip` to a shared module since the Runs tab also needs it).
- `App.tsx` — route `/projects/:wsId/:tab?` (or explicit routes for each tab) all render `ProjectDetail`; remove the standalone `ProjectRuns`/`ProjectSessions`/`ProjectCoverage` lazy routes (their bodies now live in the tab components). Keep `ProjectDefinitions` route.

**Retired:** `pages/ProjectRuns.tsx`, `pages/ProjectSessions.tsx`, `pages/ProjectCoverage.tsx` (logic moved into the tab components). `ProjectDefinitions.tsx` kept.

**Backend:** none.

## Data flow
```
getProject(wsId)  → header identity + 5 rollup tiles (persistent)
route :tab        → which tab body renders
  Overview  → detail.recent_runs + coverage/sessions/definitions summary cards
  Runs      → getProjectRuns(wsId,{offset,limit})  + client status/trigger chips
  Findings  → getFindings({ wsId })                + FindingMetrics severity filter
  Sessions  → getProjectSessions(wsId,{offset,limit}) + client scope chips
  Coverage  → getProjectCoverage(wsId)
```

## Error / empty / loading
Each tab body keeps its own loading/error/empty state (consistent with the page chrome): a small spinner while fetching, the standard error treatment on failure, and a short "No X yet" note when empty — never a blank tab. The persistent header + tiles render as soon as `getProject` resolves, independent of the active tab's fetch.

## Testing
- Frontend `npm test -- --run`: a `ProjectDetail` tab-routing test (route `/projects/x/findings` selects the Findings tab; default route selects Overview); a Runs filter test (status/trigger chips narrow the list); reuse existing `FindingMetrics` interactive test. Existing suite stays green.
- `npm run build` strict exit 0; recharts stays out of the main chunk (`grep -c recharts dist/assets/index-*.js` = 0 — `UsageBarChart` is already lazy-chunked); no `any`; static Tailwind only.
- Visual validation by matt: tabs switch + deep-link; findings tab severity filter works; runs/sessions filter chips work; header/tiles persist across tabs; existing "see all" links land on the right tab.

## Non-goals
- No backend changes; no new endpoints; no server-side run/session filtering (client-side over loaded pages is sufficient at current scale).
- Definitions keeps its own page (no nested tab bars).
- No coverage filtering in v1.
