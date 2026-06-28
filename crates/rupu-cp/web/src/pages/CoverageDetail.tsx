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
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import { FindingRow } from '../components/findings/FindingRow';
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
            <ListCard>
              {sortedFindings.map((f) => (
                <FindingRow key={f.id} finding={f} />
              ))}
            </ListCard>
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
          <ListCard>
            {files.map((f) => (
              <FileRow key={f.path} file={f} />
            ))}
          </ListCard>
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
            <ListCard>
              {visibleAssertions.map((a, i) => (
                <AssertionRow key={`${a.concern_id}-${a.file_path}-${i}`} assertion={a} />
              ))}
            </ListCard>
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

function FileRow({ file }: { file: FileView }) {
  const key = TOUCH_ORDER[touchRank(file.strongest)];
  const t = TOUCH_STYLES[key];

  return (
    <div className="flex items-center gap-3 px-4 py-2.5">
      {/* heatmap accent bar, colored by strongest touch */}
      <span className={cn('shrink-0 w-1 self-stretch rounded-full', t.bar)} aria-hidden />
      <div className="min-w-0 flex-1">
        <p className="text-lead font-mono text-ink truncate">{file.path}</p>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-3 text-note text-ink-mute">
          {file.edits > 0 && <span>{file.edits} edit{file.edits !== 1 ? 's' : ''}</span>}
          {file.grep_matches > 0 && <span>{file.grep_matches} grep match{file.grep_matches !== 1 ? 'es' : ''}</span>}
          {file.read_lines.length > 0 && (
            <span>{file.read_lines.length} read range{file.read_lines.length !== 1 ? 's' : ''}</span>
          )}
          <span>{relativeTime(file.last_at)}</span>
        </div>
      </div>
      <span
        className={cn(
          'shrink-0 inline-flex items-center rounded px-2 py-0.5 text-note font-medium uppercase tracking-wide ring-1',
          t.pill,
        )}
      >
        {t.label}
      </span>
    </div>
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

function AssertionRow({ assertion }: { assertion: ConcernAssertion }) {
  const status = normAssertionStatus(assertion.status);
  const s = ASSERTION_STATUS_STYLES[status];

  const summary = assertion.evidence?.summary ?? '';
  const truncated = summary.length > 160 ? summary.slice(0, 157) + '…' : summary;

  return (
    <div className="flex items-start gap-3 px-4 py-3">
      <span
        className={cn(
          'shrink-0 inline-flex items-center rounded px-2 py-0.5 text-note font-medium ring-1 mt-0.5',
          s.pill,
        )}
      >
        {s.label}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-3 text-note text-ink-mute mb-0.5">
          <span className="font-mono break-all">{assertion.file_path}</span>
          <span className="font-mono text-ink-mute/70">{assertion.concern_id}</span>
        </div>
        {truncated && <p className="text-ui text-ink-dim leading-snug">{truncated}</p>}
      </div>
    </div>
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
