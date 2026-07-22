import { describe, it, expect } from 'vitest';
import { resolveAgentUi } from './useAgentAuthoringUi';
describe('resolveAgentUi', () => {
  it('localStorage override wins', () => {
    expect(resolveAgentUi({ agent_authoring_ui: 'classic' }, 'next')).toBe('next');
    expect(resolveAgentUi({ agent_authoring_ui: 'next' }, 'classic')).toBe('classic');
  });
  it('falls back to server config when no override', () => {
    expect(resolveAgentUi({ agent_authoring_ui: 'next' }, null)).toBe('next');
  });
  it('defaults to classic when unset or unknown', () => {
    expect(resolveAgentUi(null, null)).toBe('classic');
    expect(resolveAgentUi({ agent_authoring_ui: 'bogus' }, null)).toBe('classic');
  });
});
