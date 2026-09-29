// Pure narrowing for the Situation Room live stream: a group chip AND a
// free-text query. Single source of truth shared by EventStream and its tests
// so the "what does the search match?" contract is verified independently of
// the DOM.

import type { CardGroup, StreamCard } from './cards';

/** `all` plus every real card group — the filter-chip vocabulary. */
export type StreamFilter = 'all' | CardGroup;

/** The searchable text of a card: everything an operator might scan for —
 *  the badge, headline, secondary line, run id, project, agent, file ref,
 *  step id and severity. Lower-cased once for substring matching. */
function haystack(c: StreamCard): string {
  return [
    c.badge, c.title, c.detail, c.runId, c.projectName, c.workflow, c.agent,
    c.stepId, c.stepKind, c.unitKey, c.filePath, c.severity,
  ]
    .filter((s): s is string => typeof s === 'string' && s.length > 0)
    .join(' ')
    .toLowerCase();
}

/** Narrow `cards` by the active group chip and a free-text query.
 *
 * - `group === 'all'` keeps every group; otherwise keeps only that group.
 * - `query` is a case-insensitive substring match over {@link haystack};
 *   blank / whitespace-only queries match everything.
 *
 * Order is preserved (the caller feeds newest-first cards).
 */
export function filterStreamCards(
  cards: readonly StreamCard[],
  group: StreamFilter,
  query: string,
): StreamCard[] {
  const q = query.trim().toLowerCase();
  return cards.filter((c) => {
    if (group !== 'all' && c.group !== group) return false;
    if (q.length > 0 && !haystack(c).includes(q)) return false;
    return true;
  });
}
