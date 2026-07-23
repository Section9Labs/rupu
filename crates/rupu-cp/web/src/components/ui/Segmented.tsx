// Segmented — "which view of this data?" control. Exclusive, always exactly
// one option active. ONE style (boxed, joined, `p-0.5`) — replaces the ≥3
// dialects of rounded-md tab strip that were copy-pasted per page (lifecycle
// tabs, Runs/Cycles/Claims, PivotPicker, 7d/30d/All). `PivotPicker` and
// `UsageRangeControls` are the first internal consumers (visual parity kept).

import { cn } from '../../lib/cn';

export interface SegmentedOption {
  value: string;
  label: string;
}

export type SegmentedSize = 'sm' | 'md';

const SIZE_CLS: Record<SegmentedSize, string> = {
  sm: 'px-2 py-0.5 text-note',
  md: 'px-2.5 py-1 text-ui',
};

export interface SegmentedProps {
  options: SegmentedOption[];
  value: string;
  onChange: (value: string) => void;
  size?: SegmentedSize;
  ariaLabel?: string;
}

export function Segmented({ options, value, onChange, size = 'md', ariaLabel }: SegmentedProps) {
  return (
    <div
      role="group"
      aria-label={ariaLabel}
      className="inline-flex items-center gap-0.5 rounded-md border border-border bg-panel p-0.5"
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(opt.value)}
            className={cn(
              // `capitalize` is a no-op for callers who already pass
              // Title-Case labels; PivotPicker relies on it to render raw
              // lowercase pivot values (`model`, `workflow`, …) visually
              // capitalized while keeping the accessible name/DOM text
              // lowercase (existing tests assert on the lowercase name).
              'rounded font-medium capitalize transition-colors whitespace-nowrap',
              SIZE_CLS[size],
              active ? 'bg-surface text-ink font-semibold' : 'text-ink-dim hover:text-ink',
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}

export default Segmented;
