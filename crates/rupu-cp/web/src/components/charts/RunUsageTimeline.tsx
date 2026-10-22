import type { UsageTimelinePoint } from '../../lib/api';
import TokenFlow, { formatTokens } from './TokenFlow';

/** Abbreviated token-count label: `2000 → "2k"`, `1_200_000 → "1.2M"`. */
export const formatTokenTick = formatTokens;

/** Per-turn tokens across a run's steps (or a session's runs), drawn as the
 *  token flow: context in above the baseline, output below on its own scale,
 *  cache reads inside the context. `separators` adds the stage lanes (a
 *  workflow run's steps); `live` makes a running run's end breathe. */
export default function RunUsageTimeline({
  series,
  separators = false,
  live = false,
  zoom,
  onZoomChange,
}: {
  series: UsageTimelinePoint[];
  separators?: boolean;
  live?: boolean;
  /** The selected range of turns (point indices), when the page owns it. */
  zoom?: [number, number] | null;
  onZoomChange?: (zoom: [number, number] | null) => void;
}) {
  return <TokenFlow points={series} stages={separators} live={live} zoom={zoom} onZoomChange={onZoomChange} />;
}
