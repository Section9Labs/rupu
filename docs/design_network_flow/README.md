# Network / Netflow UI redesign — design notes

Status: approved direction (v3), 2026-08-31
Mockups (this project): `Netflow Current.dc.html` (baseline recreation), `Netflow A Explorer.dc.html`,
`Netflow B Timeline.dc.html`, `Netflow C Matrix.dc.html` (explorations), `Netflow v2.dc.html`
(A + B merged), **`Netflow v3 Scoped.dc.html` (approved)**.
Implementation guide: [PROMPT.md](PROMPT.md).

## Problem

The current Network surfaces (global `pages/Netflow.tsx`, project tab, run tab) render a static
bipartite SVG (`components/netflow/NetflowGraph.tsx` + `layout.ts`): one row per run on the left,
one row per `host:port` on the right, straight lines weighted by call count. Confirmed pains:

- Doesn't scale — one row per node; 100s of runs make the SVG thousands of px tall.
- No grouping — every `host:port` is flat; no ASN/org, origin, or workflow rollup.
- No time dimension anywhere.
- Not interactive — no hover, no drill-in, no cross-filtering.
- Disconnected from the flow table and host rollup rendered below it.
- Not insightful — it answers no question the tables don't already answer better.

## Approved design (v3)

**One surface at every scope.** Global, project, workflow and run views are the SAME component;
scope only narrows the dataset (matching how the three existing surfaces already share
`NetflowTable`/`NetflowSummary`/`ScopeDisclosure`). Scope comes from the route in the real CP
(the mockup's breadcrumb selects simulate it).

The surface, top to bottom:

1. **Header** — title, one-line subtitle, and a "Coverage & gaps" pill (amber) opening a popover
   containing the `ScopeDisclosure` text, the fidelity legend, and the `dropped_total` accounting.
   This is the "honesty on demand" decision: disclosure/fidelity/drop info moves out of the
   always-visible page body into one consistent, loud-enough affordance.
2. **Activity strip** — full-range histogram (calls stacked ok/error per bucket) with
   drag-to-zoom; presets (All / Last hour / 24h / 7d) beside it; the applied window is always
   echoed as text from the server's `window` echo, never from picker state (existing rule).
3. **KPI strip** — Flows, Endpoints, Networks (ASN orgs), Error rate, Bytes in (marked `†
   observed only` whenever coarse flows are in view), p95 latency. All computed over the
   currently filtered set.
4. **Visualization panel** with a Topology / Timeline segmented switcher:
   - **Topology**: three grouped columns — Workflows → Origins (`provider:*`/`scm:*`) → Networks
     (ASN orgs) — connected by ribbons (width = calls, red when error rate > 5%). Hover
     highlights connected paths; click toggles a filter. Scales with the number of
     workflows/origins/orgs (~constant), not runs.
   - **Timeline**: one swimlane per endpoint, grouped under ASN-org headers; heat cells per time
     bucket (shade = calls, red baseline = errors), an "Active runs" strip on top for
     correlation, per-lane stats (calls / errors / p95). Click a cell to zoom the shared window;
     click an endpoint to filter.
5. **Endpoint cards** (Topology view) — one card per ASN org listing its `host:port`s with
   calls / errors / p95; rows toggle endpoint filters.
6. **Filter chips** — every active filter (workflow, origin, network, endpoint, window) as a
   removable chip + Clear all.
7. **Flow table** — the existing per-flow table, cross-filtered by everything above; row click
   opens a right-hand detail panel with the full record (bytes, TTFB, peer IP, ASN, error) and
   the fidelity explanation. Fidelity badges move from a table column into the detail panel and
   the coverage popover.

### Scope behavior

- Narrower scope = same UI, less data. Topology columns list only workflows/origins/orgs present
  in scope; lanes only endpoints reached in scope.
- Entering run scope snaps the time window to the run's span and appends the run-scope
  sub-agent sentence to the disclosure (existing `RUN_SCOPE_SUB_AGENT_NOTE`).
- Project scope keeps the project-specific empty-state caveat (`netflowEmptyStateHint`).

### Honesty rules carried forward (non-negotiable)

- `null`/absent byte counts render as `—`, never `0 B` (`formatBytes`).
- Byte KPIs/rollups that include coarse flows are explicitly marked as observed-only sums.
- Ribbon/cell/edge weights scale by **calls, never bytes** (coarse bytes sum to 0 by
  construction — Fix 4 rationale in `NetflowGraph.tsx` applies to every new visual weight).
- `dropped_total` is whole-history, never window-scoped; wording per `DroppedBanner`.
- Empty states keep the applied-window distinction (`netflowWindowApplied`,
  `netflowRangeEmptyHint`) and the scope-limit sentence.
- Coverage copy stays single-sourced in `ScopeDisclosure.tsx`.

## Explorations kept for reference

- **C — Matrix** (`Netflow C Matrix.dc.html`): workflows/projects/runs × endpoints density
  heatmap. Rejected as the primary view (less narrative), but a strong candidate for a later
  third view on the same switcher — it stays one screen tall at any scale.
