// Netflow — the global egress page: the UNION of every flow rupu recorded,
// across every run (see `rupu_cp::api::netflow::read_all_workspaces_sync`'s
// doc comment for the directory union). `Origin::System`/`Update`/`Cp`
// traffic reaches no ledger at any scope (every production site wires a
// `NullSink`) — the full accounting lives in `ScopeDisclosure.tsx`, whose
// text now surfaces through the explorer's "Coverage & gaps" popover (the
// v3 "honesty on demand" affordance) rather than an always-visible
// paragraph here.
//
// The page is deliberately just a header — the entire surface is
// `NetflowExplorer`, the SAME component the project and run Network tabs
// render; scope comes from the route and only narrows the data.

import NetflowExplorer from '../components/netflow/explorer/NetflowExplorer';

export default function Netflow() {
  return (
    <div className="p-8">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Network</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Every flow recorded across all runs — same views at every scope; the scope only
          narrows the data.
        </p>
      </header>
      <NetflowExplorer scope="global" />
    </div>
  );
}
