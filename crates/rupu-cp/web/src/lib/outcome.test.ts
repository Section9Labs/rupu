import { describe, it, expect } from 'vitest';
import { assistantBlockLine, recoveryLine } from './outcome';

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

describe('assistantBlockLine (port of rupu_transcript::outcome::assistant_block_line)', () => {
  it('names a fallback boundary, an unrecognized block and an abandoned block like the Rust formatter', () => {
    expect(assistantBlockLine({ type: 'fallback', from_model: 'model-a', to_model: 'model-b' }, false)).toBe(
      'served by fallback · model-a → model-b',
    );
    expect(
      assistantBlockLine({ type: 'unknown', provider: 'anthropic', raw: { type: 'lantern_note', n: 1 } }, false),
    ).toBe('unrecognized block · lantern_note {"n":1,"type":"lantern_note"}');
    expect(assistantBlockLine({ type: 'tool_use', id: 'c1', name: 'bash', input: {} }, true)).toBe(
      'abandoned · tool call · bash',
    );
    expect(assistantBlockLine({ type: 'reasoning', provider: 'anthropic', model: 'm', raw: {} }, true)).toBe(
      'abandoned · reasoning',
    );
  });

  it('cuts a long raw payload at 200 characters', () => {
    const s = assistantBlockLine({ type: 'unknown', raw: { type: 'x', pad: 'y'.repeat(400) } }, false);
    expect(s.endsWith('…')).toBe(true);
    expect(Array.from(s).length).toBeLessThanOrEqual('unrecognized block · x '.length + 200);
  });
});
