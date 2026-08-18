// Project Network tab body — the project-scoped netflow aggregate: every
// flow across every run under this project. This scope deliberately does
// NOT include the update checker's traffic (`Origin::Update`), the CP
// daemon's own fleet traffic (`Origin::Cp`), or `Origin::System` — which
// covers auth/oauth token exchange, the theme-URL fetch, AND the
// ASN-table refresh (`cmd/cp.rs`'s sweep / this crate's
// `maybe_refresh_asn`) — not because any of those live somewhere else,
// but because none of them live anywhere: every production construction
// site for all three origins is wired to a `NullSink` (netflow-per-run
// plan), so they're recorded at no scope, this one included; see
// `ScopeDisclosure.tsx`'s header comment and
// `rupu_cp::api::netflow::get_project_netflow`'s doc comment.
//
// KNOWN GAP (Blocker 1, whole-branch review): `get_project_netflow` reads
// only `<workspace>/.rupu/netflow/`, and nothing creates that directory on
// `rupu init` — so on a default install this scope is EMPTY for every
// project, even ones with real captured runs, because those runs' ledgers
// all landed at the global fallback instead. `NetflowTable`'s empty-state
// hint (`scope="project"`) says so explicitly; this comment is not a
// substitute for reading that fix — see
// `ScopeDisclosure.tsx`'s `netflowEmptyStateHint`.
//
// Mirrors ProjectCoverageTab's shape: self-fetches on the `wsId` prop, no
// filter chips. This tab body only mounts while "Network" is the active
// ProjectDetail tab (see the `tab === 'network' && ...` gate in
// ProjectDetail.tsx), so the mount-time fetch below already IS this page's
// lazy-load — no ref-guard is needed the way RunDetail's longer-lived tab
// body needs one (RunDetail keeps all tab bodies mounted across tab
// switches; ProjectDetail doesn't).
//
// NetflowSummary is mounted directly beside NetflowTable (same order the
// run-scoped Network tab uses: graph, summary, table) precisely so its
// "renders nothing when hosts is empty" behavior is never orphaned without
// explanation — NetflowTable's own empty state covers that case for both.
//
// <NetflowScopeDisclosure /> (Fix 2) is the SAME component pages/Netflow.tsx
// and RunDetail.tsx (Fix 3) render — previously this was a hand-copied
// paragraph that had already drifted (it claimed MCP/webhook coverage that
// doesn't exist); sharing one component is what keeps that from recurring.

import { useEffect, useState } from 'react';
import NetflowGraph from '../netflow/NetflowGraph';
import { NetflowScopeDisclosure } from '../netflow/ScopeDisclosure';
import NetflowSummary from '../netflow/NetflowSummary';
import NetflowTable from '../netflow/NetflowTable';
import NetflowWindowReadout from '../netflow/NetflowWindowReadout';
import TimeRangePicker, { toNetflowRange, type TimeRangeValue } from '../netflow/TimeRangePicker';
import {
  fetchNetflowGraph,
  fetchProjectNetflow,
  type GraphView,
  type NetflowResponse,
} from '../../lib/netflow';

export default function ProjectNetworkTab({ wsId }: { wsId: string }) {
  const [data, setData] = useState<NetflowResponse | null>(null);
  const [graph, setGraph] = useState<GraphView | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Relative-default time-range filter (Task 4) — see pages/Netflow.tsx's
  // identical field for the `toNetflowRange`/call-compatibility rationale.
  const [range, setRange] = useState<TimeRangeValue>({ preset: 'all' });

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    setData(null);
    setGraph(null);
    setError(null);
    const q = toNetflowRange(range);
    // Combined into one `Promise.all` (Fix 6) — see pages/Netflow.tsx's
    // identical comment for why a joint loading/error state is preferred
    // over two independently-failing fetches with no explanation.
    Promise.all([
      q ? fetchProjectNetflow(wsId, q) : fetchProjectNetflow(wsId),
      q ? fetchNetflowGraph(`project:${wsId}`, q) : fetchNetflowGraph(`project:${wsId}`),
    ])
      .then(([d, g]) => {
        if (cancelled) return;
        setData(d);
        setGraph(g);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load network flows');
      });
    return () => {
      cancelled = true;
    };
  }, [wsId, range]);

  return (
    <div className="space-y-4">
      <NetflowScopeDisclosure scope="project" />
      <div className="flex flex-wrap items-center gap-3">
        <TimeRangePicker value={range} onChange={setRange} />
        {/* Server-echoed, never picker-state-derived — see the component's
            header comment (whole-branch review round 1, "Also do"). */}
        {data && <NetflowWindowReadout appliedWindow={data.window} />}
      </div>
      {error ? (
        <p className="text-sm text-err">{error}</p>
      ) : data === null ? (
        <p className="text-sm text-ink-dim">Loading network flows…</p>
      ) : (
        <>
          {graph && <NetflowGraph graph={graph} scope="project" appliedWindow={data.window} />}
          <NetflowSummary hosts={data.hosts} />
          <NetflowTable
            flows={data.flows}
            droppedTotal={data.dropped_total}
            asnLoaded={data.asn_loaded}
            scope="project"
            appliedWindow={data.window}
          />
        </>
      )}
    </div>
  );
}
