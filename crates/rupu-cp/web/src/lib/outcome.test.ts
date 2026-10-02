import { describe, it, expect } from 'vitest';
import { recoveryLine } from './outcome';

describe('recoveryLine (port of rupu_transcript::outcome::recovery_line)', () => {
  const base = { rung: 1 };
  it('phrases every action like the Rust formatter', () => {
    expect(recoveryLine({ ...base, action: 'continued', attempt: 2, budget: 3 })).toBe('↺ rung 1 · continued 2/3');
    expect(recoveryLine({ ...base, action: 'continued' })).toBe('↺ rung 1 · continued');
    expect(recoveryLine({ ...base, action: 'retried' })).toBe('↺ rung 1 · retried');
    expect(recoveryLine({ ...base, action: 'compacted' })).toBe('↺ rung 1 · compacted');
    expect(recoveryLine({ ...base, action: 'fell_back', provider: 'anthropic', model: 'claude-opus-4-8' })).toBe(
      '↺ rung 1 · fell back to anthropic/claude-opus-4-8',
    );
    expect(recoveryLine({ ...base, action: 'served_by_fallback', model: 'claude-opus-5' })).toBe(
      '↺ rung 1 · served by claude-opus-5',
    );
    expect(recoveryLine({ ...base, action: 'skipped', provider: 'openai', model: 'm1', reason: 'no credentials' })).toBe(
      '↺ rung 1 · skipped openai/m1: no credentials',
    );
    expect(recoveryLine({ ...base, action: 'failed' })).toBe('↺ rung 1 · no recovery left');
    expect(recoveryLine({ ...base, action: 'asked' })).toBe('↺ rung 1 · asked');
    expect(recoveryLine({ ...base, action: 'parked' })).toBe('↺ rung 1 · parked');
    expect(recoveryLine({ ...base, action: 'teleported' })).toBe('↺ rung 1 · teleported');
  });
});
