import { describe, it, expect } from 'vitest';
import { pollIntervalFor } from './sessionPoll';

describe('pollIntervalFor', () => {
  it('fast while active (running or has active_run_id)', () => {
    expect(pollIntervalFor({ status: 'running', active_run_id: null })).toBe(1500);
    expect(pollIntervalFor({ status: 'idle', active_run_id: 'run_x' })).toBe(1500);
  });
  it('slow when idle/terminal', () => {
    expect(pollIntervalFor({ status: 'idle', active_run_id: null })).toBe(5000);
    expect(pollIntervalFor({ status: 'failed', active_run_id: null })).toBe(5000);
    expect(pollIntervalFor(null)).toBe(5000);
  });
});
