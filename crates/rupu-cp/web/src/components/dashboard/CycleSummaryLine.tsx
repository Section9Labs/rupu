// CycleSummaryLine — the one line of aggregate cycle numbers (spec §5.5).
//
// Beneath the throughput chart. The per-cycle detail, the per-run drill-in,
// the old `+N clean` expansion pill: all of that lived in the now-removed
// activity feed and belongs to `/runs`, not the dashboard. This is the only
// cycle information a dashboard needs — three scalars and a link out.
//
// `clean`/`with_failures` are `Option<u64>` on the wire: a host that cannot
// report the breakdown (SSH) contributes `null`, which renders as an
// em-dash, never a fabricated `0`.

import { Link } from 'react-router-dom';
import type { CycleCounts } from '../../lib/api';

export function CycleSummaryLine({
  cycles,
  cyclesPartial,
}: {
  cycles: CycleCounts;
  /** True = clean/with_failures below is a partial split, not a fleet total. */
  cyclesPartial: boolean;
}) {
  return (
    <div className="flex items-center justify-between text-sm text-[rgb(var(--c-ink-dim))]">
      <span>
        <span className="font-medium text-[rgb(var(--c-ink))]">{cycles.total}</span> cycles ·{' '}
        <span className="text-[rgb(var(--c-ink))]">{cycles.clean === null ? '—' : cycles.clean}</span>{' '}
        clean ·{' '}
        <span className="text-[rgb(var(--c-ink))]">
          {cycles.with_failures === null ? '—' : cycles.with_failures}
        </span>{' '}
        with failures
        {cyclesPartial && (
          <span title="Some reporting hosts do not supply the clean/failed breakdown — this is a partial split, not a fleet total.">
            {' '}
            (partial)
          </span>
        )}
      </span>
      <Link to="/runs" className="text-[rgb(var(--c-ink-mute))] hover:text-[rgb(var(--c-ink))]">
        see all →
      </Link>
    </div>
  );
}

export default CycleSummaryLine;
