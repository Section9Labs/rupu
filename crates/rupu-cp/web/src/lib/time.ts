// Small, dependency-free time helpers for the Runs views. Kept local so we
// don't pull in date-fns just for two formatters.

/** Relative "time ago" string, e.g. "3m ago", "2h ago", "just now". */
export function relativeTime(iso?: string | null): string {
  if (!iso) return '—';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '—';
  const diffMs = Date.now() - t;
  const sec = Math.round(diffMs / 1000);
  if (sec < 5) return 'just now';
  if (sec < 60) return `${sec}s ago`;
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.round(hr / 24);
  if (day < 30) return `${day}d ago`;
  const mon = Math.round(day / 30);
  if (mon < 12) return `${mon}mo ago`;
  return `${Math.round(mon / 12)}y ago`;
}

/** Human duration between two ISO timestamps, e.g. "1m 12s", "340ms", "2h 3m". */
export function durationBetween(startIso?: string | null, endIso?: string | null): string {
  if (!startIso) return '—';
  const start = Date.parse(startIso);
  if (Number.isNaN(start)) return '—';
  const end = endIso ? Date.parse(endIso) : Date.now();
  if (Number.isNaN(end)) return '—';
  return formatDurationMs(Math.max(0, end - start));
}

/** Format a millisecond count as a compact human duration. */
export function formatDurationMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const totalSec = Math.floor(ms / 1000);
  const sec = totalSec % 60;
  const totalMin = Math.floor(totalSec / 60);
  const min = totalMin % 60;
  const hr = Math.floor(totalMin / 60);
  if (hr > 0) return `${hr}h ${min}m`;
  if (min > 0) return `${min}m ${sec}s`;
  return `${sec}s`;
}

/** Absolute timestamp, locale-formatted; "—" on missing/invalid. */
export function absoluteTime(iso?: string | null): string {
  if (!iso) return '—';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '—';
  return new Date(t).toLocaleString();
}
