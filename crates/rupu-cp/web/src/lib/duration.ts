/**
 * Canonical human duration from milliseconds. `null`/undefined → em-dash.
 * Scales ms → s → m → h: sub-second shows `ms`, sub-10s shows one decimal,
 * minutes show `Xm Ys`, and ≥1h shows `Xh Ym` (previously this never emitted
 * hours, so 2h rendered as "120m").
 */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null) return '—';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  if (s < 10) return `${Math.round(s * 10) / 10}s`;
  if (s < 60) return `${Math.round(s)}s`;
  const totalSec = Math.floor(s);
  const totalMin = Math.floor(totalSec / 60);
  if (totalMin < 60) return `${totalMin}m ${totalSec % 60}s`;
  const hr = Math.floor(totalMin / 60);
  return `${hr}h ${totalMin % 60}m`;
}
