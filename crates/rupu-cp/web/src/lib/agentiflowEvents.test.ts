// agentiflowEventCards — maps agentiflow lifecycle events to the shared
// Situation Room StreamCards (newest-first), tinted by the lead's crew, so the
// Events tab renders through RunEventFeed / EventCard like everywhere else.

import { describe, it, expect } from 'vitest';
import { agentiflowEventCards } from './agentiflowEvents';
import type { AgentiflowEvent } from './api';

const EVENTS: AgentiflowEvent[] = [
  { ts: '2026-10-07T15:00:00Z', kind: 'run_started', goals: 1, engagement_profiles: ['network'] },
  { ts: '2026-10-07T15:01:00Z', kind: 'round', round: 0, budget: 'ok', goals_met: 0, goals_total: 1, spent_usd: 1.5, spent_tokens: 1000 },
  { ts: '2026-10-07T15:02:00Z', kind: 'round', round: 1, outcome: 'error', error: 'provider: API error 401' },
  { ts: '2026-10-07T15:03:00Z', kind: 'run_stopped', stop_reason: 'goals_met' },
];

describe('agentiflowEventCards', () => {
  const cards = agentiflowEventCards(EVENTS, 'cobalt-harbor/heron#1');

  it('returns one card per event, newest-first, crew-tinted', () => {
    expect(cards).toHaveLength(4);
    expect(cards.map((c) => c.badge)).toEqual(['Stopped', 'Round 1', 'Round 0', 'Started']);
    expect(cards.every((c) => c.crew === 'cobalt-harbor')).toBe(true);
  });

  it('maps run_stopped to a lifecycle card with the stop reason', () => {
    const stopped = cards[0];
    expect(stopped.form).toBe('lifecycle');
    expect(stopped.title).toBe('Engagement stopped');
    expect(stopped.detail).toBe('goals_met');
    expect(stopped.accent).toBe('brand'); // goals_met → ok → brand
  });

  it('maps a failed round to an error card carrying the error', () => {
    const errRound = cards[1];
    expect(errRound.form).toBe('error');
    expect(errRound.group).toBe('error');
    expect(errRound.accent).toBe('error');
    expect(errRound.title).toBe('Round 1 — failed');
    expect(errRound.detail).toBe('provider: API error 401');
  });

  it('maps an ok round to an activity card summarizing goals/budget/spend', () => {
    const okRound = cards[2];
    expect(okRound.form).toBe('activity');
    expect(okRound.accent).toBe('brand');
    expect(okRound.detail).toContain('goals 0/1');
    expect(okRound.detail).toContain('ok');
    expect(okRound.detail).toContain('$1.50');
  });

  it('maps run_started with its profiles and goal count', () => {
    const started = cards[3];
    expect(started.title).toBe('Engagement started');
    expect(started.detail).toBe('profiles network · 1 goal');
  });
});

describe('agentiflowEventCards — unit lifecycle', () => {
  const cards = agentiflowEventCards(
    [
      { ts: '2026-10-07T15:00:30Z', kind: 'af_unit_started', unit_id: 'u1', agent: 'recon', codename: 'cobalt-harbor/elk#1', participant: 'recon#1', unit_kind: 'agent' },
      { ts: '2026-10-07T15:02:00Z', kind: 'af_unit_completed', unit_id: 'u1', agent: 'recon', codename: 'cobalt-harbor/elk#1', success: true, input_tokens: 1200, output_tokens: 300, provider: 'anthropic', model: 'claude-mythos-5', output: 'found a thing' },
      { ts: '2026-10-07T15:03:00Z', kind: 'af_unit_completed', unit_id: 'u2', agent: 'triage', codename: 'cobalt-harbor/lynx#1', success: false, error: 'spawn failed' },
    ],
    'cobalt-harbor/heron#1',
  );

  it('maps unit started/completed to crew-tinted cards, newest-first', () => {
    expect(cards.map((c) => c.badge)).toEqual(['Failed', 'Done', 'Dispatched']);
  });

  it('carries tokens, provider and codename on a completed unit', () => {
    const done = cards[1];
    expect(done.form).toBe('complete');
    expect(done.accent).toBe('brand');
    expect(done.codename).toBe('cobalt-harbor/elk#1');
    expect(done.crew).toBe('cobalt-harbor');
    expect(done.tokensIn).toBe(1200);
    expect(done.tokensOut).toBe(300);
    expect(done.provider).toBe('anthropic');
    expect(done.detail).toBe('found a thing');
  });

  it('renders a failed unit as an error card with its error', () => {
    const failed = cards[0];
    expect(failed.form).toBe('error');
    expect(failed.accent).toBe('error');
    expect(failed.detail).toBe('spawn failed');
  });
});
