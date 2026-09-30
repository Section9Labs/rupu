/**
 * Sub-agent identities for dispatch tool cards.
 *
 * A `dispatch_agent(s)` tool's output carries the child's codename and agent
 * but not its provider/model; the parent run's `dispatch_started` events carry
 * all four keyed by `sub_run_id`. The page that already holds the run's event
 * stream (RunDetail) builds this map once and provides it around its
 * transcript area, so each card looks its sub-run up without a fetch. Outside
 * a provider the map is empty and cards fall back to the tool output.
 */

import { createContext } from 'react';
import { isKnownRunEvent, type RunEvent } from '../../lib/api';

export interface SubrunIdentity {
  codename?: string;
  agent?: string;
  provider?: string;
  model?: string;
}

export type SubrunIdentityMap = ReadonlyMap<string, SubrunIdentity>;

export const SubrunIdentityContext = createContext<SubrunIdentityMap>(new Map());

/** Fold `dispatch_started` events into `sub_run_id → identity`, layered over
 *  `seed` (the graph response's server-side `subrun_identities`, folded from
 *  the whole events.jsonl). Live events win field by field. */
export function buildSubrunIdentities(
  events: readonly RunEvent[],
  seed?: Readonly<Record<string, SubrunIdentity>>,
): Map<string, SubrunIdentity> {
  const map = new Map<string, SubrunIdentity>();
  for (const [subRunId, id] of Object.entries(seed ?? {})) map.set(subRunId, { ...id });
  for (const ev of events) {
    if (!isKnownRunEvent(ev) || ev.type !== 'dispatch_started') continue;
    const id: SubrunIdentity = { ...map.get(ev.sub_run_id) };
    if (ev.codename) id.codename = ev.codename;
    if (ev.agent) id.agent = ev.agent;
    if (ev.provider) id.provider = ev.provider;
    if (ev.model) id.model = ev.model;
    map.set(ev.sub_run_id, id);
  }
  return map;
}

function sameIdentity(a: SubrunIdentity, b: SubrunIdentity): boolean {
  return a.codename === b.codename && a.agent === b.agent && a.provider === b.provider && a.model === b.model;
}

/** True when two identity maps hold the same entries — lets a page keep the
 *  previous Map reference (and so skip re-rendering every context consumer)
 *  when an event tick didn't change any sub-run identity. */
export function sameSubrunIdentities(a: SubrunIdentityMap, b: SubrunIdentityMap): boolean {
  if (a === b) return true;
  if (a.size !== b.size) return false;
  for (const [k, v] of a) {
    const w = b.get(k);
    if (!w || !sameIdentity(v, w)) return false;
  }
  return true;
}
