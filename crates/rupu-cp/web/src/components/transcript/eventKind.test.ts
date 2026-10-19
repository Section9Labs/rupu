import { describe, it, expect } from 'vitest';
import { blockEventKind, blockMatchesFilter, filterCounts } from './eventKind';
import type { ToolKind, TurnBlock, TurnView } from './transcriptView';

function tool(kind: ToolKind): TurnBlock {
  return { kind: 'tool', view: { tool: kind, input: {}, kind } };
}

function turn(blocks: TurnBlock[]): TurnView {
  return {
    blocks,
    tokensIn: null,
    tokensOut: null,
    summary: { toolCount: 0, findingCount: 0, result: 'ok' },
  };
}

describe('blockEventKind', () => {
  it('classifies blocks', () => {
    expect(blockEventKind(tool('terminal'))).toBe('command');
    expect(blockEventKind(tool('finding'))).toBe('finding');
    expect(blockEventKind(tool('read'))).toBe('tool');
    expect(blockEventKind({ kind: 'assistant', content: 'hi' })).toBe('said');
    expect(blockEventKind({ kind: 'thinking', text: 'x', provider: 'p' })).toBe('thinking');
    expect(blockEventKind({ kind: 'user', content: 'u' })).toBe('user');
    expect(blockEventKind({ kind: 'notice', noticeKind: 'k', message: 'm' })).toBe('quiet');
  });
});

describe('blockMatchesFilter', () => {
  it('matches only the matching kind; all always', () => {
    const said: TurnBlock = { kind: 'assistant', content: 'hi' };
    expect(blockMatchesFilter(said, 'all')).toBe(true);
    expect(blockMatchesFilter(said, 'said')).toBe(true);
    expect(blockMatchesFilter(said, 'thinking')).toBe(false);
    expect(blockMatchesFilter(said, 'commands')).toBe(false);
    expect(blockMatchesFilter(said, 'findings')).toBe(false);
    expect(blockMatchesFilter(tool('terminal'), 'commands')).toBe(true);
    expect(blockMatchesFilter(tool('finding'), 'findings')).toBe(true);
    expect(blockMatchesFilter(tool('read'), 'commands')).toBe(false);
  });
});

describe('filterCounts', () => {
  it('totals across turns', () => {
    const turns = [
      turn([{ kind: 'thinking', text: 't', provider: 'p' }, { kind: 'assistant', content: 'a' }]),
      turn([tool('terminal'), tool('finding'), tool('read')]),
    ];
    expect(filterCounts(turns)).toEqual({ said: 1, thinking: 1, commands: 1, findings: 1 });
  });
});
