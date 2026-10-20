// KpiStrip — KPIs as TYPE, not tiles (spec §"KPIs"): one panel divided by
// hairlines, label over number, the value that needs attention in red. Tabular
// numbers; no sparkline on a KPI (the trend gets its own chart). Replaces the
// bordered-tile grid.
//
// Each item is a label + a value + an optional meta line (which must name its
// baseline — "+4 vs prior 30d", never a bare "+4"). `attention` paints the
// value in `status-failed`. Rendering only; callers supply already-formatted
// strings/nodes. Token classes only.
//
// Layout: the classic bordered-grid trick — an outer clipped, rounded, bordered
// panel wraps an inner grid whose cells each carry a top+left hairline, pulled
// up/left by one pixel so the outermost hairlines tuck under the panel border.
// That renders one divided panel at any item count and any breakpoint, with no
// dangling edge on an odd last row.

import { cn } from '../../lib/cn';

export interface KpiItem {
  label: string;
  /** Pre-formatted (e.g. "92%", "518", "—"). */
  value: React.ReactNode;
  /** Quiet line under the number — always name the baseline of any relative figure. */
  meta?: React.ReactNode;
  /** Paint the value in the attention (red) tone. */
  attention?: boolean;
  /** Stable hook for tests. */
  testId?: string;
}

export function KpiStrip({ items, className }: { items: KpiItem[]; className?: string }) {
  return (
    <div className={cn('overflow-hidden rounded-xl border border-border bg-panel', className)}>
      <div className="-ml-px -mt-px grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5">
        {items.map((it) => (
          <div key={it.label} data-testid={it.testId} className="border-l border-t border-border p-4">
            <div className="text-note font-medium text-ink-dim">{it.label}</div>
            <div
              className={cn(
                'mt-2 text-2xl font-semibold tabular-nums tracking-tight',
                it.attention ? 'text-status-failed' : 'text-ink',
              )}
            >
              {it.value}
            </div>
            {it.meta != null && it.meta !== '' && (
              <div className="mt-1 text-note text-ink-dim">{it.meta}</div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

export default KpiStrip;
