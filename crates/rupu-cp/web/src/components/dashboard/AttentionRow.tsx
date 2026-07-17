// AttentionRow — the triage ribbon, weighted.
//
// Was four equal chips. But AwaitingApproval and Paused are the only states
// where the system is blocked ON THE OPERATOR; open findings is a backlog, not
// an interrupt. Equal weight was the bug.

import { Link } from 'react-router-dom';
import type { ActiveCounts } from '../../lib/api';

export function AttentionRow({
  active,
  failedInWindow,
  findingsOpen,
  findingsPartial,
}: {
  active: ActiveCounts;
  failedInWindow: number;
  /** `null` = nobody reported. Render "—", never "0". */
  findingsOpen: number | null;
  /** True = the number below is a partial sum. Mark it; never imply completeness. */
  findingsPartial: boolean;
}) {
  const blocked = active.awaiting_approval + active.paused;

  return (
    <div className="flex flex-wrap items-stretch gap-3">
      <Link
        to="/runs?lifecycle=active&status=awaiting_approval"
        className={`flex-1 rounded-lg border px-4 py-3 ${
          blocked > 0
            ? 'border-[rgb(var(--c-status-awaiting))] bg-[rgb(var(--c-surface))]'
            : 'border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))]'
        }`}
      >
        <div className="text-xs text-[rgb(var(--c-ink-dim))]">Blocked on you</div>
        <div className="text-2xl font-semibold tabular-nums text-[rgb(var(--c-ink))]">
          {blocked}
        </div>
      </Link>
      <Link
        to="/runs?lifecycle=failed"
        className={`flex-1 rounded-lg border px-4 py-3 ${
          failedInWindow > 0
            ? 'border-[rgb(var(--c-status-failed))]'
            : 'border-[rgb(var(--c-border))]'
        } bg-[rgb(var(--c-panel))]`}
      >
        <div className="text-xs text-[rgb(var(--c-ink-dim))]">Failed</div>
        <div className="text-2xl font-semibold tabular-nums text-[rgb(var(--c-ink))]">
          {failedInWindow}
        </div>
      </Link>
      {/* Demoted: a backlog, not an interrupt. */}
      <Link
        to="/findings"
        className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] px-4 py-3"
      >
        <div className="text-xs text-[rgb(var(--c-ink-dim))]">
          Open findings
          {findingsPartial && (
            <span title="Some reporting hosts do not supply a findings count — this is a partial sum, not a fleet total.">
              {' '}
              (partial)
            </span>
          )}
        </div>
        <div className="text-base tabular-nums text-[rgb(var(--c-ink-dim))]">
          {/* `null` means nobody reported. "—" not "0": unknown is not none. */}
          {findingsOpen === null ? '—' : `${findingsOpen}${findingsPartial ? '+' : ''}`}
        </div>
      </Link>
    </div>
  );
}
