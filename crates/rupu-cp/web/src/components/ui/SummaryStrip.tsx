// SummaryStrip — a key/value strip, not a run-on sentence (spec §"Summary").
// Each figure labelled, tabular numbers, a parenthetical promoted to a quiet
// second value (`sub`). Replaces lines like
// "620 repos · 2 providers · 0 autoflows (7 off) · …".
//
// Same bordered-grid layout as KpiStrip (see its note). Rendering only; callers
// pass already-formatted values (an em-dash for "not reported", never a 0).

import { cn } from '../../lib/cn';

export interface SummaryItem {
  label: string;
  /** Pre-formatted primary value (incl. "—" for not-reported). */
  value: React.ReactNode;
  /** Quiet trailing value, e.g. "· 7 off" / "· 3,109+ open". */
  sub?: React.ReactNode;
  testId?: string;
}

export function SummaryStrip({ items, className }: { items: SummaryItem[]; className?: string }) {
  return (
    <div className={cn('overflow-hidden rounded-lg border border-border bg-panel', className)}>
      <div className="-ml-px -mt-px grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6">
        {items.map((it) => (
          <div
            key={it.label}
            data-testid={it.testId}
            className="border-l border-t border-border px-4 py-2.5"
          >
            <div className="text-note text-ink-mute">{it.label}</div>
            <div className="mt-0.5 text-lead font-semibold tabular-nums text-ink">
              {it.value}
              {it.sub != null && it.sub !== '' && (
                <span className="ml-1 text-note font-normal text-ink-mute">{it.sub}</span>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

export default SummaryStrip;
