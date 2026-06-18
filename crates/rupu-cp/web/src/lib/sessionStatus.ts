// Session-status coercion. The backend types `SessionSummary.status` as
// `unknown` (the wire shape isn't pinned yet), so we coerce defensively: a
// string is used as-is; anything else is JSON-stringified for display. Common
// lifecycle values map to a colored dot tone; everything else falls back
// neutral. Tailwind classes are STATIC literals keyed off a small map.

export type SessionTone = 'running' | 'idle' | 'stopped' | 'neutral';

const TONE_DOT: Record<SessionTone, string> = {
  running: 'bg-blue-500',
  idle: 'bg-green-500',
  stopped: 'bg-slate-400',
  neutral: 'bg-slate-400',
};

/** Raw → display label. Strings pass through; non-strings are stringified. */
export function sessionStatusLabel(status: unknown): string {
  if (typeof status === 'string') return status;
  if (status === null || status === undefined) return 'unknown';
  try {
    return JSON.stringify(status);
  } catch {
    return String(status);
  }
}

/** Map a coerced label to one of the four dot tones. */
export function sessionStatusTone(status: unknown): SessionTone {
  const label = sessionStatusLabel(status).toLowerCase();
  if (label.includes('run') || label.includes('active') || label.includes('working')) return 'running';
  if (label.includes('idle') || label.includes('ready') || label.includes('waiting')) return 'idle';
  if (label.includes('stop') || label.includes('done') || label.includes('archiv') || label.includes('exit'))
    return 'stopped';
  return 'neutral';
}

/** Static dot class for a session status. */
export function sessionStatusDot(status: unknown): string {
  return TONE_DOT[sessionStatusTone(status)];
}
