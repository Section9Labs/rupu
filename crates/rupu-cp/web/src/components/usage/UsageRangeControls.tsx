// UsageRangeControls — the 7/30/All + drag-selected "custom" chip + pivot
// picker row shared by every mount of the interactive spend-over-time graph
// (`/usage`, `ProjectUsageTimeline`, `AgentUsageTimeline`). Extracted out of
// `ProjectUsageTimeline` (Task 3 of the Overview/Agent-detail usage-reuse
// pass) since the exact same markup was about to be copy-pasted a third
// time. `Usage.tsx` is NOT switched to this — its layout interleaves the
// same controls into a wider header row (title, host-freshness strip, a
// "refresh failed" notice) rather than a standalone block, so extracting
// there would risk a regression for no real gain; it keeps its own copy.
//
// Purely presentational / controlled — no state of its own. All five
// dimensions (which range is active, whether a custom drag-window is
// active, the active pivot) are owned by the caller, exactly as they were
// before extraction.
//
// The 7d/30d/All picker is reimplemented internally on `ui/Segmented` (One
// Control Language kit) — props and visual result unchanged.

import { type DashboardRange } from '../../lib/api';
import { Segmented } from '../ui/Segmented';
import { PivotPicker, type Pivot } from './PivotPicker';

const RANGE_OPTIONS: { value: DashboardRange; label: string }[] = [
  { value: '7d', label: '7d' },
  { value: '30d', label: '30d' },
  { value: 'all', label: 'All' },
];

export default function UsageRangeControls({
  range,
  isCustomWindow,
  onRangeChange,
  onClearCustom,
  pivot,
  onPivotChange,
}: {
  range: DashboardRange;
  isCustomWindow: boolean;
  onRangeChange: (r: DashboardRange) => void;
  onClearCustom: () => void;
  pivot: Pivot;
  onPivotChange: (p: Pivot) => void;
}) {
  return (
    <div className="flex flex-wrap items-center justify-end gap-2">
      <PivotPicker value={pivot} onChange={onPivotChange} />
      {isCustomWindow && (
        <button
          type="button"
          onClick={onClearCustom}
          className="rounded-full border border-border px-2 py-0.5 text-[10px] text-ink-mute hover:bg-surface"
          title="Clear the drag-selected window and return to the active preset"
        >
          custom · ×
        </button>
      )}
      <Segmented
        ariaLabel="Range"
        size="sm"
        options={RANGE_OPTIONS}
        // No preset reads as active while a custom drag-window is in effect
        // (matches the pre-extraction behavior) — `'__custom__'` matches
        // none of RANGE_OPTIONS' values.
        value={isCustomWindow ? '__custom__' : range}
        onChange={(v) => onRangeChange(v as DashboardRange)}
      />
    </div>
  );
}
