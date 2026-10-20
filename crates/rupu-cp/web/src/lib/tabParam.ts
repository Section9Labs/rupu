import { useCallback } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';

export function readTab<T extends string>(search: string, tabs: readonly T[], fallback: T): T {
  const value = new URLSearchParams(search).get('tab');
  return value !== null && (tabs as readonly string[]).includes(value) ? (value as T) : fallback;
}

export function withTab(search: string, tab: string): string {
  const params = new URLSearchParams(search);
  params.set('tab', tab);
  return `?${params.toString()}`;
}

/** The page's active tab, kept in `?tab=` so a link away and back restores it. */
export function useTabParam<T extends string>(tabs: readonly T[], fallback: T): [T, (tab: T) => void] {
  const location = useLocation();
  const navigate = useNavigate();
  const { pathname, search, hash, state } = location;
  const tab = readTab(search, tabs, fallback);
  // Replace, never push: switching tabs must not add back-button entries.
  // The router state is carried over because pages read it (the finding
  // report's back link).
  const setTab = useCallback(
    (next: T) => navigate({ pathname, search: withTab(search, next), hash }, { replace: true, state }),
    [navigate, pathname, search, hash, state],
  );
  return [tab, setTab];
}
