// Shared findings list rendered as a SortableTable. Used by the global Findings
// page, the per-project Findings tab, and the coverage-detail Overview. Columns:
// Severity | Summary | Tags | Report | File:Line | CWE | Concern | Agent, plus Project |
// Target when `showProvenance` is set (the cross-project / project-scoped
// variants). Each row expands (via `renderDetail`) to its evidence panel — or,
// for a full-profile finding, to the triage card summarising its structured report.
//
// Severity sorts by rank with critical highest; Summary / File / Concern sort
// lexically. The rows arrive backend-sorted (critical → info, newest first), so
// no `initialSort` is supplied — the unsorted default preserves that order.

import { useNavigate } from 'react-router-dom';
import {
  normFindingSeverity,
  sevRank,
  type FindingOut,
  type FindingRecord,
} from '../../lib/api';
import { cn } from '../../lib/cn';
import { cweFromFinding, cweRef } from '../../lib/cwe';
import { codeHref } from '../../lib/findingReport';
import SeverityChip from '../coverage/SeverityChip';
import SortableTable, { type Column, type RowSelection } from '../lists/SortableTable';
import { AgentName } from '../codename/AgentName';
import { FindingEvidence } from './FindingEvidence';
import TriageCard from './TriageCard';
import { findingCodename } from './findingCodename';

/** Floor for the Summary (subject) column's width. */
export const SUMMARY_MIN_W = 'min-w-[16rem]';
/** Long `fit` cells (paths, ids) truncate with an ellipsis instead of forcing
 *  the column — and the table — wider; the full value rides in `title`. */
export const LONG_CELL = 'block max-w-[16rem] truncate';
/** The agent cell: a codename chip plus a label, clipped (title on the chip). */
export const AGENT_CELL = 'block max-w-[14rem] truncate';

function location(f: FindingRecord): string {
  const parts: string[] = [];
  if (f.file_path) parts.push(f.file_path);
  if (f.line_range) parts.push(`${f.line_range[0]}–${f.line_range[1]}`);
  return parts.join(':');
}

