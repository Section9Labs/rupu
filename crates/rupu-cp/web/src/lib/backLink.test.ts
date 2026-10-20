import { describe, expect, it } from 'vitest';
import { backLabel, readBackState } from './backLink';

describe('backLabel', () => {
  it('names the page a link came from by its route', () => {
    expect(backLabel('/projects/ws1/findings')).toBe('Project');
    expect(backLabel('/runs/run_1')).toBe('Run');
    expect(backLabel('/agentiflows/af_1')).toBe('Flow');
    expect(backLabel('/sessions/ses_1')).toBe('Session');
    expect(backLabel('/coverage/t-1')).toBe('Coverage');
    expect(backLabel('/findings/fnd_1')).toBe('Finding');
    expect(backLabel('/findings')).toBe('Findings');
    expect(backLabel('/security')).toBe('Security');
    expect(backLabel('/transcript')).toBe('Transcript');
  });

  it('falls back to a plain Back for any other page', () => {
    expect(backLabel('/overview')).toBe('Back');
    expect(backLabel('/')).toBe('Back');
  });
});

describe('readBackState', () => {
  it('reads a well-formed back entry, keeping the origin state it carries', () => {
    const inner = { back: { to: '/projects/ws1', label: 'Project' } };
    expect(readBackState({ back: { to: '/runs/run_1?tab=findings#x', label: 'Run', state: inner } }))
      .toEqual({ to: '/runs/run_1?tab=findings#x', label: 'Run', state: inner });
  });

  it('rejects state that is not a back entry', () => {
    expect(readBackState(undefined)).toBeUndefined();
    expect(readBackState(null)).toBeUndefined();
    expect(readBackState({ other: 1 })).toBeUndefined();
    expect(readBackState({ back: 'nope' })).toBeUndefined();
    expect(readBackState({ back: { to: 'https://elsewhere.example', label: 'Run' } })).toBeUndefined();
    expect(readBackState({ back: { to: '/runs/run_1' } })).toBeUndefined();
    expect(readBackState({ back: { to: '/runs/run_1', label: '' } })).toBeUndefined();
  });
});
