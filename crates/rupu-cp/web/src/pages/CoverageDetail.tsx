// Per-target coverage detail — a tabbed shell (Overview | Catalog | Audit |
// Gap). Overview shows findings, files touched, and the assertion grid; the
// other tabs lazily fetch their own endpoints. Routes: /coverage/:target and
// /coverage/:target/{catalog,audit,gap}. The ?ws_id= scope is preserved across
// tabs.

import { useEffect, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { ArrowLeft, ShieldCheck, ShieldOff } from 'lucide-react';
import CoverageCatalogTab from '../components/coverage/CoverageCatalogTab';
import CoverageAuditTab from '../components/coverage/CoverageAuditTab';
import CoverageGapTab from '../components/coverage/CoverageGapTab';
import CoverageDiffTab from '../components/coverage/CoverageDiffTab';
import {
  api,
  normAssertionStatus,
  normFindingSeverity,
  sevRank,
  type AssertionStatus,
  type ConcernAssertion,
  type CoverageDetail,
  type FileView,
  type FindingsSummary,
} from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { FindingsTable } from '../components/findings/FindingsTable';
import { FindingMetrics } from '../components/findings/FindingMetrics';
import { cn } from '../lib/cn';
import { relativeTime } from '../lib/time';

// How many assertion rows to render before capping with a "+N more" note.
const ASSERTION_CAP = 200;

export type CoverageTab = 'overview' | 'catalog' | 'audit' | 'gap' | 'diff';

export default function CoverageDetail({ tab = 'overview' }: { tab?: CoverageTab }) {
  const { target = '' } = useParams<{ target: string }>();
  const [searchParams] = useSearchParams();
  const wsId = searchParams.get('ws_id') ?? undefined;
  const navigate = useNavigate();
  const enc = encodeURIComponent(target);
  const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';

  const [detail, setDetail] = useState<CoverageDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!target) return;
    let cancelled = false;
    setDetail(null);
    setError(null);
    api
      .getCoverageDetail(target, wsId)
      .then((data) => {
        if (cancelled) return;
        setDetail(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load coverage detail');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  if (error) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      </div>
    );
  }

  if (!detail) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 text-sm text-ink-dim">Loading…</div>
      </div>
    );
  }

  // Sort findings critical → info.
  const sortedFindings = detail.findings.slice().sort((a, b) => {
    return sevRank(normFindingSeverity(a.severity)) - sevRank(normFindingSeverity(b.severity));
  });

  // Severity rollup for the metric-tile strip.
  const findingsSummary: FindingsSummary = {
    total: sortedFindings.length,
    critical: 0,
    high: 0,
    medium: 0,
    low: 0,
    info: 0,
  };
  for (const f of sortedFindings) {
    findingsSummary[normFindingSeverity(f.severity)]++;
  }

  // Assertion status counts.
  const statusCounts: Record<AssertionStatus, number> = {
    clean: 0,
    finding: 0,
    examined: 0,
    not_applicable: 0,
    unknown: 0,
  };
  for (const a of detail.assertions) {
    statusCounts[normAssertionStatus(a.status)]++;
  }

  const visibleAssertions = detail.assertions.slice(0, ASSERTION_CAP);
  const hiddenCount = detail.assertions.length - visibleAssertions.length;

  // Per-file heatmap — strongest touch last, then most-recently touched.
  const files = (detail.files ?? []).slice().sort((a, b) => {
    const r = touchRank(b.strongest) - touchRank(a.strongest);
    if (r !== 0) return r;
    return (b.last_at ?? '').localeCompare(a.last_at ?? '');
  });

  return (
    <div className="p-8">
      <BackLink />

      {/* Header */}
      <header className="mt-3 flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="text-2xl font-semibold text-ink break-all">{detail.target_id}</h1>
            <CatalogBadge present={detail.has_catalog} />
          </div>
          <div className="mt-1 flex flex-wrap gap-x-4 gap-y-0.5 text-ui text-ink-dim">
            <span>{detail.assertions.length} assertion{detail.assertions.length !== 1 ? 's' : ''}</span>
            <span>{sortedFindings.length} finding{sortedFindings.length !== 1 ? 's' : ''}</span>
            <span>{files.length} file{files.length !== 1 ? 's' : ''} touched</span>
          </div>
        </div>
      </header>

      {/* Tab bar */}
      <nav className="mt-4 flex gap-1 border-b border-border">
        {(
          [
            { id: 'overview', label: 'Overview', path: `/coverage/${enc}${qs}` },
            { id: 'catalog', label: 'Catalog', path: `/coverage/${enc}/catalog${qs}` },
            { id: 'audit', label: 'Audit', path: `/coverage/${enc}/audit${qs}` },
            { id: 'gap', label: 'Gap', path: `/coverage/${enc}/gap${qs}` },
            { id: 'diff', label: 'Diff', path: `/coverage/${enc}/diff${qs}` },
          ] as { id: CoverageTab; label: string; path: string }[]
        ).map((t) => (
          <button
            key={t.id}
            onClick={() => navigate(t.path)}
            className={cn(
              'px-3 py-1.5 text-sm font-medium border-b-2 -mb-px',
              tab === t.id
                ? 'border-brand-500 text-ink'
                : 'border-transparent text-ink-dim hover:text-ink',
            )}
          >
            {t.label}
          </button>
        ))}
      </nav>

      {tab === 'catalog' && <CoverageCatalogTab target={target} wsId={wsId} />}
      {tab === 'audit' && <CoverageAuditTab target={target} wsId={wsId} />}
      {tab === 'gap' && <CoverageGapTab target={target} wsId={wsId} />}
      {tab === 'diff' && <CoverageDiffTab target={target} wsId={wsId} />}

      {tab === 'overview' && (
      <>
      {/* ── Findings ────────────────────────────────────────────── */}
      <section className="mt-8">
        <SectionHeader
          tone={sortedFindings.length > 0 ? 'bad' : 'muted'}
          label="Findings"
          count={sortedFindings.length}
          hint="sorted critical → info"
        />
        {sortedFindings.length === 0 ? (
          <p className="text-sm text-ink-dim pl-1 mt-1">No findings.</p>
        ) : (
          <>
            <div className="mb-3">
              <FindingMetrics summary={findingsSummary} />
            </div>
            <FindingsTable findings={sortedFindings} />
          </>
        )}
      </section>

      {/* ── Files (touch heatmap) ───────────────────────────────── */}
      <section className="mt-8">
        <SectionHeader
          tone="muted"
          label="Files touched"
          count={files.length}
          hint="strongest touch first"
        />
        {files.length === 0 ? (
          <p className="text-sm text-ink-dim pl-1 mt-1">No file activity recorded.</p>
        ) : (
          <FilesTable files={files} />
        )}
      </section>

      {/* ── Coverage ────────────────────────────────────────────── */}
      <section className="mt-8">
        <SectionHeader
          tone="muted"
          label="Assessed concerns"
          count={detail.assertions.length}
        />

        {/* Status summary row */}
        {detail.assertions.length > 0 && (
          <div className="flex flex-wrap gap-3 mb-3 pl-1">
            <StatusCount label="clean"          count={statusCounts.clean}          status="clean" />
            <StatusCount label="finding"        count={statusCounts.finding}        status="finding" />
            <StatusCount label="examined"       count={statusCounts.examined}       status="examined" />
            <StatusCount label="not applicable" count={statusCounts.not_applicable} status="not_applicable" />
            {statusCounts.unknown > 0 && (
              <StatusCount label="unknown" count={statusCounts.unknown} status="unknown" />
            )}
          </div>
        )}

        {detail.assertions.length === 0 ? (
          <p className="text-sm text-ink-dim pl-1 mt-1">No assertions recorded.</p>
        ) : (
          <>
            <AssertionsTable assertions={visibleAssertions} />
            {hiddenCount > 0 && (
              <p className="mt-2 text-note text-ink-mute pl-1">
                +{hiddenCount} more assertion{hiddenCount !== 1 ? 's' : ''} not shown
              </p>
            )}
          </>
        )}
      </section>
      </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// File heatmap row
// ---------------------------------------------------------------------------

// Touch strength, strongest last — mirrors rupu-coverage's `TouchStrength`.
const TOUCH_ORDER = ['glob', 'cmd', 'grep', 'read', 'edit'] as const;

function touchRank(raw: string): number {
  const i = TOUCH_ORDER.indexOf(raw.toLowerCase() as (typeof TOUCH_ORDER)[number]);
  return i < 0 ? 0 : i;
}

// Heatmap palette: hotter (edit) → cooler (glob). STATIC classes only.
const TOUCH_STYLES: Record<(typeof TOUCH_ORDER)[number], { pill: string; bar: string; label: string }> = {
  edit: { pill: 'bg-red-50 text-red-700 ring-red-200',       bar: 'bg-red-400',    label: 'edit' },
  read: { pill: 'bg-orange-50 text-orange-700 ring-orange-200', bar: 'bg-orange-300', label: 'read' },
  grep: { pill: 'bg-yellow-50 text-yellow-700 ring-yellow-200', bar: 'bg-yellow-300', label: 'grep' },
  cmd:  { pill: 'bg-blue-50 text-blue-700 ring-blue-200',     bar: 'bg-blue-300',   label: 'cmd' },
  glob: { pill: 'bg-slate-100 text-ink-mute ring-slate-200',  bar: 'bg-slate-300',  label: 'glob' },
};

/** Whether a file carries a given touch mode (presence, not a count — the
 *  per-file ledger only exposes counts for edits / reads / greps). */
function hasMode(file: FileView, mode: string): boolean {
  if (file.strongest?.toLowerCase() === mode) return true;
  return (file.touch_modes ?? []).some((m) => m.toLowerCase() === mode);
}

function PresenceCell({ on }: { on: boolean }) {
  return on ? (
    <span className="tabular-nums text-ink">•</span>
  ) : (
    <span className="text-ink-mute">—</span>
  );
}

/**
 * Files-touched heatmap as a SortableTable. Columns: File (swatch + path) |
 * Touch type | Edits | Reads | Greps | Cmds | Globs | Last updated. Sortable on
 * File / Touch type / Edits / Last updated; default order is the
 * strongest-touch-first pre-sort. Cmds / Globs are presence indicators (the
 * file ledger only counts edits / reads / greps).
 */
function FilesTable({ files }: { files: FileView[] }) {
  const columns: Column<FileView>[] = [
    {
      key: 'path',
      header: 'File',
      sortable: true,
      sortValue: (f) => f.path,
      render: (f) => {
        const t = TOUCH_STYLES[TOUCH_ORDER[touchRank(f.strongest)]];
        return (
          <span className="flex items-center gap-2 min-w-0">
            <span className={cn('shrink-0 w-1.5 h-3.5 rounded-full', t.bar)} aria-hidden />
            <span className="font-mono text-ink truncate">{f.path}</span>
          </span>
        );
      },
    },
    {
      key: 'touch',
      header: 'Touch type',
      width: 'w-28',
      sortable: true,
      sortValue: (f) => touchRank(f.strongest),
      render: (f) => {
        const t = TOUCH_STYLES[TOUCH_ORDER[touchRank(f.strongest)]];
        return (
          <span
            className={cn(
              'inline-flex items-center rounded px-2 py-0.5 text-note font-medium uppercase tracking-wide ring-1',
              t.pill,
            )}
          >
            {t.label}
          </span>
        );
      },
    },
    {
      key: 'edits',
      header: 'Edits',
      align: 'right',
      width: 'w-20',
      sortable: true,
      sortValue: (f) => f.edits,
      render: (f) => f.edits,
    },
    {
      key: 'reads',
      header: 'Reads',
      align: 'right',
      width: 'w-20',
      render: (f) => f.read_lines.length,
    },
    {
      key: 'greps',
      header: 'Greps',
      align: 'right',
      width: 'w-20',
      render: (f) => f.grep_matches,
    },
    {
      key: 'cmds',
      header: 'Cmds',
      align: 'right',
      width: 'w-16',
      render: (f) => <PresenceCell on={hasMode(f, 'cmd')} />,
    },
    {
      key: 'globs',
      header: 'Globs',
      align: 'right',
      width: 'w-16',
      render: (f) => <PresenceCell on={hasMode(f, 'glob')} />,
    },
    {
      key: 'last',
      header: 'Last updated',
      align: 'right',
      width: 'w-32',
      sortable: true,
      sortValue: (f) => f.last_at ?? null,
      render: (f) => <span className="text-ink-mute">{relativeTime(f.last_at)}</span>,
    },
  ];

  return (
    <SortableTable<FileView> columns={columns} rows={files} rowKey={(f) => f.path} />
  );
}

// ---------------------------------------------------------------------------
// Assertion row
// ---------------------------------------------------------------------------

const ASSERTION_STATUS_STYLES: Record<AssertionStatus, { pill: string; label: string }> = {
  clean:          { pill: 'bg-green-50 text-green-700 ring-green-200',   label: 'clean' },
  finding:        { pill: 'bg-orange-50 text-sev-medium ring-orange-200', label: 'finding' },
  examined:       { pill: 'bg-blue-50 text-blue-700 ring-blue-200',      label: 'examined' },
  not_applicable: { pill: 'bg-slate-100 text-ink-mute ring-slate-200',   label: 'n/a' },
  unknown:        { pill: 'bg-slate-100 text-ink-mute ring-slate-200',   label: '?' },
};

/** First line of an assertion's evidence ranges, for the `File:Line` cell. */
function assertionLine(a: ConcernAssertion): number | null {
  const first = a.evidence?.line_ranges?.[0];
  return first && first.length > 0 ? first[0] : null;
}

/**
 * Assessed concerns as a SortableTable. Columns: Concern | File:Line | Status.
 * All three are sortable. Rows are pre-capped (+N note rendered by the caller).
 */
// concern_id + file_path is not unique (the same concern can be asserted on a
// file across several line ranges), so each row carries a positional id.
type AssertionRowT = ConcernAssertion & { _key: string };

function AssertionsTable({ assertions }: { assertions: ConcernAssertion[] }) {
  const rows: AssertionRowT[] = assertions.map((a, i) => ({
    ...a,
    _key: `${a.concern_id}-${a.file_path}-${i}`,
  }));

  const columns: Column<AssertionRowT>[] = [
    {
      key: 'concern',
      header: 'Concern',
      sortable: true,
      sortValue: (a) => a.concern_id,
      render: (a) => <span className="font-mono text-note text-ink break-all">{a.concern_id}</span>,
    },
    {
      key: 'location',
      header: 'File:Line',
      sortable: true,
      sortValue: (a) => a.file_path,
      render: (a) => {
        const line = assertionLine(a);
        return (
          <span className="font-mono text-note text-ink-mute break-all">
            {a.file_path}
            {line !== null && `:${line}`}
          </span>
        );
      },
    },
    {
      key: 'status',
      header: 'Status',
      width: 'w-28',
      sortable: true,
      sortValue: (a) => normAssertionStatus(a.status),
      render: (a) => {
        const s = ASSERTION_STATUS_STYLES[normAssertionStatus(a.status)];
        return (
          <span
            className={cn(
              'inline-flex items-center rounded px-2 py-0.5 text-note font-medium ring-1',
              s.pill,
            )}
          >
            {s.label}
          </span>
        );
      },
    },
  ];

  return (
    <SortableTable<AssertionRowT>
      columns={columns}
      rows={rows}
      rowKey={(a) => a._key}
      renderDetail={(a) => (
        <p className="text-note text-ink-dim whitespace-pre-wrap break-words">
          {a.evidence.summary?.trim() ? a.evidence.summary : 'No evidence summary recorded.'}
        </p>
      )}
    />
  );
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

function CatalogBadge({ present }: { present: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded px-2 py-0.5 text-note font-medium ring-1',
        present
          ? 'bg-green-50 text-green-700 ring-green-200'
          : 'bg-slate-100 text-ink-mute ring-slate-200',
      )}
    >
      {present ? <ShieldCheck size={11} /> : <ShieldOff size={11} />}
      {present ? 'catalog' : 'no catalog'}
    </span>
  );
}

function StatusCount({
  label,
  count,
  status,
}: {
  label: string;
  count: number;
  status: AssertionStatus;
}) {
  if (count === 0) return null;
  const s = ASSERTION_STATUS_STYLES[status];
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded px-2 py-0.5 text-note font-medium ring-1',
        s.pill,
      )}
    >
      <span className="tabular-nums">{count}</span>
      <span>{label}</span>
    </span>
  );
}

function BackLink() {
  return (
    <Link
      to="/coverage"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Coverage
    </Link>
  );
}
