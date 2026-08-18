// NetflowWindowReadout — the operator's honest statement of what time
// window is actually in effect for the data on screen. Built SOLELY from
// the server's own `window` echo (`NetflowResponse.window`), NEVER from
// `TimeRangePicker`'s local UI state — same source `NetflowTable` and
// `NetflowGraph`'s `appliedWindow` already use (`ScopeDisclosure.tsx`'s
// `netflowWindowApplied`).
//
// Added per whole-branch review round 1 ("Also do"): the picker itself
// deliberately shows no applied-range summary derived from ITS OWN state
// (see TimeRangePicker.tsx's header comment on why local intent isn't a
// safe source of truth — Critical 1). An echo-derived readout carries none
// of that risk, and it answers two problems at once:
//   - it's the one place an operator can see the actual bounds a fetch
//     ran with, independent of whatever the picker happens to be showing
//     mid-edit;
//   - it neutralises the "Last hour" pinning ambiguity: `presetFrom`
//     resolves against `now()` once, at click time, and never re-derives,
//     so the label alone can go stale relative to the wall clock. The
//     actual timestamps here don't.

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
