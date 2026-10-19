/**
 * Event-kind taxonomy for the transcript reader: maps a `TurnBlock` to the
 * glyph/row class it renders as, the filter chips it matches, and the counts
 * shown on those chips. Pure — no React.
 */

import type { TurnBlock, TurnView } from './transcriptView';

export type FilterKind = 'all' | 'said' | 'thinking' | 'commands' | 'findings';

export type EventKind =
  | 'message'
  | 'thinking'
  | 'said'
  | 'user'
  | 'command'
  | 'finding'
  | 'tool'
  | 'quiet';

export function blockEventKind(block: TurnBlock): EventKind {
  switch (block.kind) {
    case 'tool':
      if (block.view.kind === 'finding') return 'finding';
      if (block.view.kind === 'terminal') return 'command';
      return 'tool';
    case 'assistant':
      return 'said';
    case 'thinking':
      return 'thinking';
    case 'user':
      return 'user';
    case 'message':
      return 'message';
    default:
      return 'quiet';
  }
}

const FILTER_KIND: Record<Exclude<FilterKind, 'all'>, EventKind> = {
  said: 'said',
  thinking: 'thinking',
  commands: 'command',
  findings: 'finding',
};

export function blockMatchesFilter(block: TurnBlock, filter: FilterKind): boolean {
  if (filter === 'all') return true;
  return blockEventKind(block) === FILTER_KIND[filter];
}

export function filterCounts(turns: TurnView[]): Record<Exclude<FilterKind, 'all'>, number> {
  const counts = { said: 0, thinking: 0, commands: 0, findings: 0 };
  for (const t of turns) {
    for (const b of t.blocks) {
      for (const f of Object.keys(counts) as (keyof typeof counts)[]) {
        if (blockMatchesFilter(b, f)) counts[f] += 1;
      }
    }
  }
  return counts;
}
