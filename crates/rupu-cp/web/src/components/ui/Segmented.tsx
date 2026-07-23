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
  /** Renders each option label with a `capitalize` transform. Opt-in (off by
   *  default) — added for `PivotPicker`, which passes raw lowercase pivot
   *  values (`model`, `workflow`, …) as labels so existing consumer tests
   *  asserting on the (lowercase) accessible name keep passing, while still
   *  rendering Title-Case visually. `text-transform` doesn't change the
   *  accessible name/DOM text, only the rendering — safe either way. */
  capitalize?: boolean;
}

export function Segmented({
  options,
  value,
  onChange,
  size = 'md',
  ariaLabel,
  capitalize = false,
}: SegmentedProps) {
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
              'rounded font-medium transition-colors whitespace-nowrap',
              capitalize && 'capitalize',
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
