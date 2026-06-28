// Shared findings list rendered as a SortableTable. Used by the global Findings
// page, the per-project Findings tab, and the coverage-detail Overview. Columns:
// Severity | Summary | File:Line | CWE | Concern, plus Project | Target when
// `showProvenance` is set (the cross-project / project-scoped variants). Each
// row expands (via `renderDetail`) to its evidence panel.
//
// Severity sorts by rank with critical highest; Summary / File / Concern sort
// lexically. The rows arrive backend-sorted (critical → info, newest first), so
// no `initialSort` is supplied — the unsorted default preserves that order.

import {
  normFindingSeverity,
  sevRank,
  type FindingOut,
  type FindingRecord,
} from '../../lib/api';
import { cweFromFinding } from '../../lib/cwe';
import SeverityChip from '../coverage/SeverityChip';
import SortableTable, { type Column } from '../lists/SortableTable';
import { FindingEvidence } from './FindingEvidence';

function location(f: FindingRecord): string {
  const parts: string[] = [];
  if (f.file_path) parts.push(f.file_path);
  if (f.line_range) parts.push(`${f.line_range[0]}–${f.line_range[1]}`);
  return parts.join(':');
}

export function FindingsTable({
  findings,
  showProvenance = false,
}: {
  findings: FindingRecord[];
  /** Render Project / Target columns. Only set when `findings` are `FindingOut`
   *  (they carry the provenance keys). */
  showProvenance?: boolean;
}) {
  const columns: Column<FindingRecord>[] = [
    {
      key: 'severity',
      header: 'Severity',
      width: 'w-24',
      sortable: true,
      // sevRank: critical=0 … info=4, so ascending (first click) sorts
      // most-severe first — matching the backend default order + intuition.
      sortValue: (f) => sevRank(normFindingSeverity(f.severity)),
      render: (f) => <SeverityChip severity={normFindingSeverity(f.severity)} />,
    },
    {
      key: 'summary',
      header: 'Summary',
      sortable: true,
      sortValue: (f) => f.summary,
      render: (f) => <span className="text-ink leading-snug">{f.summary}</span>,
    },
    {
      key: 'location',
      header: 'File:Line',
      sortable: true,
      sortValue: (f) => f.file_path ?? null,
      render: (f) => {
        const loc = location(f);
        return loc ? (
          <span className="font-mono text-note text-ink-mute break-all">{loc}</span>
        ) : (
          <span className="text-ink-mute">—</span>
        );
      },
    },
    {
      key: 'cwe',
      header: 'CWE',
      width: 'w-24',
      render: (f) => {
        const cwe = cweFromFinding(f);
        return cwe ? (
          <a
            href={cwe.url}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center rounded bg-slate-100 px-1.5 py-0.5 text-note font-medium text-slate-600 ring-1 ring-slate-200 hover:bg-slate-200"
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
      sortable: true,
      sortValue: (f) => f.concern_id ?? null,
      render: (f) =>
        f.concern_id ? (
          <span className="font-mono text-note text-ink-mute break-all">{f.concern_id}</span>
        ) : (
          <span className="text-ink-mute">—</span>
        ),
    },
  ];

  if (showProvenance) {
    columns.push(
      {
        key: 'project',
        header: 'Project',
        render: (f) => <span className="text-ink-dim">{(f as FindingOut).project || '—'}</span>,
      },
      {
        key: 'target',
        header: 'Target',
        render: (f) => (
          <span className="font-mono text-note text-ink-mute break-all">
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
      renderDetail={(f) => <FindingEvidence finding={f} />}
    />
  );
}
