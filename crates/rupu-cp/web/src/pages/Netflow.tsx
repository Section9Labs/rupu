// Netflow — the global egress page: the UNION of every flow rupu recorded,
// across every run, plus `system`-origin traffic (the rare passive
// update-notice check, `Origin::Update`) that carries no run_id and
// therefore never surfaces on a per-run Network tab or a workflow page
// (flows belong to a run, never to a workflow *definition* — nothing
// netflow-shaped is mounted there). Neither CP's own fleet traffic nor
// its ASN-table refresh are among that system-origin traffic — both
// `Origin::Cp` (`HttpHostConnector`) and `Origin::System` ASN downloads
// (`cmd/cp.rs`'s sweep, this crate's own `maybe_refresh_asn`) are wired to
// a `NullSink` and recorded nowhere, at any scope (netflow-per-run plan,
// Task 8). This is also the ONE scope that unions every per-run ledger
// file (`<run_id>.jsonl`) under `$RUPU_HOME/netflow/` alongside every
// registered workspace's own `.rupu/netflow/` — see
// `rupu_cp::api::netflow::read_all_workspaces_sync`'s doc comment.
//
// This is the one place a viewer forms their mental model of what this
// subsystem covers, so <NetflowScopeDisclosure /> below is load-bearing,
// not decoration — it states both the scope limit and the non-HTTP blind
// spot unconditionally, because a page full of rows is exactly when a
// viewer is most likely to assume they're seeing everything.
// NetflowTable's own empty-state hint restates the scope limit for a table
// with zero rows; both quote the same `NETFLOW_COVERAGE_LIST` constant so
// they can't drift apart (Fix 2).

import { useEffect, useState } from 'react';
import NetflowGraph from '../components/netflow/NetflowGraph';
import { NetflowScopeDisclosure } from '../components/netflow/ScopeDisclosure';
import NetflowSummary from '../components/netflow/NetflowSummary';
import NetflowTable from '../components/netflow/NetflowTable';
import {
  fetchGlobalNetflow,
  fetchNetflowGraph,
  type GraphView,
  type NetflowResponse,
} from '../lib/netflow';

export default function Netflow() {
  const [data, setData] = useState<NetflowResponse | null>(null);
  const [graph, setGraph] = useState<GraphView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setGraph(null);
    setError(null);
    // Global scope: call both fetchers with NO scope argument at all,
    // rather than passing an empty string — the API contract distinguishes
    // "no scope param" (global) from a malformed scoped request.
    //
    // Combined into one `Promise.all` (Fix 6) rather than two independent
    // `.then`/`.catch` pairs: a failure on EITHER fetch now surfaces one
    // error message and one retry story instead of a panel that silently
    // renders whatever half succeeded with no explanation for the rest.
    Promise.all([fetchGlobalNetflow(), fetchNetflowGraph()])
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
  }, []);

  return (
    <div className="p-8">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Network</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Every flow recorded across all runs, plus unattributed system egress that belongs to no
          single run.
        </p>
        <NetflowScopeDisclosure scope="global" className="mt-2" />
      </header>

      {error ? (
        <p className="text-sm text-err">{error}</p>
      ) : data === null ? (
        <p className="text-sm text-ink-dim">Loading network flows…</p>
      ) : (
        <div className="space-y-6">
          {graph && <NetflowGraph graph={graph} scope="global" />}
          <NetflowSummary hosts={data.hosts} />
          <NetflowTable flows={data.flows} dropped={data.dropped} asnLoaded={data.asn_loaded} />
        </div>
      )}
    </div>
  );
}
