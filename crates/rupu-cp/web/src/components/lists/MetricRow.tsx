import { Link } from 'react-router-dom';
import type { ReactNode } from 'react';

export interface Metric {
  label: string;
  /** null/undefined → the metric is omitted (genuinely-absent ≠ zero). */
  value: string | null | undefined;
}

/**
 * Shared list row (the "metric strip" design): a header line (identity +
 * chips + a trailing node such as a status pill) above a labeled stat strip.
 */
export default function MetricRow({
  header,
  trailing,
  metrics,
  to,
}: {
  header: ReactNode;
  trailing?: ReactNode;
  metrics: Metric[];
  to?: string;
}) {
  const body = (
    <div className="px-4 py-2.5">
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1 flex items-center gap-2 flex-wrap">{header}</div>
        {trailing}
      </div>
      <div className="mt-1.5 flex items-end gap-4 flex-wrap">
        {metrics
          .filter((m) => m.value != null)
          .map((m) => (
            <span key={m.label} className="inline-flex flex-col leading-tight">
              <span className="text-lead font-semibold text-ink tabular-nums">{m.value}</span>
              <span className="text-[9px] uppercase tracking-wide text-ink-mute">{m.label}</span>
            </span>
          ))}
      </div>
    </div>
  );
  return to ? (
    <Link to={to} className="block hover:bg-slate-50 transition-colors">
      {body}
    </Link>
  ) : (
    <div className="hover:bg-slate-50 transition-colors">{body}</div>
  );
}
