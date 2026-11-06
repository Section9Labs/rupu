// A derived list that keeps its identity while its items don't change, so
// memos keyed on it skip work when an unrelated update rebuilt it.

import { useRef } from 'react';

/** `next`, or the previous list when it has the same items by `same`. */
export function useStableList<T>(next: T[], same: (a: T, b: T) => boolean = Object.is): T[] {
  const prev = useRef<T[]>(next);
  const p = prev.current;
  if (p !== next && (p.length !== next.length || next.some((x, i) => !same(x, p[i])))) prev.current = next;
  return prev.current;
}
