// Expanded-row body for a full-profile finding in the findings tables: the
// root cause, attack path, ownership (gaps flagged) and report completeness at
// a glance, with a link to the full `/findings/:id` report page. Renders from
// the list row's `report_summary` only — the report body is not on list rows.

import { Link } from 'react-router-dom';
import Markdown from '../transcript/Markdown';
import type { FindingRecord } from '../../lib/api';

export default function TriageCard({ finding }: { finding: FindingRecord }) {
  const s = finding.report_summary;
  if (!s) return null;
  const gap = (v: string) => v.trim() === 'Unknown';
  return (
    <div className="grid gap-4 md:grid-cols-[minmax(0,1.4fr)_minmax(0,1fr)]">
      <div className="min-w-0 space-y-2">
        <h4 className="text-meta font-semibold uppercase tracking-wide text-ink-mute">Root cause</h4>
        <div className="text-ink-dim [&_p]:text-ui">
          <Markdown text={s.root_cause} />
        </div>
        {s.chain.length > 0 && (
          <>
            <h4 className="text-meta font-semibold uppercase tracking-wide text-ink-mute">Attack path</h4>
            <p className="font-mono text-note text-ink-dim">{s.chain.join(' → ')}</p>
          </>
        )}
      </div>
      <div className="min-w-0 space-y-2">
        <p className="text-ui">
          <span
            className={gap(s.owner) ? 'text-warn' : 'text-ink'}
            data-gap={gap(s.owner) ? 'true' : 'false'}
          >
            Owner: {s.owner}
          </span>
          <span className="text-ink-mute"> · </span>
          <span className={gap(s.product) ? 'text-warn' : 'text-ink'}>{s.product}</span>
        </p>
        <p className="text-note text-ink-mute">
          Report {s.completeness.filled}/{s.completeness.total}
          {s.completeness.gaps.length > 0 && <> · unknown: {s.completeness.gaps.join(', ')}</>}
          {s.has_poc && <> · PoC attached</>}
          {s.verification_status && <> · verification: {s.verification_status}</>}
        </p>
        <Link
          to={`/findings/${encodeURIComponent(finding.id)}`}
          className="inline-block rounded-md bg-brand-50 px-2.5 py-1 text-ui font-medium text-brand-700 hover:bg-brand-100"
        >
          Open full report →
        </Link>
      </div>
    </div>
  );
}
