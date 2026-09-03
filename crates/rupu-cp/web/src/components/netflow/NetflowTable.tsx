// Per-flow netflow table — the primary Netflow-tab view for a run, project,
// or the global scope. Built on the same `SortableTable` + `EmptyState`
// primitives as `findings/FindingsTable.tsx` rather than a hand-rolled
// `<table>`, so markup, sorting affordance and empty-state look match the
// rest of the CP.
//
// Every honesty rule this subsystem exists to enforce lives here:
//   - `formatBytes` (Task 4) renders `null`/`undefined` as an em dash, never
//     `0 B` — an unobserved byte count must never read as a real zero.
//   - Fidelity moved from a per-row column into the flow detail panel and
//     the explorer's CoveragePopover (the v3 redesign's "honesty on
//     demand" decision) — a row's dashes still read honestly because the
//     detail panel a click away explains them, and the coverage popover
//     carries the legend on every scope.
//   - `droppedTotal > 0` on an EMPTY flows list still gets the loud
//     banner: an all-dropped scope showing only "no flows recorded" would
//     be the silent-loss defect in its worst form. For non-empty lists
//     the same sentence (single-sourced: `droppedTotalSentence`) lives in
//     the CoveragePopover instead of a permanent banner.
//   - `asnLoaded === false` gets its own note: a blank Network column would
//     read as "this peer has no ASN", not "enrichment wasn't available".
//   - The empty state states netflow's scope limit (rupu's own egress, not
//     the agent's `bash` subprocess) so an empty table doesn't imply "no
//     network activity happened".

import { formatBytes, type FlowView, type IncompleteSource } from '../../lib/netflow';
import SortableTable, { type Column } from '../lists/SortableTable';
import { EmptyState } from '../ui/EmptyState';
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
  /** Sources that own part of this scope's traffic but could not be read
   *  (`NetflowResponse.incomplete`). Non-empty means this list is short by
   *  an unknown amount — a run placed on a remote host keeps its ledger
   *  there, so an empty local answer for one is "we could not look", not
   *  "no traffic". Rendered as a loud banner on BOTH the empty and the
   *  populated path, unlike `droppedTotal`'s honesty-on-demand popover:
   *  a whole missing source changes what the numbers shown MEAN. */
  incomplete?: IncompleteSource[];
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
  /** Show the Run + Workflow attribution columns (`FlowView.run_id`/
   *  `.workflow`, server-resolved). The explorer passes `true` at project
   *  and global scope; at run scope every row belongs to the same run, so
   *  the columns would be noise. */
  showAttribution?: boolean;
  /** Row click → the explorer's flow-detail slide-over. */
  onRowClick?: (flow: FlowView) => void;
  /** Whether any cross-filter (workflow/origin/org/host chips) is active —
   *  the THIRD cause of an empty list, with its own honest wording: a
   *  filtered-to-nothing table must not claim the scope recorded nothing. */
  filtersActive?: boolean;
}

/** Exported for the explorer's FlowDetailPanel — one authored rendering of
 *  a flow's origin, so the table and the detail panel for the same row can
 *  never disagree on what the origin is called. */
export function originLabel(f: FlowView): string {
  return f.ctx.origin.name ?? f.ctx.origin.kind;
}

/** The one authored copy of the dropped-loss sentence — the empty-state
 *  banner below and the explorer's CoveragePopover both render exactly
 *  this string, so the "whole history, every ledger this view reads"
 *  scoping can't drift between the two surfaces. */
export function droppedTotalSentence(droppedTotal: number): string {
  return (
    `${droppedTotal} flows dropped across the full history of every ledger this view ` +
    `reads — the capture buffer overflowed at some point, so this list may be ` +
    `incomplete regardless of any date range shown here.`
  );
}

/**
 * `droppedTotal > 0` banner — v3 redesign: renders ONLY on the empty-flows
 * path now. On a populated table the same sentence (single-sourced via
 * [`droppedTotalSentence`]) lives in the explorer's always-reachable
 * CoveragePopover instead of a permanent page banner — "honesty on
 * demand". The all-dropped empty case KEEPS the loud in-place banner: a
 * bare "no flows recorded" over a scope that in fact lost every record
 * would be the silent-incompleteness defect this subsystem exists to
 * prevent, in its worst form, and a popover nobody is prompted to open is
 * not loud enough for that.
 *
 * The wording still says "full history" / "every ledger this view reads"
 * for the reasons the pre-v3 banner documented: `droppedTotal` is
 * whole-history regardless of any window (a drop batch has no timestamp),
 * and even run scope sums across this run's + every dispatched step's +
 * sub-agent's ledger files (`rupu-cp::api::netflow::run_and_unit_ids`).
 */
