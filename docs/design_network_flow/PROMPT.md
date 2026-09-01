# PROMPT — implement the new Network (netflow) views in rupu-cp

You are implementing the approved Network UI redesign in `crates/rupu-cp/web`. The interactive
reference is `Netflow v3 Scoped.dc.html` in the design project (plus README.md next to this
file). Match it visually and behaviorally using the CP's real primitives and tokens — do not
invent new colors, spacing, or copy.

## Ground rules

- One surface, every scope. Build a single `NetflowExplorer` component rendered by all three
  existing routes/surfaces: `pages/Netflow.tsx` (global), the project Network tab, and the run
  Network tab (`pages/RunDetail.tsx`). Scope comes from the route — there is NO scope breadcrumb
  in the real CP (that was mockup-only simulation). Props: `scope: NetflowScope` plus the ids
  the fetchers need.
- Reuse, don't fork: `SortableTable`, `EmptyState`, `Badge`, `Segmented`, `Input`, `Button`,
  `FidelityBadge`, `TimeRangePicker`, `NetflowWindowReadout`, `NetflowScopeDisclosure`, and all
  exports of `components/netflow/ScopeDisclosure.tsx`. Colors via Tailwind tokens or
  `useThemeColors()` for inline SVG (same bridge `NetflowGraph` uses today). Type scale:
  `text-meta/note/ui/lead`. Both themes must work — no hard-coded hexes.
- Every honesty rule in `NetflowTable.tsx` / `ScopeDisclosure.tsx` / `lib/netflow.ts` header
  comments still holds. Weight visuals by `calls`, never `bytes`. `formatBytes(null)` → `—`.
  `dropped_total` is whole-history. The window echo (`NetflowResponse.window`) — not picker
  state — decides empty-state wording and the readout.
- Tests accompany every component, same style as the existing `*.test.tsx` files.

## Server work (rupu-cp + rupu-netflow)

The new views need aggregates the API doesn't ship yet. Per repo convention (see
`HostRollup`'s "computed server-side" rule), add them to the read side once, in Rust —
no client-side re-derivation:

1. `rupu_netflow::ledger::views`: add
   - `SankeyView { workflows: Vec<NodeAgg>, origins: Vec<NodeAgg>, orgs: Vec<NodeAgg>,
     wf_origin: Vec<LinkAgg>, origin_org: Vec<LinkAgg> }` where `NodeAgg { id, label, calls,
     errors }` and `LinkAgg { from, to, calls, errors }`. Orgs come from read-time ASN
     enrichment; flows without an ASN entry group under an explicit `unknown` org (label
     "Unknown network") — never silently dropped.
   - `TimelineView { bucket_ms, from, to, lanes: Vec<LaneAgg>, runs: Vec<RunActivity> }` where
     `LaneAgg { host, port, org, asn, fidelity, calls, errors, p50_ms, p95_ms,
     buckets: Vec<BucketAgg> }`, `BucketAgg { calls, errors }` (dense, fixed count),
     `RunActivity { bucket_index -> active run count }` (dense vec).
   - `HistogramView`: dense ok/error buckets over the full retained range for the activity
     strip (fixed 84 buckets, server-chosen bounds echoed back).
2. `rupu-cp/src/api/netflow.rs`: new route `GET /api/netflow/explorer` accepting the existing
   `scope` (`run:<id>` / `project:<id>`, absent = global) and `from`/`to` (RFC 3339, reuse
   `parse_time_range`), returning `{ sankey, timeline, histogram, hosts, kpis, dropped_total,
   asn_loaded, window }`. KPIs: flows, endpoints, orgs, errors, bytes_in/bytes_out as
   `Option<u64>` sums of observed values + `bytes_partial: bool` (true when any in-scope flow
   had unobservable bytes), p95_ms. Reuse `resolve_ledger_paths` / `run_and_unit_ids` so run
   scope keeps folding dispatched steps and sub-agents.
3. Keep `/api/netflow` (flows list) as is for the table; add optional `workflow`, `origin`,
   `org`, `host` filter params so table cross-filtering is server-side and pageable.
