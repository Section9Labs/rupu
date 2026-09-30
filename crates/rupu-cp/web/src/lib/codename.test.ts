import { describe, expect, it } from 'vitest';
import { parseCodename, crewTint, roleBadge, memberLabel } from './codename';
describe('codename', () => {
  it('parses crew, leaf and role', () => {
    expect(parseCodename('cobalt-harbor')).toEqual({ crew: 'cobalt-harbor', leaf: 'cobalt-harbor', role: undefined });
    expect(parseCodename('cobalt-harbor/heron#412>lynx#3')).toEqual({ crew: 'cobalt-harbor', leaf: 'heron#412>lynx#3', role: 'lynx' });
    expect(parseCodename('cobalt-harbor/heron#4.2').role).toBe('heron');
  });
  it('looks up tints and badges by theme', () => {
    expect(crewTint('cobalt-harbor', 'light')).toBe('#1d4ed8');
    expect(crewTint('cobalt-harbor', 'dark')).toBe('#93b4fd');
    expect(crewTint('nope-harbor', 'light')).toBeUndefined();
    expect(roleBadge('heron', 'light')?.shape).toBeTypeOf('string');
  });
  it('builds member labels', () => {
    expect(memberLabel('jade-reef/heron#4', 'security-reviewer', 'anthropic', 'claude-opus-5-5'))
      .toBe('heron#4 · security-reviewer · anthropic/claude-opus-5-5');
    expect(memberLabel(undefined, 'triage')).toBe('triage');
  });
});