export function FindingsTable({
  findings,
  showProvenance = false,
  wsId,
  selection,
}: {
  findings: FindingRecord[];
  /** Render Project / Target columns. Only set when `findings` are `FindingOut`
   *  (they carry the provenance keys). */
  showProvenance?: boolean;
  /** Fallback owning workspace id, used when a row isn't a `FindingOut` (e.g.
   *  the coverage-detail table, whose findings are plain `FindingRecord`s
   *  already scoped to one project by the caller). Rows that ARE `FindingOut`
   *  use their own `ws_id` instead. When resolved alongside `file_path` +
   *  `line_range`, the location cell deep-links into that project's Code tab. */
  wsId?: string;
  /** Leading checkbox column (bulk tagging). */
  selection?: RowSelection<FindingRecord>;
}) {
  const navigate = useNavigate();

  const columns: Column<FindingRecord>[] = [
    {
      key: 'severity',
      header: 'Severity',
      fit: true,
      sortable: true,
      // sevRank: critical=0 … info=4, so ascending (first click) sorts
      // most-severe first — matching the backend default order + intuition.
      sortValue: (f) => sevRank(normFindingSeverity(f.severity)),
      render: (f) => <SeverityChip severity={normFindingSeverity(f.severity)} />,
    },
    {
      key: 'summary',
      header: 'Summary',
      subject: true,
      // The subject column only gets leftover width (`max-w-0`); with several
      // wide `fit` columns that was ~0px. A floor keeps it readable.
      width: SUMMARY_MIN_W,
      sortable: true,
      sortValue: (f) => f.summary,
      titleValue: (f) => f.summary,
      render: (f) => <span className="text-ink leading-snug">{f.summary}</span>,
    },
    {
      key: 'tags',
      header: 'Tags',
      fit: true,
      render: (f) => {
        const tags = f.tags ?? [];
        if (tags.length === 0) return null;
        return (
          <span className="flex items-center gap-1">
            {tags.slice(0, 2).map((t) => (
              <span
                key={t}
                title={t}
                className="max-w-[10rem] truncate rounded bg-surface px-1.5 py-0.5 font-mono text-note text-ink ring-1 ring-border"
              >
                {t}
              </span>
            ))}
            {tags.length > 2 && (
              <span title={tags.join(', ')} className="text-note text-ink-mute">
                +{tags.length - 2}
              </span>
            )}
          </span>
        );
      },
    },
    {
      key: 'report',
      header: 'Report',
      fit: true,
      sortable: true,
      // Completeness ratio; rows without a report_summary sort below every
      // full report (-1) so "summary" rows cluster together.
      sortValue: (f) =>
        f.report_summary && f.report_summary.completeness.total > 0
          ? f.report_summary.completeness.filled / f.report_summary.completeness.total
          : -1,
      render: (f) =>
        f.report_summary ? (
          <span className="font-mono text-note text-ink-dim">
            {f.report_summary.completeness.filled}/{f.report_summary.completeness.total}
            {f.report_summary.has_poc && <span className="ml-1 text-ok">PoC</span>}
          </span>
        ) : f.profile === 'full' ? (
          // A full-profile row without a report_summary carries a report this
          // build couldn't parse (e.g. written by a newer rupu).
          <span className="text-note text-ink-mute">unreadable</span>
        ) : (
          <span className="text-note text-ink-mute">summary</span>
        ),
    },
    {
      key: 'location',
      header: 'File:Line',
      fit: true,
      sortable: true,
      sortValue: (f) => f.file_path ?? null,
      render: (f) => {
        const loc = location(f);
        if (!loc) return <span className="text-ink-mute">—</span>;
        const rowWsId = (f as FindingOut).ws_id ?? wsId;
        if (f.file_path && f.line_range && rowWsId) {
          return (
            <button
              type="button"
              onClick={() => navigate(codeHref(rowWsId, f.file_path!, f.line_range![0]))}
              title={loc}
              className={cn(LONG_CELL, 'font-mono text-note text-brand-700 hover:underline')}
            >
              {loc}
            </button>
          );
        }
        return (
          <span title={loc} className={cn(LONG_CELL, 'font-mono text-note text-ink-mute')}>
            {loc}
          </span>
        );
      },
    },
    {
      key: 'cwe',
      header: 'CWE',
      fit: true,
      render: (f) => {
        const reportCwe = f.report_summary?.cwe[0];
        const cwe = (reportCwe ? cweRef(reportCwe) : null) ?? cweFromFinding(f);
        return cwe ? (
          <a
            href={cwe.url}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center rounded bg-surface px-1.5 py-0.5 text-note font-medium text-ink ring-1 ring-border hover:bg-surface-hover"
          >
            {cwe.id}
          </a>
        ) : (
          <span className="text-ink-mute">—</span>
        );
      },
    },
    {
      key: 'concern',
      header: 'Concern',
      fit: true,
      sortable: true,
      sortValue: (f) => f.concern_id ?? null,
      render: (f) =>
        f.concern_id ? (
          <span title={f.concern_id} className={cn(LONG_CELL, 'font-mono text-note text-ink-mute')}>
            {f.concern_id}
          </span>
        ) : (
          <span className="text-ink-mute">—</span>
        ),
    },
    {
      key: 'agent',
      header: 'Agent',
      fit: true,
      sortable: true,
      sortValue: (f) => findingCodename(f)?.codename ?? null,
      render: (f) => {
        const n = findingCodename(f);
        return n ? (
          <span className={cn(AGENT_CELL, 'text-note')}>
            <AgentName
              codename={n.codename}
              agent={n.agent}
              provider={n.provider}
              model={n.model}
              showCrew
              derived={n.derived}
            />
          </span>
        ) : (
          <span className="text-ink-mute">—</span>
        );
      },
    },
  ];

  if (showProvenance) {
    columns.push(
      {
        key: 'project',
        header: 'Project',
        fit: true,
        render: (f) => <span className="text-ink-dim">{(f as FindingOut).project || '—'}</span>,
      },
      {
        key: 'target',
        header: 'Target',
        fit: true,
        render: (f) => (
          <span
            title={(f as FindingOut).target_id || undefined}
            className={cn(LONG_CELL, 'font-mono text-note text-ink-mute')}
          >
            {(f as FindingOut).target_id || '—'}
          </span>
        ),
      },
    );
  }

  return (
    <SortableTable<FindingRecord>
      columns={columns}
      rows={findings}
      rowKey={(f) =>
        showProvenance
          ? `${(f as FindingOut).ws_id}/${(f as FindingOut).target_id}/${f.id}`
          : f.id
      }
      selection={selection}
      renderDetail={(f) =>
        f.profile === 'full' && f.report_summary ? (
          <TriageCard finding={f} />
        ) : (
          <FindingEvidence finding={f} />
        )
      }
    />
  );
}
