// RunWarningsBanner — the run page's "this run has warnings" notice, so an
// operator sees a `step_warning` without opening the Events tab. A warning is
// not a failure (the steps below still finish on their own merits), so this is
// the amber notice style the page already uses for the read-only-deploy note,
// not the red `error_message` block. A wide fan-out can warn once per unit, so
// only the first few are listed inline; the rest sit behind a disclosure.

import { AlertTriangle } from 'lucide-react';
import type { RunWarning } from '../lib/runGraphModel';

const INLINE_LIMIT = 5;

function WarningRow({ w }: { w: RunWarning }) {
  return (
    <li className="flex flex-wrap items-baseline gap-x-2">
      <span className="font-mono text-meta">
        {w.index === undefined ? w.stepId : `${w.stepId} · unit ${w.index}`}
      </span>
      <span className="min-w-0 break-words">{w.message}</span>
    </li>
  );
}

export default function RunWarningsBanner({ warnings }: { warnings: readonly RunWarning[] }) {
  if (warnings.length === 0) return null;
  const n = warnings.length;
  const shown = warnings.slice(0, INLINE_LIMIT);
  const rest = warnings.slice(INLINE_LIMIT);
  return (
    <div
      role="status"
      data-testid="run-warnings"
      className="mt-3 rounded-lg border border-warn/30 bg-warn-bg px-4 py-3 text-ui text-warn"
    >
      <div className="flex items-center gap-2 text-sm font-medium">
        <AlertTriangle size={16} className="shrink-0" aria-hidden />
        <span>{`${n} warning${n === 1 ? '' : 's'}`}</span>
      </div>
      <ul className="mt-1.5 space-y-1">
        {shown.map((w, i) => (
          <WarningRow key={i} w={w} />
        ))}
      </ul>
      {rest.length > 0 && (
        <details className="mt-1.5">
          <summary className="cursor-pointer text-note font-medium">{`Show ${rest.length} more`}</summary>
          <ul className="mt-1.5 space-y-1">
            {rest.map((w, i) => (
              <WarningRow key={i} w={w} />
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
