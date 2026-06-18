// Per-target coverage detail — findings first (high-value), then the full
// assertion grid (what was assessed). Route: /coverage/:target

import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft, ShieldCheck, ShieldOff } from 'lucide-react';
import {
  api,
  normAssertionStatus,
  normFindingSeverity,
  sevRank,
  type AssertionStatus,
  type ConcernAssertion,
  type CoverageDetail,
  type FindingRecord,
  type FindingSeverity,
} from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import { cn } from '../lib/cn';

// How many assertion rows to render before capping with a "+N more" note.
const ASSERTION_CAP = 200;

export default function CoverageDetail() {
  const { target = '' } = useParams<{ target: string }>();

  const [detail, setDetail] = useState<CoverageDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!target) return;
    let cancelled = false;
    setDetail(null);
    setError(null);
    api
      .getCoverageDetail(target)
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
  }, [target]);

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

  return (
    <div className="p-8 max-w-5xl">
      <BackLink />

      {/* Header */}
      <header className="mt-3 flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="text-2xl font-semibold text-ink break-all">{detail.target_id}</h1>
            <CatalogBadge present={detail.has_catalog} />
          </div>
          <div className="mt-1 flex flex-wrap gap-x-4 gap-y-0.5 text-[12px] text-ink-dim">
            <span>{detail.assertions.length} assertion{detail.assertions.length !== 1 ? 's' : ''}</span>
            <span>{sortedFindings.length} finding{sortedFindings.length !== 1 ? 's' : ''}</span>
          </div>
        </div>
      </header>

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
          <ListCard>
            {sortedFindings.map((f) => (
              <FindingRow key={f.id} finding={f} />
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
              <p className="mt-2 text-[11px] text-ink-mute pl-1">
                +{hiddenCount} more assertion{hiddenCount !== 1 ? 's' : ''} not shown
              </p>
            )}
          </>
        )}
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Findings row
// ---------------------------------------------------------------------------

const SEV_STYLES: Record<FindingSeverity, { pill: string; label: string }> = {
  critical: { pill: 'bg-purple-50 text-sev-critical ring-purple-200', label: 'critical' },
  high:     { pill: 'bg-red-50 text-sev-high ring-red-200',           label: 'high' },
  medium:   { pill: 'bg-orange-50 text-sev-medium ring-orange-200',   label: 'medium' },
  low:      { pill: 'bg-yellow-50 text-sev-low ring-yellow-200',      label: 'low' },
  info:     { pill: 'bg-slate-100 text-sev-info ring-slate-200',      label: 'info' },
};

function FindingRow({ finding }: { finding: FindingRecord }) {
  const sev = normFindingSeverity(finding.severity);
  const s = SEV_STYLES[sev];

  const locationParts: string[] = [];
  if (finding.file_path) locationParts.push(finding.file_path);
  if (finding.line_range) locationParts.push(`${finding.line_range[0]}–${finding.line_range[1]}`);
  const location = locationParts.join(':');

  return (
    <div className="flex items-start gap-3 px-4 py-3">
      {/* Severity badge */}
      <span
        className={cn(
          'shrink-0 inline-flex items-center rounded px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide ring-1 mt-0.5',
          s.pill,
        )}
      >
        {s.label}
      </span>

      {/* Body */}
      <div className="min-w-0 flex-1">
        <p className="text-sm text-ink leading-snug">{finding.summary}</p>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-3 text-[11px] text-ink-mute">
          {location && <span className="font-mono break-all">{location}</span>}
          {finding.concern_id && (
            <span>
              concern <span className="font-mono">{finding.concern_id}</span>
            </span>
          )}
        </div>
      </div>
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
          'shrink-0 inline-flex items-center rounded px-2 py-0.5 text-[11px] font-medium ring-1 mt-0.5',
          s.pill,
        )}
      >
        {s.label}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-3 text-[11px] text-ink-mute mb-0.5">
          <span className="font-mono break-all">{assertion.file_path}</span>
          <span className="font-mono text-ink-mute/70">{assertion.concern_id}</span>
        </div>
        {truncated && <p className="text-[12px] text-ink-dim leading-snug">{truncated}</p>}
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
        'inline-flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium ring-1',
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
        'inline-flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium ring-1',
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
