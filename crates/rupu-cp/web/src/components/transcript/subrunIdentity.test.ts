import { describe, it, expect } from 'vitest';
import { buildSubrunIdentities } from './subrunIdentity';
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
