import { describe, it, expect } from 'vitest';
import { buildSubrunIdentities, sameSubrunIdentities } from './subrunIdentity';
import type { RunEvent } from '../../lib/api';

describe('buildSubrunIdentities', () => {
  it('maps dispatch_started sub_run_id → codename/agent/provider/model, ignoring other events', () => {
    const events: RunEvent[] = [
      { type: 'run_started', run_id: 'r', event_version: 1, workflow_path: 'w', started_at: 'x' },
      {
        type: 'dispatch_started', run_id: 'r', sub_run_id: 'sub_1', agent: 'scanner',
        transcript_path: '/t/s.jsonl', codename: 'jade-reef/lead>lynx#1', provider: 'anthropic', model: 'claude-opus-5-5',
      },
      { type: 'dispatch_started', run_id: 'r', sub_run_id: 'sub_2', transcript_path: '/t/2.jsonl' },
    ];
    const m = buildSubrunIdentities(events);
    expect(m.get('sub_1')).toEqual({
      codename: 'jade-reef/lead>lynx#1', agent: 'scanner', provider: 'anthropic', model: 'claude-opus-5-5',
    });
    expect(m.get('sub_2')).toEqual({});
    expect(m.size).toBe(2);
  });
});

describe('buildSubrunIdentities — seeded from the graph response', () => {
  it('seed alone resolves a sub-run with no live events', () => {
    const m = buildSubrunIdentities([], { sub_1: { codename: 'jade-reef/lead>lynx#1', agent: 'scanner', provider: 'anthropic', model: 'opus' } });
    expect(m.get('sub_1')).toEqual({ codename: 'jade-reef/lead>lynx#1', agent: 'scanner', provider: 'anthropic', model: 'opus' });
  });

  it('live dispatch_started layers over the seed (live wins, absent fields kept)', () => {
    const events: RunEvent[] = [
      { type: 'dispatch_started', run_id: 'r', sub_run_id: 'sub_1', transcript_path: '/t', model: 'opus-2' },
    ];
    const m = buildSubrunIdentities(events, { sub_1: { agent: 'scanner', provider: 'anthropic', model: 'opus' } });
    expect(m.get('sub_1')).toEqual({ agent: 'scanner', provider: 'anthropic', model: 'opus-2' });
  });

  it('sameSubrunIdentities compares by content', () => {
    const a = buildSubrunIdentities([], { s: { agent: 'x' } });
    const b = buildSubrunIdentities([], { s: { agent: 'x' } });
    expect(sameSubrunIdentities(a, b)).toBe(true);
    expect(sameSubrunIdentities(a, buildSubrunIdentities([], { s: { agent: 'y' } }))).toBe(false);
  });
});
