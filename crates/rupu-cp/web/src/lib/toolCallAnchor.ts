// Scroll-to-tool-call for `#call-<id>` links.
//
// The netflow table's "transcript" link for a socket flow is
// `/runs/<run>#call-<encodeURIComponent(tool_call_id)>`. `ToolCard` marks its
// wrapper `data-call-id="<raw id>"`; this hook decodes the hash and scrolls
// the matching card into view.
//
// Best effort, and honest about it: a run's transcript panel shows ONE step's
// (or one fan-out unit's) transcript at a time and loads it asynchronously, and
// nothing indexes which step a call id belongs to. So the card is looked up for
// a bounded time in whatever transcript is on screen; if it never appears the
// hook reports `missing` and the page says so, rather than silently scrolling
// nowhere.

import { useEffect, useState } from 'react';

const PREFIX = '#call-';

/** The raw tool call id carried by a `#call-<encoded id>` hash, else null. */
export function parseCallHash(hash: string): string | null {
  if (!hash.startsWith(PREFIX)) return null;
  const enc = hash.slice(PREFIX.length);
  if (enc === '') return null;
  try {
    return decodeURIComponent(enc);
  } catch {
    return null; // malformed percent-escape — not a link we produced
  }
}

/** The card element for a raw call id. Compares the attribute directly so no
 *  CSS escaping of the id is needed. */
export function findToolCallEl(callId: string, root: ParentNode = document): HTMLElement | null {
  for (const el of Array.from(root.querySelectorAll<HTMLElement>('[data-call-id]'))) {
    if (el.dataset.callId === callId) return el;
  }
  return null;
}

export type ToolCallAnchorState =
  | { status: 'idle' }
  | { status: 'searching'; callId: string }
  | { status: 'found'; callId: string }
  | { status: 'missing'; callId: string };

const POLL_MS = 200;
const GIVE_UP_MS = 8000;
const HIGHLIGHT_MS = 2000;
const HIGHLIGHT_CLASSES = ['ring-2', 'ring-brand-500', 'rounded-md'];

/**
 * On mount and on every hash change, when the hash is `#call-<id>`, poll for
 * the card (the transcript may still be loading) and scroll it to the centre,
 * briefly ringing it. `enabled` false pauses the search (e.g. the transcript
 * tab is not showing) and resets to `idle`, so a stale "missing" notice never
 * outlives the tab it described.
 *
 * `rearmKey` identifies the transcript currently on screen (step / unit /
 * path). When it changes — the user picked another step, as the "missing"
 * notice tells them to — the search starts over from `searching`, so the card
 * is scrolled to as soon as the owning step's transcript renders it.
 */
export function useToolCallAnchor(
  hash: string,
  enabled = true,
  rearmKey = '',
): ToolCallAnchorState {
  const [state, setState] = useState<ToolCallAnchorState>({ status: 'idle' });

  useEffect(() => {
    const callId = parseCallHash(hash);
    if (callId === null) {
      setState({ status: 'idle' });
      return;
    }
    if (!enabled) {
      setState({ status: 'idle' });
      return;
    }

    setState({ status: 'searching', callId });
    let timer: ReturnType<typeof setTimeout> | undefined;
    let ringTimer: ReturnType<typeof setTimeout> | undefined;
    let ringed: HTMLElement | null = null;
    const startedAt = Date.now();

    const attempt = () => {
      const el = findToolCallEl(callId);
      if (el) {
        el.scrollIntoView({ block: 'center' });
        el.classList.add(...HIGHLIGHT_CLASSES);
        ringed = el;
        ringTimer = setTimeout(() => el.classList.remove(...HIGHLIGHT_CLASSES), HIGHLIGHT_MS);
        setState({ status: 'found', callId });
        return;
      }
      if (Date.now() - startedAt >= GIVE_UP_MS) {
        setState({ status: 'missing', callId });
        return;
      }
      timer = setTimeout(attempt, POLL_MS);
    };
    attempt();

    return () => {
      if (timer) clearTimeout(timer);
      if (ringTimer) clearTimeout(ringTimer);
      ringed?.classList.remove(...HIGHLIGHT_CLASSES);
    };
  }, [hash, enabled, rearmKey]);

  return state;
}
