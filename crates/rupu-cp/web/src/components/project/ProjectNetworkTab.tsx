// Project Network tab body — the project-scoped netflow aggregate: every
// flow across every run under this project, PLUS `system`-origin egress
// (updater, ASN refresh) that carries no run_id and so never surfaces on a
// per-run Network tab. That's a property of this scope, not an accident —
// those flows only ever attach to a workspace, never to a single run. This
// scope deliberately does NOT include the CP daemon's own global ledger —
// that traffic belongs to the CP fleet as a whole, not to this project; see
// `rupu_cp::api::netflow::get_project_netflow`'s doc comment (Fix 1,
// netflow Plan 3 review round 3).
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

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    setData(null);
    setGraph(null);
    setError(null);
    // Combined into one `Promise.all` (Fix 6) — see pages/Netflow.tsx's
    // identical comment for why a joint loading/error state is preferred
    // over two independently-failing fetches with no explanation.
    Promise.all([fetchProjectNetflow(wsId), fetchNetflowGraph(`project:${wsId}`)])
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
  }, [wsId]);

  return (
    <div className="space-y-4">
      <NetflowScopeDisclosure scope="project" />
      {error ? (
        <p className="text-sm text-err">{error}</p>
      ) : data === null ? (
        <p className="text-sm text-ink-dim">Loading network flows…</p>
      ) : (
        <>
          {graph && <NetflowGraph graph={graph} scope="project" />}
          <NetflowSummary hosts={data.hosts} />
          <NetflowTable flows={data.flows} dropped={data.dropped} asnLoaded={data.asn_loaded} />
        </>
      )}
    </div>
  );
}
