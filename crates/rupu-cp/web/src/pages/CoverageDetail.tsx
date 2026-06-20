// Per-target coverage detail — findings first (high-value), then the full
// assertion grid (what was assessed). Route: /coverage/:target

import { useEffect, useState } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';
import { ArrowLeft, ChevronDown, ChevronRight, ShieldCheck, ShieldOff } from 'lucide-react';
import {
  api,
  normAssertionStatus,
  normFindingSeverity,
  sevRank,
  type AssertionStatus,
  type ConcernAssertion,
  type CoverageDetail,
  type FileView,
  type FindingRecord,
} from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import { cn } from '../lib/cn';
import { relativeTime } from '../lib/time';
import { SEVERITY_STYLE } from '../lib/severity';

// How many assertion rows to render before capping with a "+N more" note.
const ASSERTION_CAP = 200;

export default function CoverageDetail() {
  const { target = '' } = useParams<{ target: string }>();
  const [searchParams] = useSearchParams();
  const wsId = searchParams.get('ws_id') ?? undefined;

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
            <span>{files.length} file{files.length !== 1 ? 's' : ''} touched</span>
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

function FindingRow({ finding }: { finding: FindingRecord }) {
  const sev = normFindingSeverity(finding.severity);
  const s = SEVERITY_STYLE[sev];
  const [open, setOpen] = useState(false);

  const locationParts: string[] = [];
  if (finding.file_path) locationParts.push(finding.file_path);
  if (finding.line_range) locationParts.push(`${finding.line_range[0]}–${finding.line_range[1]}`);
  const location = locationParts.join(':');

  const rationale = finding.evidence?.rationale ?? '';
  const excerpt = finding.evidence?.code_excerpt ?? '';
  const references = finding.evidence?.references ?? [];
  const hasEvidence = Boolean(rationale || excerpt || references.length > 0);

  return (
    <div className="px-4 py-3">
      <div className="flex items-start gap-3">
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
            {hasEvidence && (
              <button
                type="button"
                onClick={() => setOpen((v) => !v)}
                className="inline-flex items-center gap-0.5 text-ink-dim hover:text-ink"
              >
                {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                evidence
              </button>
            )}
          </div>

          {hasEvidence && open && (
            <div className="mt-2 space-y-2">
              {rationale && (
                <p className="text-[12px] text-ink-dim leading-snug whitespace-pre-wrap">
                  {rationale}
                </p>
              )}
              {excerpt && (
                <pre className="overflow-x-auto rounded bg-slate-50 ring-1 ring-slate-200 px-3 py-2 text-[11px] font-mono text-ink leading-snug whitespace-pre">
                  {excerpt}
                </pre>
              )}
              {references.length > 0 && (
                <ul className="list-disc pl-4 text-[11px] text-ink-mute space-y-0.5">
                  {references.map((ref, i) => (
                    <li key={i} className="break-all font-mono">{ref}</li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      </div>
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
        <p className="text-[13px] font-mono text-ink truncate">{file.path}</p>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-3 text-[11px] text-ink-mute">
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
          'shrink-0 inline-flex items-center rounded px-2 py-0.5 text-[11px] font-medium uppercase tracking-wide ring-1',
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
