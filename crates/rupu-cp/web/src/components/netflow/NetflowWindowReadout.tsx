// NetflowWindowReadout — the operator's honest statement of what time
// window is actually in effect for the data on screen. Built SOLELY from
// the server's own `window` echo (`NetflowResponse.window`), never from the
// query's reading of `since:` / `until:` — same source `NetflowTable`'s
// `appliedWindow` uses (`ScopeDisclosure.tsx`'s `netflowWindowApplied`). A
// span like `since:1h` resolves against the clock once, when the query is
// committed; the timestamps here are the ones that produced the data.

import { absoluteTime } from '../../lib/time';
import type { NetflowWindowEcho } from './ScopeDisclosure';

function readoutText(window: NetflowWindowEcho): string {
  const { from, to } = window;
  if (from === null && to === null) return 'Showing all recorded flows.';
  if (from !== null && to === null) return `Showing flows from ${absoluteTime(from)} onward.`;
  if (from === null && to !== null) return `Showing flows up to ${absoluteTime(to)}.`;
  return `Showing flows from ${absoluteTime(from)} to ${absoluteTime(to)}.`;
}

export function NetflowWindowReadout({ appliedWindow }: { appliedWindow: NetflowWindowEcho }) {
  return <p className="text-note text-ink-mute">{readoutText(appliedWindow)}</p>;
}

export default NetflowWindowReadout;