4. Tests mirror `tests/netflow_api.rs` / `tests/host_run_netflow.rs` patterns.

## Client work (`web/src`)

New directory `components/netflow/explorer/`:

- `NetflowExplorer.tsx` — layout + state owner. State: `range` (existing `TimeRangeValue`),
  `view: 'topology' | 'timeline'`, filter sets (`workflows`, `origins`, `orgs`, `hosts`),
  `selectedFlow`. One fetch effect (Promise.all: explorer + flows) keyed on scope + range +
  filters, one error surface, `window` echo threaded everywhere (same pattern as today's
  `pages/Netflow.tsx`).
- `CoveragePopover.tsx` — the "Coverage & gaps" pill + popover. Content = `disclosureText`
  (via `NetflowScopeDisclosure` exports), fidelity legend (reuse `FIDELITY_TITLE` copy from
  `FidelityBadge.tsx` — export it), and the dropped_total sentence (same wording as
  `DroppedBanner`; keep the banner itself for `flows.length === 0` all-dropped cases).
- `ActivityStrip.tsx` — histogram + drag-to-zoom (pointer events, `setPointerCapture`;
  commit only if selection > 1 bucket) + `TimeRangePicker` presets + `NetflowWindowReadout`.
- `KpiStrip.tsx` — six KPI cards; render `†` + tooltip when `bytes_partial`.
- `TopologyView.tsx` + `topologyLayout.ts` — three fixed-x columns (HTML node rows) with an
  SVG ribbon underlay (cubic béziers between row midpoints; width `2 + 24·calls/max`, brand
  stroke at 0.22 opacity, `status.failed` when link error rate > 5%; hover → connected 0.55 /
  others 0.05). Layout is pure and unit-tested like `layout.ts`. Node click toggles the filter;
  nodes with zero calls under current cross-filters render dimmed, not removed (except
  out-of-scope nodes, which are absent from the server response entirely).
- `TimelineView.tsx` — lanes from `TimelineView.lanes`, org group headers, active-runs strip,
  per-lane stats, cell click → zoom range to that bucket (push onto a small zoom stack for
  "Zoom out"), lane click → toggle host filter. Cell shade `opacity = 0.12 + 0.78·calls/max`
  (global max across lanes), 3px `status.failed` baseline when the bucket has errors.
- `OrgCards.tsx` — per-org endpoint cards (Topology view only); rows toggle host filters.
- `FilterChips.tsx` — chips for every active filter + window, removable, Clear all.
- `FlowDetailPanel.tsx` — right slide-over; full record; `FidelityBadge` + explanation line;
  em-dash note for coarse rows ("not observable, never zero").
- Flow table: keep `NetflowTable` but (a) drop the Fidelity column into the detail panel,
  (b) add Run + Workflow columns at non-run scopes, (c) add `onRowClick`.

Wiring: `pages/Netflow.tsx` becomes header + `<NetflowExplorer scope="global" />`. Project and
run Network tabs swap their current graph+summary+table stack for the same component with their
scope. Delete nothing until parity ships; then retire `NetflowGraph.tsx`/`layout.ts` and their
tests in the same PR that removes the last caller.

## Acceptance checklist

- [ ] Global / project / run all render the identical surface; only data narrows.
- [ ] Run scope: window defaults to the run's span; disclosure includes the sub-agent note.
- [ ] Cross-filtering: any combination of node/org-card/lane/chip filters + window agrees
      across KPIs, both views, cards, and the table.
- [ ] Topology stays ≤ ~8 rows per column regardless of run count; Timeline stays one lane per
      endpoint. Nothing scales with number of runs except the runs strip.
- [ ] Coverage popover reachable from every scope; DroppedBanner still shows on all-dropped
      empty states; empty-state copy still branches on the server window echo.
- [ ] No `0 B` ever rendered for unobservable bytes; byte KPIs marked when partial; all visual
      weights are call-based.
- [ ] Dark + light themes, `prefers-reduced-motion` respected for any transitions.
- [ ] Table columns never clip: table cards use `overflow-x: auto`.
