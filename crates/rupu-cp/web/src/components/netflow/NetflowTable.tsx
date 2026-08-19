// Per-flow netflow table — the primary Netflow-tab view for a run, project,
// or the global scope. Built on the same `SortableTable` + `EmptyState`
// primitives as `findings/FindingsTable.tsx` rather than a hand-rolled
// `<table>`, so markup, sorting affordance and empty-state look match the
// rest of the CP.
//
// Every honesty rule this subsystem exists to enforce lives here:
//   - `formatBytes` (Task 4) renders `null`/`undefined` as an em dash, never
//     `0 B` — an unobserved byte count must never read as a real zero.
//   - The fidelity badge is unconditional: every row gets one, so a Coarse
//     row never LOOKS as complete as an HTTP row.
//   - `droppedTotal > 0` gets a loud banner, explicitly scoped ("full
//     history", across every ledger this view reads) rather than any
//     active date filter — the list is silently incomplete without it,
//     which is the exact defect this subsystem prevents. This banner
//     renders even when
//     `flows` is empty (an all-dropped scope): the empty state and the
//     loss banner are not mutually exclusive, and showing only the empty
//     state there would be the same silent-loss defect in its worst form.
//   - `asnLoaded === false` gets its own note: a blank Network column would
//     read as "this peer has no ASN", not "enrichment wasn't available".
//   - The empty state states netflow's scope limit (rupu's own egress, not
//     the agent's `bash` subprocess) so an empty table doesn't imply "no
//     network activity happened".

import { formatBytes, type FlowView } from '../../lib/netflow';
import SortableTable, { type Column } from '../lists/SortableTable';
import { EmptyState } from '../ui/EmptyState';
import { FidelityBadge } from './FidelityBadge';
import {
  netflowEmptyStateHint,
  netflowRangeEmptyHint,
  netflowWindowApplied,
  type NetflowScope,
  type NetflowWindowEcho,
} from './ScopeDisclosure';

export interface NetflowTableProps {
  flows: FlowView[];
  /** Records lost to writer overflow, for the WHOLE ledger — never scoped
   *  to whatever date range (if any) produced `flows`. Named to match the
   *  wire field (`NetflowResponse.dropped_total`) rather than a bare
   *  `dropped`, and the banner text below echoes the same "whole history"
   *  scope, so this number can't misread as "lost within the current
   *  view" the moment a date-range picker sits above this table (Task 4).
   *  Non-zero means this list is incomplete — surfaced as a banner, never
   *  silently absorbed. */
  droppedTotal: number;
  /** `false` means ASN enrichment was unavailable, not that flows lack an
   *  ASN. Drives an explanatory note rather than a blank Network column. */
  asnLoaded: boolean;
  /** Selects the empty-state hint (`netflowEmptyStateHint`) — project
   *  scope gets a caveat the other two don't need. Defaults to `'run'`
   *  (the generic hint) so existing callers that don't pass this keep
   *  their prior behaviour exactly. */
  scope?: NetflowScope;
  /** `NetflowResponse.window` — the server's OWN echo of the `?from=`/
   *  `?to=` it actually applied, NOT the picker's local UI state (Task 4).
   *  Used ONLY to pick which empty-state copy is honest: "nothing in this
   *  range" (a bound was applied and matched nothing — flows may still
   *  exist outside it) vs "nothing recorded at all" (no bound was applied,
   *  so an empty `flows` really does mean the whole scope is empty). Optional
   *  and defaulting to "no window" so every pre-Task-4 caller that doesn't
   *  pass it keeps the exact prior wording. Named `appliedWindow` (not
   *  `window`) purely to avoid shadowing the global `window` object inside
   *  this component. */
  appliedWindow?: NetflowWindowEcho;
}

function originLabel(f: FlowView): string {
  return f.ctx.origin.name ?? f.ctx.origin.kind;
}

/**
 * `droppedTotal > 0` banner — factored out so it renders identically
 * whether the surviving-flows list is empty or not. Fix round 1: this used
 * to live only in the non-empty return path, so a scope where EVERY flow
 * was dropped rendered a bare "No network flows recorded" with zero
 * indication that anything was lost — the exact silent-incompleteness
 * defect this banner exists to prevent, reachable in the one case
 * (all-dropped) where it matters most. The empty state and the loss banner
 * are not mutually exclusive; the loss is the more important of the two, so
 * it renders first.
 *
 * Netflow Plan 3 Task 3, review round 1: the copy explicitly says "full
 * history" rather than just "N flows dropped" — `droppedTotal` is the whole
 * loss regardless of any date-range filter applied to `flows`
 * (`NetflowResponse.dropped_total`'s doc comment), and once Task 4 puts a
 * date picker above this table, an unqualified count would read as
 * "dropped within the selected range", which is exactly the silent-gap
 * misreading this whole subsystem exists to prevent.
 *
 * Deliberately says "every ledger this view reads", not "this ledger" (a
 * wording this banner used to carry): even at run scope, `droppedTotal` is
 * already a sum across more than one file — this run's own ledger, plus
 * any dispatched step's, plus any sub-agent's, at any dispatch depth (see
 * `rupu-cp::api::netflow::run_and_unit_ids`) — and project/global scope
 * sum across every contributing run's ledger on top of that. Naming a
 * single ledger would have undersold how wide "incomplete" can actually
 * be.
 */
