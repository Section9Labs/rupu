import { describe, it, expect } from 'vitest';
import { buildAgentLaunch } from './AgentLauncherSheet';

describe('buildAgentLaunch', () => {
  it('directory mode sends working_dir only', () => {
    expect(buildAgentLaunch('hi', 'ask', 'directory', 'github:o/r', '/tmp/x')).toEqual({
      prompt: 'hi', mode: 'ask', working_dir: '/tmp/x',
    });
  });
  it('repo mode sends target only; blank prompt omitted', () => {
    expect(buildAgentLaunch('  ', 'bypass', 'repo', 'github:o/r', '')).toEqual({
      mode: 'bypass', target: 'github:o/r',
    });
  });
  it('workspace mode sends neither target nor dir', () => {
    expect(buildAgentLaunch('go', 'ask', 'workspace', '', '')).toEqual({ prompt: 'go', mode: 'ask' });
  });
});
