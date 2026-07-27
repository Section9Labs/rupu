// Session-status coercion. The backend types `SessionSummary.status` as
// `unknown` (the wire shape isn't pinned yet), so we coerce defensively: a
// string is used as-is; anything else is JSON-stringified for display. Common
// lifecycle values map to a colored dot tone; everything else falls back
// neutral. Dot colors come from the unified status palette (`lib/status.ts`)
// so session dots match run pills. `idle` stays a distinct tone from a
// finished run but reuses the green (done) hue; `stopped`/`neutral` reuse the
// muted pending slate.
//
// `rupu-cli`'s `SessionStatus` enum (`session.rs:213-220`) actually only ever
// emits four values — `idle | running | failed | stopped` — a DIFFERENT
// vocabulary from the run-status enum `lib/status.ts` speaks (a session
// `idle` is not a run `completed`; never rename one into the other). Task 2
// (shared animated status glyph) maps that vocabulary onto the same visual
// tone + motion language `StatusPill` uses, via `SESSION_STATUS_DESCRIPTOR` /
// `SessionStatusPill` (`components/StatusPill.tsx`) — the WORDS stay
// session-native, only the color/icon/motion are shared.

import { STATUS, type StatusDescriptor } from './status';

/** The confirmed `rupu-cli` session vocabulary (session.rs:213-220). */
export type SessionStatusValue = 'idle' | 'running' | 'failed' | 'stopped';

/** True when a coerced status label is one of the four values `rupu-cli`
 * actually emits, as opposed to older/looser inputs this module also has to
 * tolerate (the wire type is `unknown`). */
export function isSessionStatusValue(v: string): v is SessionStatusValue {
  return v === 'idle' || v === 'running' || v === 'failed' || v === 'stopped';
}

/**
 * Canonical session vocabulary → shared visual descriptor. Labels are
 * overridden back to the session-native word (never the run word the
 * underlying descriptor carries) — only color/icon/motion are borrowed.
 *   idle    → completed's quiet green (a session at rest, not a finished run)
 *   running → running's blue + the shared motion pulse
 *   failed  → failed's loud red, static (terminal states don't animate)
 *   stopped → pending's quiet slate
 */
export const SESSION_STATUS_DESCRIPTOR: Record<SessionStatusValue, StatusDescriptor> = {
  idle: { ...STATUS.completed, label: 'Idle' },
  running: { ...STATUS.running, label: 'Running' },
  failed: { ...STATUS.failed, label: 'Failed' },
  stopped: { ...STATUS.pending, label: 'Stopped' },
};

export type SessionTone = 'running' | 'idle' | 'failed' | 'stopped' | 'neutral';

const TONE_DOT: Record<SessionTone, string> = {
  // Carries the shared running motion class too, so the existing bespoke
  // Sessions dot (Sessions.tsx hasn't been ported to SessionStatusPill yet)
  // already animates consistently with every other page's running glyph.
  running: `${STATUS.running.dotClass} rg-pulse-run`, // bg-status-running (blue-500) + breathing ring
  idle: STATUS.completed.dotClass, // bg-status-done (green-500)
  failed: STATUS.failed.dotClass, // bg-status-failed (red-500)
  stopped: STATUS.pending.dotClass, // bg-status-pending (slate-400)
  neutral: STATUS.pending.dotClass, // bg-status-pending (slate-400)
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

/** Map a coerced label to one of the five dot tones. */
export function sessionStatusTone(status: unknown): SessionTone {
  const label = sessionStatusLabel(status).toLowerCase();
  // Exact match against the confirmed rupu-cli vocabulary first — this is
  // what fixes `failed` (previously matched none of the substrings below and
  // silently fell through to `neutral`, indistinguishable from `stopped`).
  if (isSessionStatusValue(label)) return label;
  if (label.includes('run') || label.includes('active') || label.includes('working')) return 'running';
  if (label.includes('idle') || label.includes('ready') || label.includes('waiting')) return 'idle';
  if (label.includes('fail') || label.includes('error') || label.includes('crash')) return 'failed';
  if (label.includes('stop') || label.includes('done') || label.includes('archiv') || label.includes('exit'))
    return 'stopped';
  return 'neutral';
}

/** Static dot class for a session status. */
export function sessionStatusDot(status: unknown): string {
  return TONE_DOT[sessionStatusTone(status)];
}
