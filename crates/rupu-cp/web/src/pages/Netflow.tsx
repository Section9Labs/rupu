// Netflow — the global egress page: the UNION of every flow rupu recorded,
// across every run. `Origin::System` covers several unrelated things —
// auth/oauth token exchange (`resolver.rs`, `oauth/device.rs`,
// `oauth/callback.rs`), the theme-URL fetch (`output/theme.rs`), and the
// ASN-table refresh (`cmd/cp.rs`'s sweep, this crate's own
// `maybe_refresh_asn`) — and EVERY production construction site for it
// builds its client with `Arc::new(NullSink)`. None of that traffic
// reaches any ledger, at any scope; see `ScopeDisclosure.tsx`'s header
// comment for the full accounting.
//
// The update checker (`Origin::Update`, `rupu-update`'s client) and CP's
// own fleet traffic (`Origin::Cp`, `HttpHostConnector`) are the same
// story: both wired to `Arc::new(NullSink)`, both produce no flow record
// at all, at any scope. This is also the ONE scope that unions every
// per-run ledger file (`<run_id>.jsonl`) under `$RUPU_HOME/netflow/`
// alongside every registered workspace's own `.rupu/netflow/` — see
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
import TimeRangePicker, { toNetflowRange, type TimeRangeValue } from '../components/netflow/TimeRangePicker';
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
  // Relative-default time-range filter (Task 4). `toNetflowRange` collapses
  // the 'all' preset (and an as-yet-unapplied 'custom') to `undefined`, so
  // the fetches below stay call-compatible with the pre-Task-4 "no scope,
  // no range" shape whenever no filter is actually in effect.
  const [range, setRange] = useState<TimeRangeValue>({ preset: 'all' });

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setGraph(null);
    setError(null);
    const q = toNetflowRange(range);
    // Global scope: call both fetchers with NO scope argument at all,
    // rather than passing an empty string — the API contract distinguishes
    // "no scope param" (global) from a malformed scoped request. The range
    // argument is likewise omitted entirely (not passed as `undefined`)
    // when `q` is `undefined`, so an unfiltered load calls exactly as it
    // did before this picker existed.
    //
    // Combined into one `Promise.all` (Fix 6) rather than two independent
    // `.then`/`.catch` pairs: a failure on EITHER fetch now surfaces one
    // error message and one retry story instead of a panel that silently
    // renders whatever half succeeded with no explanation for the rest.
    Promise.all([
      q ? fetchGlobalNetflow(q) : fetchGlobalNetflow(),
      q ? fetchNetflowGraph(undefined, q) : fetchNetflowGraph(),
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
  }, [range]);

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

      <div className="mb-4">
        <TimeRangePicker value={range} onChange={setRange} />
      </div>

      {error ? (
        <p className="text-sm text-err">{error}</p>
      ) : data === null ? (
        <p className="text-sm text-ink-dim">Loading network flows…</p>
      ) : (
        <div className="space-y-6">
          {graph && <NetflowGraph graph={graph} scope="global" />}
          <NetflowSummary hosts={data.hosts} />
          <NetflowTable
            flows={data.flows}
            droppedTotal={data.dropped_total}
            asnLoaded={data.asn_loaded}
            scope="global"
            appliedWindow={data.window}
          />
        </div>
      )}
    </div>
  );
}
