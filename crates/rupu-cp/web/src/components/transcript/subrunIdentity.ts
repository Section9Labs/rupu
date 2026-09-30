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

/** Fold `dispatch_started` events into `sub_run_id → identity` (last wins). */
export function buildSubrunIdentities(events: readonly RunEvent[]): Map<string, SubrunIdentity> {
  const map = new Map<string, SubrunIdentity>();
  for (const ev of events) {
    if (!isKnownRunEvent(ev) || ev.type !== 'dispatch_started') continue;
    const id: SubrunIdentity = {};
    if (ev.codename) id.codename = ev.codename;
    if (ev.agent) id.agent = ev.agent;
    if (ev.provider) id.provider = ev.provider;
    if (ev.model) id.model = ev.model;
    map.set(ev.sub_run_id, id);
  }
  return map;
}