function DroppedBanner({ droppedTotal }: { droppedTotal: number }) {
  if (droppedTotal <= 0) return null;
  return (
    <div
      role="status"
      className="rounded-lg border border-warn/30 bg-warn-bg px-4 py-2 text-sm text-warn"
    >
      {droppedTotalSentence(droppedTotal)}
    </div>
  );
}

/**
 * A source of this scope's flows that could not be read at all.
 *
 * Unlike [`DroppedBanner`], this renders on the POPULATED path too: a
 * partial list with no warning reads as a complete one, and the numbers
 * above it (counts, rollups, KPIs) are all understated by an unknown
 * amount. The reason is the remote's own words, because it names the fix —
 * an out-of-date `rupu` on that host, or a host that is simply down.
 */
function IncompleteBanner({ incomplete }: { incomplete?: IncompleteSource[] }) {
  if (!incomplete || incomplete.length === 0) return null;
  return (
    <div
      role="status"
      className="rounded-lg border border-warn/30 bg-warn-bg px-4 py-2 text-sm text-warn"
    >
      <p className="font-medium">
        Incomplete — {incomplete.length === 1 ? 'a source' : `${incomplete.length} sources`} of this
        scope&rsquo;s traffic could not be read, so the flows below are short by an unknown amount.
      </p>
      <ul className="mt-1 space-y-0.5">
        {incomplete.map((s) => (
          <li key={s.host_id} className="text-note">
            <span className="font-mono">{s.host_id}</span>: {s.reason}
          </li>
        ))}
      </ul>
    </div>
  );
}

export function NetflowTable({
  flows,
  droppedTotal,
  incomplete,
  asnLoaded,
  scope = 'run',
  appliedWindow,
  showAttribution = false,
  onRowClick,
  filtersActive = false,
}: NetflowTableProps) {
  if (flows.length === 0) {
    // A bound is "applied" per the SERVER's echo, not per whatever the
    // picker's local value happens to be — see this prop's doc comment and
    // `netflowWindowApplied`'s. Cross-filters are the third distinct cause
    // of emptiness and get their own wording FIRST: with filters active,
    // neither "nothing in this range" nor "nothing recorded" is what the
    // empty list actually means.
    const windowApplied = netflowWindowApplied(appliedWindow);
    return (
      <div className="space-y-3">
        <IncompleteBanner incomplete={incomplete} />
        <DroppedBanner droppedTotal={droppedTotal} />
        {filtersActive ? (
          <EmptyState
            title="No flows match the current filters"
            hint="None of this scope's recorded flows pass the active filter chips — remove some, or choose Clear all."
          />
        ) : windowApplied ? (
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
    // Attribution columns (non-run scopes only): server-resolved root run
    // + workflow. `—` is an honest "no run record accounts for this
    // ledger" (e.g. a standalone agent run), not missing data.
    ...(showAttribution
      ? ([
          {
            key: 'run',
            header: 'Run',
            fit: true,
            sortable: true,
            sortValue: (f) => f.run_id ?? null,
            render: (f) =>
              f.run_id ? (
                <span className="font-mono text-note text-brand-500">{f.run_id}</span>
              ) : (
                <span className="text-ink-mute">—</span>
              ),
          },
          {
            key: 'workflow',
            header: 'Workflow',
            fit: true,
            sortable: true,
            sortValue: (f) => f.workflow ?? null,
            render: (f) =>
              f.workflow ? (
                <span className="font-mono text-note text-ink-dim">{f.workflow}</span>
              ) : (
                <span className="text-ink-mute">—</span>
              ),
          },
        ] as Column<FlowView>[])
      : []),
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
  ];

  return (
    <div className="space-y-3">
      <IncompleteBanner incomplete={incomplete} />
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
        onRowClick={onRowClick}
      />
    </div>
  );
}

export default NetflowTable;
