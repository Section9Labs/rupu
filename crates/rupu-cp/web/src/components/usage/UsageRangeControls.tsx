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

import { type DashboardRange } from '../../lib/api';
import { PivotPicker, type Pivot } from './PivotPicker';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];

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
      <div className="flex rounded-md border border-border">
        {RANGES.map((r) => (
          <button
            key={r}
            type="button"
            onClick={() => onRangeChange(r)}
            className={`px-2 py-1 text-xs ${
              !isCustomWindow && range === r ? 'bg-surface text-ink' : 'text-ink-mute'
            }`}
          >
            {r}
          </button>
        ))}
      </div>
    </div>
  );
}
