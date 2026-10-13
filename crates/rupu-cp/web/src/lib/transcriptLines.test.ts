import { describe, it, expect } from 'vitest';
import { skillIndexLine, skillLoadLine, toolGrantLine } from './transcriptLines';

// Same inputs and expected strings as the Rust tests in
// `crates/rupu-transcript/src/{grant,skills}.rs`: change both together.

describe('toolGrantLine (port of rupu_transcript::grant::tool_grant_line)', () => {
  it('lists the offered tools, then what was narrowed away or unavailable', () => {
    expect(
      toolGrantLine(
        [{ tool: 'bash' }, { tool: 'read_file' }],
        ['scm.prs.get'],
        [{ tool: 'board.post', missing: ['message_bus'] }],
      ),
    ).toBe(
      '2 tools: bash, read_file  ·  narrowed by actions: scm.prs.get  ·  unavailable: board.post (needs message_bus)',
    );
    const many = Array.from({ length: 14 }, (_, i) => ({ tool: `t${i}` }));
    expect(toolGrantLine(many, [], [])).toBe(
      '14 tools: t0, t1, t2, t3, t4, t5, t6, t7, t8, t9, t10, t11 (+2 more)',
    );
    expect(toolGrantLine([], [], [])).toBe('0 tools');
    expect(toolGrantLine([{ tool: 'grep' }], [], [])).toBe('1 tool: grep');
  });
});

describe('skillIndexLine (port of rupu_transcript::skills::skill_index_line)', () => {
  const e = (name: string, preload: boolean, indexed: boolean) => ({ name, preload, indexed });
  it('marks preloaded and unindexed skills', () => {
    expect(skillIndexLine([e('a', true, false), e('b', false, true), e('c', false, false)])).toBe(
      '3 skills: a (preloaded), b, c (not indexed)',
    );
    expect(skillIndexLine([e('a', false, true)])).toBe('1 skill: a');
    expect(skillIndexLine([])).toBe('0 skills');
  });
});

describe('skillLoadLine (port of rupu_transcript::skills::skill_load_line)', () => {
  it('shows a 12-character hash, or none when the load recorded none', () => {
    expect(skillLoadLine('pdf', 'model', 'ab'.repeat(32))).toBe(
      'loaded skill pdf (abababababab) via model',
    );
    expect(skillLoadLine('pdf', 'preload', '')).toBe('loaded skill pdf via preload');
  });
});
