// Where a detail page's back link returns to. A link into a detail page
// carries the page it was clicked on (`useReturnHere`) in router state, and
// the detail page reads it (`useBackTo`), so the back link returns to the
// project tab, run or flow the reader came from instead of a fixed list.
// A page opened without that state (a deep link, a new tab) falls back to
// the list the caller names.

import { useLocation } from 'react-router-dom';

export type BackTo = {
  /** The path (with search and hash) the back link returns to. */
  to: string;
  /** What the back link calls that page. */
  label: string;
  /** The router state that page had, so its own back link survives a round trip. */
  state?: unknown;
};

export type BackState = { back: BackTo };

/** The label for a page, by the route prefix of its path. The first match wins. */
const LABELS: [prefix: string, label: string][] = [
  ['/projects/', 'Project'],
  ['/runs/', 'Run'],
  ['/agentiflows/', 'Flow'],
  ['/sessions/', 'Session'],
  ['/coverage/', 'Coverage'],
  ['/findings/', 'Finding'],
  ['/transcript', 'Transcript'],
  ['/security', 'Security'],
  ['/findings', 'Findings'],
];

export function backLabel(pathname: string): string {
  return LABELS.find(([prefix]) => pathname.startsWith(prefix))?.[1] ?? 'Back';
}

/** The `back` entry of a location's state, when it is one. Router state is
 *  whatever the previous page put there, so it is checked, not trusted. */
export function readBackState(state: unknown): BackTo | undefined {
  if (typeof state !== 'object' || state === null) return undefined;
  const back = (state as { back?: unknown }).back;
  if (typeof back !== 'object' || back === null) return undefined;
  const { to, label, state: inner } = back as { to?: unknown; label?: unknown; state?: unknown };
  if (typeof to !== 'string' || !to.startsWith('/') || typeof label !== 'string' || !label) return undefined;
  return { to, label, state: inner };
}

/** The router state for a link out of the current page, so the target's
 *  back link returns here. */
export function useReturnHere(): BackState {
  const { pathname, search, hash, state } = useLocation();
  return { back: { to: pathname + search + hash, label: backLabel(pathname), state } };
}

/** Where this page's back link goes: the page the reader came from, else
 *  `fallback`. */
export function useBackTo(fallback: BackTo): BackTo {
  return readBackState(useLocation().state) ?? fallback;
}
