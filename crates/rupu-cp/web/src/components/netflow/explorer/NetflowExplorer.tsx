// NetflowExplorer — ONE surface, every scope (the v3 redesign's core
// decision): the global Netflow page, the project Network tab, and the run
// Network tab all render this component; scope comes from the ROUTE and
// only narrows the data. There is no scope breadcrumb here — that was
// mockup-only simulation.
//
// Layout + state owner. All aggregation is server-side
// (`GET /api/netflow/explorer` + the filtered flows list — see
// `rupu_netflow::ledger::explorer`'s module doc); this component only
// holds view state (window, cross-filter sets, view switcher, selected
// flow, zoom stack) and re-fetches when it changes, so KPIs, both
// visualizations, the org cards, and the table always describe the same
// server-computed set.
//
// One fetch effect, one Promise.all, one error surface (the Fix 6
// pattern from the pre-v3 pages); the server `window` echo is threaded to
// every consumer that words an empty state or reads out the window —
// never picker state. Mount this with a `key` of the scope id so a
// scope-target change (e.g. RunDetail switching runs) resets all view
// state instead of leaking the previous target's filters.

import { useEffect, useMemo, useState } from 'react';
import {
  fetchGlobalNetflow,
  fetchNetflowExplorer,
  fetchProjectNetflow,
  fetchRunNetflow,
  filtersAreEmpty,
  EMPTY_NETFLOW_FILTERS,
  type ExplorerResponse,
  type FlowView,
  type NetflowFilters,
  type NetflowResponse,
} from '../../../lib/netflow';
import { Segmented } from '../../ui/Segmented';
import { netflowWindowApplied, type NetflowScope } from '../ScopeDisclosure';
import NetflowTable from '../NetflowTable';
import TimeRangePicker, { toNetflowRange, type TimeRangeValue } from '../TimeRangePicker';
import ActivityStrip from './ActivityStrip';
import CoveragePopover from './CoveragePopover';
import FilterChips, { type FilterDim } from './FilterChips';
import FlowDetailPanel from './FlowDetailPanel';
import KpiStrip from './KpiStrip';
import OrgCards from './OrgCards';
import TimelineView from './TimelineView';
import TopologyView from './TopologyView';
import type { TopoDim } from './topologyLayout';

export interface NetflowExplorerProps {
  scope: NetflowScope;
  /** Required at `project` scope. */
  projectId?: string;
  /** Required at `run` scope. */
  runId?: string;
  /** Run scope: the run's own span, applied as the initial window (the
   *  server still echoes whatever it actually enforced). An unfinished
   *  run passes no `to`, leaving the upper bound open. */
  initialWindow?: { from?: string; to?: string };
}

type ViewMode = 'topology' | 'timeline';

const VIEW_OPTIONS = [
  { value: 'topology', label: 'Topology' },
  { value: 'timeline', label: 'Timeline' },
];

const PANEL_HINT: Record<ViewMode, string> = {
  topology: 'Click any node to filter everything below. Ribbon width = calls; red = errors on the path.',
  timeline: 'Same filters, over time. Click an endpoint to filter; click a cell to zoom the window.',
};

const TOPO_DIM_TO_FILTER: Record<TopoDim, FilterDim> = {
  wf: 'workflows',
  or: 'origins',
  org: 'orgs',
};

function toggleIn(list: string[], key: string): string[] {
  return list.includes(key) ? list.filter((k) => k !== key) : [...list, key];
}

