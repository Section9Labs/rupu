// Project Overview tab body — at-a-glance summary for one project. The shell
// passes the already-loaded `getProject` bundle so this tab does NOT refetch.
// Markup moved verbatim from pages/ProjectDetail.tsx: Recent runs, Coverage
// summary, Sessions summary, and the Definitions link card. The rollup tiles
// and the full findings list stay out (shell + Findings tab own those).

import { Link } from 'react-router-dom';
import { Library, MessageSquare } from 'lucide-react';
import { type ProjectDetail } from '../../lib/api';
import { ListCard } from '../lists/ListCard';
import { StatusPill } from '../StatusPill';
import { TriggerChip } from '../TriggerChip';
import { relativeTime } from '../../lib/time';

// Top N recent runs shown inline; the "see all →" link covers the rest.
const RECENT_LIMIT = 5;

function SectionTitle({ title, href }: { title: string; href: string }) {
  return (
    <div className="flex items-center justify-between mb-2">
      <h2 className="text-sm font-semibold text-ink">{title}</h2>
      <Link
        to={href}
        className="text-note font-medium text-brand-600 hover:text-brand-700 transition-colors"
      >
        see all →
      </Link>
    </div>
  );
}

export interface ProjectOverviewTabProps {
  detail: ProjectDetail;
  wsId: string;
  /** Display label for the assessed % (e.g. "42%", "…", "—"). The shell owns
   *  the lazy assessed-pct fetch; pass its formatted label here. Defaults to
   *  "—" when omitted. */
  pctLabel?: string;
  /** Clamped assessed percentage (0–100) or null when unknown — only used to
   *  style the assessed-% span. */
  pct?: number | null;
}

export default function ProjectOverviewTab({
  detail,
  wsId,
  pctLabel = '—',
  pct = null,
}: ProjectOverviewTabProps) {
  const { sessions, coverage, recent_runs } = detail;
  const encodedId = encodeURIComponent(wsId);
  const recent = recent_runs.slice(0, RECENT_LIMIT);

  return (
    <div className="space-y-6">
      {/* ── Recent runs ── */}
      <section>
        <SectionTitle title="Recent runs" href={`/projects/${encodedId}/runs`} />
        {recent.length === 0 ? (
          <div className="rounded-xl border border-dashed border-border bg-panel/50 py-8 flex items-center justify-center">
            <p className="text-xs text-ink-mute">No runs yet</p>
          </div>
        ) : (
          <ListCard>
            {recent.map((r) => (
              <Link
                key={r.id}
                to={`/runs/${encodeURIComponent(r.id)}`}
                className="flex items-center gap-4 px-4 py-3 hover:bg-slate-50 transition-colors"
              >
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-ink truncate">{r.workflow_name}</span>
                    <TriggerChip trigger={r.trigger} />
                  </div>
                  <p className="text-note text-ink-dim mt-0.5">{relativeTime(r.started_at)}</p>
                </div>
                <StatusPill status={r.status} />
              </Link>
            ))}
          </ListCard>
        )}
      </section>

      {/* ── Bottom split: Coverage + Sessions ── */}
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        {/* Coverage summary */}
        <section>
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-sm font-semibold text-ink">Coverage</h2>
            <Link
              to={`/projects/${encodedId}/coverage`}
              className="text-note font-medium text-brand-600 hover:text-brand-700 transition-colors"
            >
              open →
            </Link>
          </div>
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 text-ui text-ink-dim">
            {coverage.targets === 0 ? (
              <span className="text-ink-mute">No coverage targets yet</span>
            ) : (
              <>
                <span className="font-medium text-ink">{coverage.targets}</span>{' '}
                target{coverage.targets !== 1 ? 's' : ''} ·{' '}
                <span className={pct !== null ? 'font-medium text-ink' : 'text-ink-mute'}>
                  {pctLabel}
                </span>{' '}
                assessed ·{' '}
                <span
                  className={
                    coverage.findings > 0 ? 'font-medium text-red-600' : 'font-medium text-ink'
                  }
                >
                  {coverage.findings}
                </span>{' '}
                finding{coverage.findings !== 1 ? 's' : ''}
              </>
            )}
          </div>
        </section>

        {/* Sessions summary */}
        <section>
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-sm font-semibold text-ink">Sessions</h2>
            <Link
              to={`/projects/${encodedId}/sessions`}
              className="text-note font-medium text-brand-600 hover:text-brand-700 transition-colors"
            >
              see all →
            </Link>
          </div>
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
            {sessions.total === 0 ? (
              <p className="text-ui text-ink-mute">No sessions yet</p>
            ) : (
              <div className="flex items-center gap-3 text-ui text-ink-dim">
                <span className="inline-flex items-center gap-1.5">
                  <MessageSquare size={12} className="text-ink-mute" />
                  <span className="font-medium text-ink">{sessions.total}</span> total
                </span>
                {sessions.active > 0 && (
                  <span className="inline-flex items-center gap-1.5">
                    <span className="w-1.5 h-1.5 rounded-full bg-blue-500" />
                    <span className="font-medium text-ink">{sessions.active}</span> active
                  </span>
                )}
              </div>
            )}
          </div>
        </section>
      </div>

      {/* ── Definitions ── */}
      <section>
        <SectionTitle title="Definitions" href={`/projects/${encodedId}/definitions`} />
        <Link
          to={`/projects/${encodedId}/definitions`}
          className="block bg-panel border border-border rounded-xl shadow-card px-4 py-3 hover:bg-slate-50 transition-colors"
        >
          <div className="flex items-center gap-2.5 text-ui text-ink-dim">
            <Library size={14} className="text-ink-mute" />
            <span>
              Agents, workflows &amp; autoflows visible to this project
              <span className="text-ink-mute"> — global + project-local </span>
              <span className="font-mono">.rupu/</span>
            </span>
          </div>
        </Link>
      </section>
    </div>
  );
}
