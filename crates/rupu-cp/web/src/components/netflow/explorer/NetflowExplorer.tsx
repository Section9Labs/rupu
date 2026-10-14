// NetflowExplorer — ONE surface, every scope (the v3 redesign's core
// decision): the global Netflow page, the project Network tab, and the run
// Network tab all render this component; scope comes from the ROUTE and
// only narrows the data. There is no scope breadcrumb here — that was
// mockup-only simulation.
//
// Layout + state owner. All aggregation is server-side
// (`GET /api/netflow/explorer` + the filtered flows list — see
// `rupu_netflow::ledger::explorer`'s module doc). The window and the
// cross-filters are one query line (`lib/query/lists/netflow.ts` turns it
// into the reads' params; graph clicks and drag-zooms write terms into it);
// this component holds the query, the view switcher, the selected flow and
// the zoom stack (the window terms each zoom replaced, so a zoom out puts
// back only the window and keeps every term written since), and re-fetches when the query's params change, so KPIs,
// both visualizations, the org cards, and the table always describe the
// same server-computed set. With no window in the query it opens on the
// default (`defaultWindow`): the run's span on a run's tab, else the
// shorter of the last hour and the latest run's span.
//
// Two independent fetch effects — the aggregates (a few KB) and the flows
// list (can be large) — each with its own error surface, so every section
// renders as soon as its own response arrives; the server `window` echo is
// threaded to every consumer that words an empty state or reads out the
// window — never the query's own reading of it. The query lives in the
// URL's `nq`. Mount this with a `key` of the scope id so a scope-target
// change (e.g. RunDetail switching runs) resets the rest of the view state.

import { useEffect, useMemo, useRef, useState } from 'react';
import {
  fetchAgentiflowNetflow,
  fetchGlobalNetflow,
  fetchNetflowExplorer,
  fetchNetflowLatest,
  fetchProjectNetflow,
  fetchRunNetflow,
  filtersAreEmpty,
  type ExplorerResponse,
  type FlowView,
  type LatestRun,
  type NetflowFilters,
  type NetflowRange,
  type NetflowResponse,
} from '../../../lib/netflow';
import {
  NETFLOW_FIELDS,
  defaultWindow,
  defaultWindowText,
  netflowRequest,
  netflowWindowTerms,
  toggleNetflowValue,
  withNetflowWindow,
  withNetflowWindowTerms,
  type NetflowDim,
} from '../../../lib/query/lists/netflow';
import type { FacetValue } from '../../query/suggest';
import { ListQueryBar, useListQuery } from '../../query/ListQueryBar';
import { Segmented } from '../../ui/Segmented';
import { type NetflowScope } from '../ScopeDisclosure';
import NetflowTable from '../NetflowTable';
import ActivityStrip from './ActivityStrip';
import CoveragePopover from './CoveragePopover';
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
  /** Required at `agentiflow` scope. */
  agentiflowId?: string;
  /** Run and agentiflow scope: the run's own span, the window the explorer
   *  opens on when the query names none (the server still echoes whatever
   *  it actually enforced). An unfinished run passes no `to`, leaving the
   *  upper bound open. */
  initialWindow?: { from?: string; to?: string };
}

type ViewMode = 'topology' | 'timeline';

const VIEW_OPTIONS = [
  { value: 'topology', label: 'Topology' },
  { value: 'timeline', label: 'Timeline' },
];

const PANEL_HINT: Record<ViewMode, string> = {
  topology: 'Click any node to add it to the query. Ribbon width = calls; red = errors on the path.',
  timeline: 'Same query, over time. Click an endpoint to add it; click a cell to zoom the window.',
};

const TOPO_DIM_TO_KEY: Record<TopoDim, NetflowDim> = {
  wf: 'workflow',
  or: 'origin',
  org: 'org',
};

/** The latest run, once asked for (global and project scope). */
type Latest = { state: 'loading' } | { state: 'ready'; run: LatestRun | null } | { state: 'failed'; error: string };

/** What the reads ask for: the query's params, its window or the default. */
interface Applied {
  range: NetflowRange;
  filters?: NetflowFilters;
  note: string | null;
}