function DroppedBanner({ droppedTotal }: { droppedTotal: number }) {
  if (droppedTotal <= 0) return null;
  return (
    <div
      role="status"
      className="rounded-lg border border-warn/30 bg-warn-bg px-4 py-2 text-sm text-warn"
    >
      <span className="font-medium">{droppedTotal} flows dropped</span> across the full
      history of every ledger this view reads — the capture buffer overflowed at some
      point, so this list may be incomplete regardless of any date range shown here.
    </div>
  );
}

export function NetflowTable({
  flows,
  droppedTotal,
  asnLoaded,
  scope = 'run',
  appliedWindow,
}: NetflowTableProps) {
  if (flows.length === 0) {
    // A bound is "applied" per the SERVER's echo, not per whatever the
    // picker's local value happens to be — see this prop's doc comment and
    // `netflowWindowApplied`'s.
    const windowApplied = netflowWindowApplied(appliedWindow);
    return (
      <div className="space-y-3">
        <DroppedBanner droppedTotal={droppedTotal} />
        {windowApplied ? (
          <EmptyState
            title="No network flows in this range"
            hint={
              <>
                {netflowRangeEmptyHint()} {netflowEmptyStateHint(scope)}
              </>
            }
          />
        ) : (
          <EmptyState
            title="No network flows recorded for this scope"
            hint={netflowEmptyStateHint(scope)}
          />
        )}
      </div>
    );
  }

  const columns: Column<FlowView>[] = [
    {
      key: 'ts',
      header: 'Time',
      fit: true,
      sortable: true,
      sortValue: (f) => f.ts,
      render: (f) => new Date(f.ts).toLocaleTimeString(),
    },
    {
      key: 'origin',
      header: 'Origin',
      fit: true,
      render: (f) => <span className="text-ink-dim">{originLabel(f)}</span>,
    },
    {
      key: 'host',
      header: 'Host',
      fit: true,
      sortable: true,
      sortValue: (f) => f.host,
      render: (f) => <span className="font-mono text-note">{f.host}</span>,
    },
    {
      key: 'path',
      header: 'Path',
      subject: true,
      titleValue: (f) => `${f.method} ${f.path}`,
      render: (f) => <span className="font-mono text-note text-ink-dim">{f.path}</span>,
    },
    {
      key: 'network',
      header: 'Network',
      fit: true,
      render: (f) =>
        f.asn ? (
          <span className="font-mono text-note text-ink-mute">
            AS{f.asn.asn} {f.asn.org}
          </span>
        ) : (
          <span className="text-ink-mute">—</span>
        ),
    },
    {
      key: 'status',
      header: 'Status',
      fit: true,
      align: 'right',
      render: (f) => (
        <span className={f.outcome === 'ok' ? 'text-ink-dim' : 'text-err'}>
          {f.status ?? '—'}
        </span>
      ),
    },
    {
      key: 'bytes_in',
      header: 'In',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (f) => f.bytes_in ?? null,
      render: (f) => formatBytes(f.bytes_in),
    },
    {
      key: 'bytes_out',
      header: 'Out',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (f) => f.bytes_out ?? null,
      render: (f) => formatBytes(f.bytes_out),
    },
    {
      key: 'duration_ms',
      header: 'Duration',
      fit: true,
      align: 'right',
      sortable: true,
      sortValue: (f) => f.duration_ms ?? null,
      render: (f) => (f.duration_ms != null ? `${f.duration_ms} ms` : '—'),
    },
    {
      key: 'fidelity',
      header: 'Fidelity',
      fit: true,
      render: (f) => <FidelityBadge fidelity={f.fidelity} />,
    },
  ];

  return (
    <div className="space-y-3">
      <DroppedBanner droppedTotal={droppedTotal} />
      {!asnLoaded && (
        <p className="text-note text-ink-mute">
          ASN data not loaded — network enrichment will appear once the table has been fetched.
        </p>
      )}
      <SortableTable<FlowView>
        columns={columns}
        rows={flows}
        rowKey={(f) => f.id}
        initialSort={{ key: 'ts', dir: 'desc' }}
      />
    </div>
  );
}

export default NetflowTable;
