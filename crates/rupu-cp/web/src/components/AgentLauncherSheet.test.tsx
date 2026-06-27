import { describe, it, expect } from 'vitest';
import { buildAgentLaunch } from './AgentLauncherSheet';
import { WORKSPACE_ITEM, type TargetItem } from '../lib/targetItems';

describe('buildAgentLaunch', () => {
  it('directory item sends working_dir only', () => {
    const dirItem: TargetItem = { kind: 'directory', label: '/tmp/x', resolved: { working_dir: '/tmp/x' } };
    expect(buildAgentLaunch('hi', 'ask', dirItem)).toEqual({
      prompt: 'hi', mode: 'ask', working_dir: '/tmp/x',
    });
  });
  it('repo item sends target only; blank prompt omitted', () => {
    const repoItem: TargetItem = { kind: 'repo', label: 'github:o/r', resolved: { target: 'github:o/r' } };
    expect(buildAgentLaunch('  ', 'bypass', repoItem)).toEqual({
      mode: 'bypass', target: 'github:o/r',
    });
  });
  it('workspace item sends neither target nor dir', () => {
    expect(buildAgentLaunch('go', 'ask', WORKSPACE_ITEM)).toEqual({ prompt: 'go', mode: 'ask' });
  });
});
