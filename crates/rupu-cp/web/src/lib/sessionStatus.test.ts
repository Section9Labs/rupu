import { describe, expect, it } from 'vitest';
import {
  SESSION_STATUS_DESCRIPTOR,
  isSessionStatusValue,
  sessionStatusDot,
  sessionStatusLabel,
  sessionStatusTone,
} from './sessionStatus';

describe('sessionStatus — the rupu-cli 4-value vocabulary (session.rs:213-220)', () => {
  it('recognizes exactly idle/running/failed/stopped as the confirmed vocabulary', () => {
    expect(isSessionStatusValue('idle')).toBe(true);
    expect(isSessionStatusValue('running')).toBe(true);
    expect(isSessionStatusValue('failed')).toBe(true);
    expect(isSessionStatusValue('stopped')).toBe(true);
    expect(isSessionStatusValue('active')).toBe(false);
    expect(isSessionStatusValue('completed')).toBe(false);
  });

  // Real bug fix: previously `failed` matched none of the substring checks
  // and silently fell through to `neutral` — visually indistinguishable from
  // `stopped`. It must now resolve to its own tone.
  it('a "failed" session status resolves to the failed tone, not neutral', () => {
    expect(sessionStatusTone('failed')).toBe('failed');
    expect(sessionStatusDot('failed')).toContain('bg-status-failed');
    expect(sessionStatusDot('failed')).not.toBe(sessionStatusDot('stopped'));
  });

  it('preserves the session-native labels (idle/running/failed/stopped) unrenamed', () => {
    expect(sessionStatusLabel('idle')).toBe('idle');
    expect(sessionStatusLabel('running')).toBe('running');
    expect(sessionStatusLabel('failed')).toBe('failed');
    expect(sessionStatusLabel('stopped')).toBe('stopped');
  });

  it('SESSION_STATUS_DESCRIPTOR keeps session words, never run words, for the label', () => {
    expect(SESSION_STATUS_DESCRIPTOR.idle.label).toBe('Idle');
    expect(SESSION_STATUS_DESCRIPTOR.idle.label).not.toBe('Completed');
    expect(SESSION_STATUS_DESCRIPTOR.running.label).toBe('Running');
    expect(SESSION_STATUS_DESCRIPTOR.failed.label).toBe('Failed');
    expect(SESSION_STATUS_DESCRIPTOR.stopped.label).toBe('Stopped');
    expect(SESSION_STATUS_DESCRIPTOR.stopped.label).not.toBe('Cancelled');
  });

  it('a running session dot carries the same shared motion class the pill uses', () => {
    expect(sessionStatusDot('running')).toContain('rg-pulse-run');
  });

  it('terminal/resting session tones (idle/failed/stopped) carry no motion class', () => {
    expect(sessionStatusDot('idle')).not.toContain('rg-pulse');
    expect(sessionStatusDot('failed')).not.toContain('rg-pulse');
    expect(sessionStatusDot('stopped')).not.toContain('rg-pulse');
  });

  it('legacy/fuzzy inputs outside the confirmed vocabulary still resolve sensibly', () => {
    expect(sessionStatusTone('active')).toBe('running');
    expect(sessionStatusTone('archived')).toBe('stopped');
    expect(sessionStatusTone('errored')).toBe('failed');
    expect(sessionStatusTone(null)).toBe('neutral');
  });
});
