// @vitest-environment jsdom
// useInbox's Retry (operator MCP Plan 3b, UI-1): a failed row's retry is
// pending, not optimistic — it stays `retryRequested` until a reload no longer
// lists the run as failed — and a refused retry keeps its message per row.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api, ApiError, type RunListRow } from '../api';
import { runsApi } from '../api3b/runs';
import { useInbox } from './useInbox';

afterEach(() => {
  vi.restoreAllMocks();
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
});
