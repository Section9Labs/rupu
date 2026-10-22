// An event's content identity: what it says, without when it was read or the
// run attribution a history row adds. The SSE firehose replays a run's
// already-written events (a different `ts`, no `pos`, no attribution), so the
// same event read twice — or on two pages — has one identity. Live Events
// dedups on it, and a card keyed by it is the same card on Live Events and
// on its run's Events tab (so "seen" on one is seen on the other).

import type { RunEvent } from '../api';
import { HISTORY_ATTRIBUTION_KEYS } from '../query/lists/events';

/** Deterministic stringify (sorted keys). */
export function stableStringify(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(stableStringify).join(',')}]`;
  if (v !== null && typeof v === 'object') {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o).sort().map((k) => `${JSON.stringify(k)}:${stableStringify(o[k])}`).join(',')}}`;
  }
  return JSON.stringify(v);
}

const NOT_IDENTITY = new Set<string>(['ts', 'pos', ...HISTORY_ATTRIBUTION_KEYS]);

export function identityOf(ev: RunEvent): string {
  const raw = ev as Record<string, unknown>;
  const rest: Record<string, unknown> = {};
  for (const k of Object.keys(raw)) {
    if (NOT_IDENTITY.has(k)) continue;
    rest[k] = raw[k];
  }
  return stableStringify(rest);
}
