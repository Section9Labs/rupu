import { describe, it, expect } from 'vitest';
import { isSessionActive, pollIntervalFor } from './sessionPoll';

describe('isSessionActive', () => {
  it('true when status is running', () => {
    expect(isSessionActive({ status: 'running', active_run_id: null })).toBe(true);
  });
  it('true when active_run_id is set (regardless of status)', () => {
    expect(isSessionActive({ status: 'idle', active_run_id: 'run_x' })).toBe(true);
  });
  it('false when idle with no active_run_id', () => {
    expect(isSessionActive({ status: 'idle', active_run_id: null })).toBe(false);
  });
  it('false when failed', () => {
    expect(isSessionActive({ status: 'failed', active_run_id: null })).toBe(false);
  });
  it('false when null', () => {
    expect(isSessionActive(null)).toBe(false);
  });
});

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
