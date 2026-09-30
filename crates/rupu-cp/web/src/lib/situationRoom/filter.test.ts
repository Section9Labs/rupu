import { describe, it, expect } from 'vitest';
import { filterStreamCards } from './filter';
import type { StreamCard } from './cards';

function card(over: Partial<StreamCard>): StreamCard {
  return {
    key: over.key ?? Math.random().toString(36),
    ts: over.ts ?? 0,
    form: over.form ?? 'activity',
    group: over.group ?? 'activity',
    accent: over.accent ?? 'brand',
    badge: over.badge ?? 'Working',
    title: over.title ?? 'a step',
    ...over,
  } as StreamCard;
}

const cards: StreamCard[] = [
  card({ key: 'a', group: 'activity', title: 'oracle-assessor scanning', agent: 'oracle-assessor' }),
  card({ key: 'f', group: 'finding', title: 'SQL injection', severity: 'high', filePath: 'src/db/query.rs' }),
  card({ key: 'e', group: 'error', title: 'Workflow run failed', detail: 'boom', runId: 'run_ABC123' }),
  card({ key: 'w', group: 'await', title: 'Approval needed', badge: 'AWAITING YOU' }),
];

describe('filterStreamCards', () => {
  it('group "all" with empty query returns every card in order', () => {
    expect(filterStreamCards(cards, 'all', '').map((c) => c.key)).toEqual(['a', 'f', 'e', 'w']);
  });

  it('narrows to a single group', () => {
    expect(filterStreamCards(cards, 'finding', '').map((c) => c.key)).toEqual(['f']);
    expect(filterStreamCards(cards, 'error', '').map((c) => c.key)).toEqual(['e']);
  });

  it('matches the query case-insensitively across title/agent', () => {
    expect(filterStreamCards(cards, 'all', 'ORACLE').map((c) => c.key)).toEqual(['a']);
    expect(filterStreamCards(cards, 'all', 'injection').map((c) => c.key)).toEqual(['f']);
  });

  it('matches run id and file path', () => {
    expect(filterStreamCards(cards, 'all', 'run_abc').map((c) => c.key)).toEqual(['e']);
    expect(filterStreamCards(cards, 'all', 'query.rs').map((c) => c.key)).toEqual(['f']);
  });

  it('combines group and query', () => {
    expect(filterStreamCards(cards, 'activity', 'scanning').map((c) => c.key)).toEqual(['a']);
    expect(filterStreamCards(cards, 'finding', 'scanning')).toEqual([]);
  });

  it('treats a whitespace-only query as no query', () => {
    expect(filterStreamCards(cards, 'all', '   ').map((c) => c.key)).toEqual(['a', 'f', 'e', 'w']);
  });

  it('returns nothing when the query matches no card', () => {
    expect(filterStreamCards(cards, 'all', 'zzz-nope')).toEqual([]);
  });
});

describe('filterStreamCards — codenames', () => {
  const named: StreamCard[] = [
    card({ key: 'n', title: 'review', codename: 'jade-reef/heron#4', crew: 'jade-reef', provider: 'anthropic', model: 'claude-sonnet-4-6' }),
    card({ key: 'o', title: 'other' }),
  ];
  it('matches a codename leaf word, the crew, and provider/model', () => {
    expect(filterStreamCards(named, 'all', 'heron').map((c) => c.key)).toEqual(['n']);
    expect(filterStreamCards(named, 'all', 'jade-reef').map((c) => c.key)).toEqual(['n']);
    expect(filterStreamCards(named, 'all', 'sonnet').map((c) => c.key)).toEqual(['n']);
    expect(filterStreamCards(named, 'all', 'anthropic').map((c) => c.key)).toEqual(['n']);
  });
});

