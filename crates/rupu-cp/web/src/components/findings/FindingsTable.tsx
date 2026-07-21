// Shared findings list rendered as a SortableTable. Used by the global Findings
// page, the per-project Findings tab, and the coverage-detail Overview. Columns:
// Severity | Summary | File:Line | CWE | Concern, plus Project | Target when
// `showProvenance` is set (the cross-project / project-scoped variants). Each
// row expands (via `renderDetail`) to its evidence panel.
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
  wsId,
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
}) {
  const navigate = useNavigate();

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
        if (!loc) return <span className="text-ink-mute">—</span>;
        const rowWsId = (f as FindingOut).ws_id ?? wsId;
        if (f.file_path && f.line_range && rowWsId) {
          return (
            <button
              type="button"
              onClick={() =>
                navigate(
                  `/projects/${encodeURIComponent(rowWsId)}/code?path=${encodeURIComponent(f.file_path!)}&line=${f.line_range![0]}`,
                )
              }
              className="font-mono text-note break-all text-brand-700 hover:underline"
            >
              {loc}
            </button>
          );
        }
        return <span className="font-mono text-note text-ink-mute break-all">{loc}</span>;
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