export function NetflowExplorer({
  scope,
  projectId,
  runId,
  agentiflowId,
  initialWindow,
}: NetflowExplorerProps) {
  const [q, setQ] = useListQuery('nq');
  const [view, setView] = useState<ViewMode>('topology');
  const [zoomStack, setZoomStack] = useState<string[][]>([]);
  const [selectedFlow, setSelectedFlow] = useState<FlowView | null>(null);
  const [explorer, setExplorer] = useState<ExplorerResponse | null>(null);
  const [flows, setFlows] = useState<NetflowResponse | null>(null);
  const [explorerError, setExplorerError] = useState<string | null>(null);
  const [flowsError, setFlowsError] = useState<string | null>(null);
  // True while a refetch is in flight OVER existing data, per request (the
  // initial load has its own per-section loading state). Previous data
  // staying on screen is deliberate — but only honest with a visible
  // "Updating…" beside it, or whole-history numbers would render under a
  // just-narrowed query with nothing saying so.
  const [explorerRefreshing, setExplorerRefreshing] = useState(false);
  const [flowsRefreshing, setFlowsRefreshing] = useState(false);

  const scopeParam =
    scope === 'run' && runId
      ? `run:${runId}`
      : scope === 'agentiflow' && agentiflowId
        ? `agentiflow:${agentiflowId}`
        : scope === 'project' && projectId
          ? `project:${projectId}`
          : undefined;

  // A run's own tab opens on the run's span; global and project scope on
  // the shorter of the last hour and the latest run's span.
  const ownSpan = scope === 'run' || scope === 'agentiflow';
  const [latest, setLatest] = useState<Latest>({ state: 'loading' });
  useEffect(() => {
    if (ownSpan) return;
    let cancelled = false;
    fetchNetflowLatest(scope === 'project' && projectId ? `project:${projectId}` : undefined)
      .then((r) => !cancelled && setLatest({ state: 'ready', run: r.latest }))
      .catch((e: unknown) => !cancelled && setLatest({ state: 'failed', error: e instanceof Error ? e.message : String(e) }));
    return () => {
      cancelled = true;
    };
  }, [ownSpan, scope, projectId]);

  const request = useMemo(() => netflowRequest(q, Date.now()), [q]);
  // A query that doesn't translate keeps what the last one asked for: the
  // line shows why, the views stay put.
  const lastApplied = useRef<Applied | null>(null);
  const applied = useMemo<Applied | null>(() => {
    if (!request.ok) return lastApplied.current;
    const filters = filtersAreEmpty(request.filters) ? undefined : request.filters;
    if (request.window !== 'default') return { range: request.range, filters, note: null };
    if (!ownSpan && latest.state === 'loading') return null;
    const now = Date.now();
    const w = defaultWindow(now, ownSpan ? { span: initialWindow ?? {} } : { latest: latest.state === 'ready' ? latest.run : null });
    const failed = latest.state === 'failed' ? ` (The latest run couldn't be read: ${latest.error}.)` : '';
    return { range: w.range, filters, note: defaultWindowText(w, now) + failed };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request, latest, ownSpan]);
  lastApplied.current = applied;
  const appliedKey = applied ? JSON.stringify([applied.range, applied.filters ?? null]) : null;

  // The aggregates and the flows list load independently: each section
  // renders as soon as its own response arrives. Previous data stays on
  // screen during a refetch — a narrowing query must not collapse the
  // whole surface to a loading line and rebuild it.
  useEffect(() => {
    if (!applied) return;
    let cancelled = false;
    setExplorerError(null);
    setExplorerRefreshing(true);
    fetchNetflowExplorer(scopeParam, applied.range, applied.filters)
      .then((e) => {
        if (cancelled) return;
        setExplorer(e);
        setExplorerRefreshing(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setExplorerRefreshing(false);
        setExplorerError(e instanceof Error ? e.message : 'Failed to load network flows');
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope, projectId, runId, agentiflowId, appliedKey]);

  useEffect(() => {
    if (!applied) return;
    let cancelled = false;
    setFlowsError(null);
    setFlowsRefreshing(true);
    const { range, filters } = applied;
    const flowsFetch =
      scope === 'run' && runId
        ? fetchRunNetflow(runId, range, filters)
        : scope === 'agentiflow' && agentiflowId
          ? fetchAgentiflowNetflow(agentiflowId, range, filters)
          : scope === 'project' && projectId
            ? fetchProjectNetflow(projectId, range, filters)
            : fetchGlobalNetflow(range, filters);
    flowsFetch
      .then((fl) => {
        if (cancelled) return;
        setFlows(fl);
        setFlowsRefreshing(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setFlowsRefreshing(false);
        setFlowsError(e instanceof Error ? e.message : 'Failed to load network flows');
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope, projectId, runId, agentiflowId, appliedKey]);

  // Suggestions: the values the scope's aggregates hold, with their calls.
  const facets = useMemo((): Record<string, FacetValue[]> => {
    if (!explorer) return {};
    const nodes = (xs: { id: string; label: string; calls: number }[]): FacetValue[] =>
      xs.map((n) => ({ value: n.id, count: n.calls, detail: n.label !== n.id ? n.label : undefined }));
    return {
      workflow: nodes(explorer.sankey.workflows),
      origin: nodes(explorer.sankey.origins),
      org: nodes(explorer.sankey.orgs),
      host: explorer.timeline.lanes.map((l) => ({ value: `${l.host}:${l.port}`, count: l.calls, detail: l.org })),
    };
  }, [explorer]);

  /** A query edit: a new context, so the zoom stack resets. */
  function changeQuery(next: string) {
    setSelectedFlow(null);
    setZoomStack([]);
    setQ(next);
  }

  function toggle(key: NetflowDim, value: string) {
    setSelectedFlow(null);
    setQ(toggleNetflowValue(q, key, value));
  }

  /** Drag- or cell-initiated zoom: push the window it replaces so "Zoom
   *  out" can unwind step by step. */
  function zoomTo(fromIso: string, toIso: string) {
    setSelectedFlow(null);
    setZoomStack((prev) => [...prev, netflowWindowTerms(q)]);
    setQ(withNetflowWindow(q, fromIso, toIso));
  }

  function zoomOut() {
    setSelectedFlow(null);
    if (zoomStack.length === 0) return;
    setQ(withNetflowWindowTerms(q, zoomStack[zoomStack.length - 1]));
    setZoomStack(zoomStack.slice(0, -1));
  }

  const filters = applied?.filters;
  const selectedTopo: Record<TopoDim, string[]> = {
    wf: filters?.workflows ?? [],
    or: filters?.origins ?? [],
    org: filters?.orgs ?? [],
  };

  const filtersActive = filters !== undefined;
  const updating =
    (explorerRefreshing && explorer !== null) || (flowsRefreshing && flows !== null);
  // Nothing of the aggregates yet: their section is the loading line alone.
  const explorerLoading = explorer === null && !explorerError;

  return (
    <div className="space-y-4">
      <ListQueryBar
        className="mb-0"
        value={q}
        onChange={changeQuery}
        fields={NETFLOW_FIELDS}
        facets={facets}
        label="Filter network flows"
        placeholder="Filter flows… e.g. since:6h origin:provider:anthropic -host:api.github.com:443"
        error={request.ok ? null : request.error}
        alerts={applied?.note && request.ok ? [<span key="window" data-testid="netflow-default-window">{applied.note}</span>] : []}
      />
      {(updating || (explorer && flows)) && (
        <div className="flex items-center justify-end gap-3">
          {updating && (
            <p role="status" className="text-note text-ink-mute">
              Updating…
            </p>
          )}
          {explorer && flows && (
            <CoveragePopover
              scope={scope}
              droppedTotal={explorer.dropped_total}
              capture={flows.capture}
            />
          )}
        </div>
      )}
      {explorerError ? (
        // The query stays reachable above an aggregates error — changing it
        // is the retry affordance.
        <p className="text-sm text-err">{explorerError}</p>
      ) : explorer === null ? (
        <p className="text-sm text-ink-dim">Loading network flows…</p>
      ) : (
        <>
          <ActivityStrip
            histogram={explorer.histogram}
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
                onToggle={(dim, key) => toggle(TOPO_DIM_TO_KEY[dim], key)}
                scope={scope}
                appliedWindow={explorer.window}
              />
            ) : (
              <TimelineView
                timeline={explorer.timeline}
                selectedHosts={filters?.hosts ?? []}
                onToggleHost={(key) => toggle('host', key)}
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
              selectedHosts={filters?.hosts ?? []}
              onToggleHost={(key) => toggle('host', key)}
            />
          )}
        </>
      )}
      {flowsError ? (
        <p className="text-sm text-err">{flowsError}</p>
      ) : flows === null ? (
        // One loading line while neither section has arrived, as before.
        !explorerLoading && <p className="text-sm text-ink-dim">Loading network flows…</p>
      ) : (
        <NetflowTable
          flows={flows.flows}
          droppedTotal={flows.dropped_total}
          incomplete={flows.incomplete}
          asnLoaded={flows.asn_loaded}
          scope={scope}
          appliedWindow={flows.window}
          showAttribution={scope !== 'run'}
          onRowClick={setSelectedFlow}
          filtersActive={filtersActive}
        />
      )}
      <FlowDetailPanel flow={selectedFlow} scope={scope} onClose={() => setSelectedFlow(null)} />
    </div>
  );
}

export default NetflowExplorer;
