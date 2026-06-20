import { useEffect } from 'react';

/**
 * Subscribe to a global keyboard hotkey. The handler receives the raw
 * KeyboardEvent so callers can decide whether to preventDefault().
 *
 * Matching is intentionally minimal — we test the lowercase `key` and the
 * combination of meta/ctrl/alt/shift modifiers.
 */
export interface HotkeyOpts {
  /** lowercase value of `event.key` (e.g. "k", "escape"). */
  key: string;
  /** true ⇒ require meta (Cmd on Mac) OR ctrl (Linux/Windows). */
  metaOrCtrl?: boolean;
  /** true ⇒ require shift. false/undefined ⇒ don't care. */
  shift?: boolean;
}

export function useHotkey(opts: HotkeyOpts, handler: (e: KeyboardEvent) => void) {
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key.toLowerCase() !== opts.key.toLowerCase()) return;
      if (opts.metaOrCtrl && !(e.metaKey || e.ctrlKey)) return;
      if (opts.shift !== undefined && e.shiftKey !== opts.shift) return;
      handler(e);
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [opts.key, opts.metaOrCtrl, opts.shift, handler]);
}
