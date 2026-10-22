// What this viewer has marked "seen" in the Needs-you deck: a failure or a
// finding they looked at stops being pinned (its row keeps a quiet "seen"
// tag). One store per browser, shared by every event surface in the tab, kept
// in localStorage so a reload remembers it. A browser that refuses storage
// still works: the marks last until the tab closes.

import { useCallback, useSyncExternalStore } from 'react';

const STORAGE_KEY = 'rupu.cp.events.seen';
const CAP = 2000;

/** FNV-1a, 32 bit, base 36: event keys are whole JSON identities, too long to keep. */
export function seenId(key: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < key.length; i++) {
    h ^= key.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

let ids: string[] | null = null;
let set = new Set<string>();
const listeners = new Set<() => void>();

function load(): void {
  if (ids) return;
  ids = [];
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    if (Array.isArray(parsed)) ids = parsed.filter((v): v is string => typeof v === 'string').slice(-CAP);
  } catch {
    /* no storage: marks last the session */
  }
  set = new Set(ids);
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function snapshot(): Set<string> {
  load();
  return set;
}

export function markSeen(key: string): void {
  load();
  const id = seenId(key);
  if (set.has(id)) return;
  ids = [...(ids ?? []), id].slice(-CAP);
  set = new Set(ids);
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(ids));
  } catch {
    /* kept in memory */
  }
  for (const fn of listeners) fn();
}

/** Forget every mark (tests). */
export function resetSeen(): void {
  ids = null;
  set = new Set();
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    /* nothing kept */
  }
  for (const fn of listeners) fn();
}

export function useSeen(): { isSeen: (key: string) => boolean; markSeen: (key: string) => void; version: Set<string> } {
  const current = useSyncExternalStore(subscribe, snapshot, snapshot);
  const isSeen = useCallback((key: string) => current.has(seenId(key)), [current]);
  return { isSeen, markSeen, version: current };
}
