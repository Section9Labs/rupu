// Netflow — the global egress page: the UNION of every flow rupu recorded,
// across every run, plus `system`-origin traffic (updater, ASN refresh, CP
// fleet traffic) that carries no run_id and therefore never surfaces on a
// per-run Network tab or a workflow page (flows belong to a run, never to a
// workflow *definition* — nothing netflow-shaped is mounted there).
//
// This is the one place a viewer forms their mental model of what this
// subsystem covers, so the two honesty notes below are load-bearing, not
// decoration:
//   1. Scope limit — netflow only sees rupu's OWN egress (provider APIs,
//      SCM connectors, MCP, webhooks, the updater, CP fleet traffic), never
//      the agent's `bash` subprocess.
//   2. Non-HTTP blind spot — `git2` clones (often a run's largest byte
//      volume), `object_store` bucket traffic, and the node WebSocket
//      aren't captured by this subsystem at all, HTTP or not.
// NetflowTable's own empty-state hint restates (1) for a table with zero
// rows; this header states both unconditionally, because a page full of
// rows is exactly when a viewer is most likely to assume they're seeing
// everything.

import { useEffect, useState } from 'react';
import NetflowGraph from '../components/netflow/NetflowGraph';
import NetflowSummary from '../components/netflow/NetflowSummary';
import NetflowTable from '../components/netflow/NetflowTable';
import { Select } from '../components/ui/Select';
import {
  fetchGlobalNetflow,
  fetchNetflowGraph,
  type GraphView,
  type NetflowResponse,
} from '../lib/netflow';

type WeightBy = 'calls' | 'bytes';

export default function Netflow() {
  const [data, setData] = useState<NetflowResponse | null>(null);
  const [graph, setGraph] = useState<GraphView | null>(null);
  const [weightBy, setWeightBy] = useState<WeightBy>('calls');

  useEffect(() => {
    let cancelled = false;
    // Global scope: call both fetchers with NO scope argument at all,
    // rather than passing an empty string — the API contract distinguishes
    // "no scope param" (global) from a malformed scoped request.
    fetchGlobalNetflow()
      .then((d) => { if (!cancelled) setData(d); })
      .catch(() => { if (!cancelled) setData(null); });
    fetchNetflowGraph()
      .then((g) => { if (!cancelled) setGraph(g); })
      .catch(() => { if (!cancelled) setGraph(null); });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="p-8">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Network</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Every flow recorded across all runs, plus unattributed system egress that belongs to no
          single run.
        </p>
        <p className="mt-2 text-note text-ink-mute max-w-2xl">
          This covers rupu&apos;s own egress — provider APIs, SCM connectors, MCP, webhooks, the
          updater, and CP fleet traffic — never traffic from the agent&apos;s bash subprocess. It
          also can&apos;t see non-HTTP egress: git2 clones (often a run&apos;s largest byte
          volume), object_store bucket traffic, and the node WebSocket are invisible here too.
        </p>
      </header>

      <div className="mb-4 flex items-center gap-2">
        <label className="text-note text-ink-dim" htmlFor="netflow-weight-by">
          Weight edges by
        </label>
        <Select
          id="netflow-weight-by"
          value={weightBy}
          onChange={(e) => setWeightBy(e.target.value as WeightBy)}
        >
          <option value="calls">calls</option>
          <option value="bytes">bytes</option>
        </Select>
      </div>

      <div className="space-y-6">
        {graph && <NetflowGraph graph={graph} weightBy={weightBy} />}
        {data && <NetflowSummary hosts={data.hosts} />}
        {data && (
          <NetflowTable flows={data.flows} dropped={data.dropped} asnLoaded={data.asn_loaded} />
        )}
      </div>
    </div>
  );
}
