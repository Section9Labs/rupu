// Project Network tab body — the project-scoped netflow aggregate: every
// flow across every run under this project, PLUS `system`-origin egress
// (updater, ASN refresh, CP fleet traffic) that carries no run_id and so
// never surfaces on a per-run Network tab. That's a property of this
// scope, not an accident — those flows only ever attach to a workspace,
// never to a single run.
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

import { useEffect, useState } from 'react';
import NetflowGraph from '../netflow/NetflowGraph';
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

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    setData(null);
    setGraph(null);
    fetchProjectNetflow(wsId)
      .then((d) => { if (!cancelled) setData(d); })
      .catch(() => { if (!cancelled) setData(null); });
    fetchNetflowGraph(`project:${wsId}`)
      .then((g) => { if (!cancelled) setGraph(g); })
      .catch(() => { if (!cancelled) setGraph(null); });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  return (
    <div className="space-y-4">
      {graph && <NetflowGraph graph={graph} />}
      {data && <NetflowSummary hosts={data.hosts} />}
      {data && (
        <NetflowTable flows={data.flows} dropped={data.dropped} asnLoaded={data.asn_loaded} />
      )}
    </div>
  );
}