export function NetflowExplorer({ scope, projectId, runId, initialWindow }: NetflowExplorerProps) {
  const [range, setRange] = useState<TimeRangeValue>(() =>
    initialWindow && (initialWindow.from || initialWindow.to)
      ? { preset: 'custom', from: initialWindow.from, to: initialWindow.to }
      : { preset: 'all' },
  );
  const [view, setView] = useState<ViewMode>('topology');
  const [filters, setFilters] = useState<NetflowFilters>(EMPTY_NETFLOW_FILTERS);
  const [zoomStack, setZoomStack] = useState<TimeRangeValue[]>([]);
  const [selectedFlow, setSelectedFlow] = useState<FlowView | null>(null);
  const [explorer, setExplorer] = useState<ExplorerResponse | null>(null);
  const [flows, setFlows] = useState<NetflowResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  // True while a refetch is in flight OVER existing data (the initial
  // load has its own full-surface loading state). Previous data staying
  // on screen is deliberate — but only honest with a visible "Updating…"
  // beside it, or whole-history numbers would render under a
  // just-selected narrower label with nothing saying so.
  const [refreshing, setRefreshing] = useState(false);

  const scopeParam =
    scope === 'run' && runId
      ? `run:${runId}`
      : scope === 'project' && projectId
        ? `project:${projectId}`
        : undefined;

  useEffect(() => {
    let cancelled = false;
    setError(null);
    const q = toNetflowRange(range);
    const f = filtersAreEmpty(filters) ? undefined : filters;
    const flowsFetch =
      scope === 'run' && runId
        ? fetchRunNetflow(runId, q, f)
        : scope === 'project' && projectId
          ? fetchProjectNetflow(projectId, q, f)
          : fetchGlobalNetflow(q, f);
    // Previous data stays on screen while a refetch is in flight — a
    // cross-filter click narrowing the set must not collapse the whole
    // surface to a loading line and rebuild it. `refreshing` marks the
    // interim so the stale numbers are never silently presented as the
    // new selection's.
    setRefreshing(true);
    Promise.all([fetchNetflowExplorer(scopeParam, q, f), flowsFetch])
      .then(([e, fl]) => {
        if (cancelled) return;
        setExplorer(e);
        setFlows(fl);
        setRefreshing(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setRefreshing(false);
        setError(e instanceof Error ? e.message : 'Failed to load network flows');
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope, projectId, runId, range, filters]);

  const orgLabels = useMemo(() => {
    const m = new Map<string, string>();
    for (const o of explorer?.sankey.orgs ?? []) m.set(o.id, o.label);
    return m;
  }, [explorer]);

  function toggleFilter(dim: FilterDim, key: string) {
    setSelectedFlow(null);
    setFilters((prev) => ({ ...prev, [dim]: toggleIn(prev[dim], key) }));
  }

  /** Picker-initiated range change: a new context, so the zoom stack resets. */
  function changeRange(next: TimeRangeValue) {
    setSelectedFlow(null);
    setZoomStack([]);
    setRange(next);
  }

  /** Drag- or cell-initiated zoom: push the current window so "Zoom out"
   *  can unwind step by step. */
  function zoomTo(fromIso: string, toIso: string) {
    setSelectedFlow(null);
    setZoomStack((prev) => [...prev, range]);
    setRange({ preset: 'custom', from: fromIso, to: toIso });
  }

  function zoomOut() {
    setSelectedFlow(null);
    setZoomStack((prev) => {
      if (prev.length === 0) return prev;
      setRange(prev[prev.length - 1]);
      return prev.slice(0, -1);
    });
  }

  function clearAll() {
    setSelectedFlow(null);
    setFilters(EMPTY_NETFLOW_FILTERS);
    setZoomStack([]);
    setRange({ preset: 'all' });
  }

  if (error) {
    // The picker stays reachable so changing the window (or re-choosing a
    // preset) is the retry affordance — an error state with no way back
    // out would strand the operator.
    return (
      <div className="space-y-3">
        <TimeRangePicker value={range} onChange={changeRange} />
        <p className="text-sm text-err">{error}</p>
      </div>
    );
  }
  if (explorer === null || flows === null) {
    return <p className="text-sm text-ink-dim">Loading network flows…</p>;
  }

  const selectedTopo: Record<TopoDim, string[]> = {
    wf: filters.workflows,
    or: filters.origins,
    org: filters.orgs,
  };

  const filtersActive = !filtersAreEmpty(filters);

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-end gap-3">
        {refreshing && (
          <p role="status" className="text-note text-ink-mute">
            Updating…
          </p>
        )}
        <CoveragePopover scope={scope} droppedTotal={explorer.dropped_total} />
      </div>
      <ActivityStrip
        histogram={explorer.histogram}
        range={range}
        onRangeChange={changeRange}
        onZoom={zoomTo}
        appliedWindow={explorer.window}
      />
      <KpiStrip kpis={explorer.kpis} />
      <div className="rounded-xl border border-border bg-panel px-5 py-4">
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
          <Segmented
            ariaLabel="Visualization"
            size="sm"
            options={VIEW_OPTIONS}
            value={view}
            onChange={(v) => setView(v as ViewMode)}
          />
          <p className="text-note text-ink-mute">{PANEL_HINT[view]}</p>
        </div>
        {view === 'topology' ? (
          <TopologyView
            sankey={explorer.sankey}
            selected={selectedTopo}
            onToggle={(dim, key) => toggleFilter(TOPO_DIM_TO_FILTER[dim], key)}
            scope={scope}
            appliedWindow={explorer.window}
          />
        ) : (
          <TimelineView
            timeline={explorer.timeline}
            selectedHosts={filters.hosts}
            onToggleHost={(key) => toggleFilter('hosts', key)}
            onZoomBucket={zoomTo}
            canZoomOut={zoomStack.length > 0}
            onZoomOut={zoomOut}
            scope={scope}
            appliedWindow={explorer.window}
          />
        )}
      </div>
      {view === 'topology' && (
        <OrgCards
          lanes={explorer.timeline.lanes}
          selectedHosts={filters.hosts}
          onToggleHost={(key) => toggleFilter('hosts', key)}
        />
      )}
      <FilterChips
        filters={filters}
        orgLabel={(key) => orgLabels.get(key) ?? key}
        windowApplied={netflowWindowApplied(explorer.window)}
        onRemove={toggleFilter}
        onClearWindow={() => changeRange({ preset: 'all' })}
        onClearAll={clearAll}
      />
      <NetflowTable
        flows={flows.flows}
        droppedTotal={flows.dropped_total}
        asnLoaded={flows.asn_loaded}
        scope={scope}
        appliedWindow={flows.window}
        showAttribution={scope !== 'run'}
        onRowClick={setSelectedFlow}
        filtersActive={filtersActive}
      />
      <FlowDetailPanel flow={selectedFlow} scope={scope} onClose={() => setSelectedFlow(null)} />
    </div>
  );
}

export default NetflowExplorer;
