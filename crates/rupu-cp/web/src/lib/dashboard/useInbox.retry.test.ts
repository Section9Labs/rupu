// @vitest-environment jsdom
// useInbox's Retry (operator MCP Plan 3b, UI-1): a failed row's retry is
// pending, not optimistic — it stays `retryRequested` until a reload no longer
// lists the run as failed — and a refused retry keeps its message per row.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api, ApiError, type RunListRow } from '../api';
import { RETRY_STALL_MS, runsApi } from '../api3b/runs';
import { useInbox } from './useInbox';

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function row(id: string, status: RunListRow['status']): RunListRow {
  return {
    id,
    workflow_name: 'lint-sweep',
    status,
    started_at: '2026-10-20T10:00:00Z',
    finished_at: '2026-10-20T10:05:00Z',
    codename: 'cobalt-harbor',
    codename_derived: false,
  } as RunListRow;
}

/** The run record `GET /api/runs/:id` answers for a retried run. */
function stubRecord(rec: () => Partial<RunListRow> & { resume_requested_at?: string | null }) {
  return vi.spyOn(api, 'getRun').mockImplementation(async () => ({
    run: { ...row('run_f1', 'failed'), ...rec() } as never,
    steps: [],
    usage: undefined as never,
  }));
}

function stubRuns(failed: () => RunListRow[]) {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([{ id: 'local' }] as never);
  vi.spyOn(api, 'getWorkflowRuns').mockImplementation(async (p) =>
    p?.q === 'status:failed' ? failed() : [],
  );
}

describe('useInbox retry', () => {
  it('retries with the recorded mode and stays pending until the run leaves the failed list', async () => {
    let failed = [row('run_f1', 'failed')];
    stubRuns(() => failed);
    stubRecord(() => ({ resume_requested_at: '2026-10-20T10:06:00Z' }));
    const retry = vi
      .spyOn(runsApi, 'retryRun')
      .mockResolvedValue({ run_id: 'run_f1', retrying: true, restart_interrupted: false, host_id: 'local' });
    const { result } = renderHook(() => useInbox());
    await waitFor(() => expect(result.current.failures).toHaveLength(1));

    act(() => result.current.retry(result.current.failures[0]));
    await waitFor(() => expect(result.current.retryRequested).toEqual({ run_f1: true }));
    expect(retry).toHaveBeenCalledWith('run_f1', {});

    // Still failed on the next read: still pending.
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.retryRequested).toEqual({ run_f1: true });

    // The retry took the run: it leaves the list and the pending mark goes.
    failed = [];
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.failures).toHaveLength(0));
    expect(result.current.retryRequested).toEqual({});
  });

  it('keeps a refused retry\'s message on its row', async () => {
    stubRuns(() => [row('run_f1', 'failed')]);
    vi.spyOn(runsApi, 'retryRun').mockRejectedValue(
      new ApiError(409, 'conflict', JSON.stringify({ error: 'run run_f1 is `running`' })),
    );
    const { result } = renderHook(() => useInbox());
    await waitFor(() => expect(result.current.failures).toHaveLength(1));
    act(() => result.current.retry(result.current.failures[0]));
    await waitFor(() => expect(result.current.retryErrors).toEqual({ run_f1: 'run run_f1 is `running`' }));
    expect(result.current.retryRequested).toEqual({});
  });
  it('confirms from the run record: the marker consumed, even though the run failed again', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    // The retried run failed again before a reconcile: it never leaves the list.
    stubRuns(() => [row('run_f1', 'failed')]);
    let marker: string | null = '2026-10-20T10:06:00Z';
    stubRecord(() => ({ resume_requested_at: marker, finished_at: '2026-10-20T10:05:00Z' }));
    vi.spyOn(runsApi, 'retryRun').mockResolvedValue({ run_id: 'run_f1', retrying: true, restart_interrupted: false, host_id: 'local' });
    const { result } = renderHook(() => useInbox());
    await waitFor(() => expect(result.current.failures).toHaveLength(1));
    act(() => result.current.retry(result.current.failures[0]));
    await waitFor(() => expect(result.current.retryRequested).toEqual({ run_f1: true }));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(result.current.retryRequested).toEqual({ run_f1: true });

    marker = null;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(result.current.retryRequested).toEqual({});
    expect(result.current.retryErrors).toEqual({});
  });

  it('confirms from the failure time changing (it ran and failed again)', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    stubRuns(() => [row('run_f1', 'failed')]);
    let finished = '2026-10-20T10:05:00Z';
    stubRecord(() => ({ resume_requested_at: '2026-10-20T10:06:00Z', finished_at: finished }));
    vi.spyOn(runsApi, 'retryRun').mockResolvedValue({ run_id: 'run_f1', retrying: true, restart_interrupted: false, host_id: 'local' });
    const { result } = renderHook(() => useInbox());
    await waitFor(() => expect(result.current.failures).toHaveLength(1));
    act(() => result.current.retry(result.current.failures[0]));
    await waitFor(() => expect(result.current.retryRequested).toEqual({ run_f1: true }));
    finished = '2026-10-20T10:09:00Z';
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(result.current.retryRequested).toEqual({});
  });

  it('drops a retry that never started after the stall timeout, says so, and stops polling', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    stubRuns(() => [row('run_f1', 'failed')]);
    const getRun = stubRecord(() => ({ resume_requested_at: '2026-10-20T10:06:00Z', finished_at: '2026-10-20T10:05:00Z' }));
    vi.spyOn(runsApi, 'retryRun').mockResolvedValue({ run_id: 'run_f1', retrying: true, restart_interrupted: false, host_id: 'local' });
    const { result } = renderHook(() => useInbox());
    await waitFor(() => expect(result.current.failures).toHaveLength(1));
    act(() => result.current.retry(result.current.failures[0]));
    await waitFor(() => expect(result.current.retryRequested).toEqual({ run_f1: true }));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(RETRY_STALL_MS + 5_000);
    });
    expect(result.current.retryRequested).toEqual({});
    expect(result.current.retryErrors.run_f1).toMatch(/hasn’t started/);
    const calls = getRun.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(20_000);
    });
    expect(getRun.mock.calls.length).toBe(calls);
  });
});
