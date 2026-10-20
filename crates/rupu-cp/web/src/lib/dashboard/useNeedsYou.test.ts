// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi, beforeEach } from 'vitest';
import { act, cleanup, renderHook, waitFor } from '@testing-library/react';

vi.mock('../api', () => ({
  api: {
    getRegisteredHosts: vi.fn(),
    getWorkflowRuns: vi.fn(),
    approveRun: vi.fn(),
    rejectRun: vi.fn(),
    resumeRun: vi.fn(),
  },
  apiErrorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
}));

import { api } from '../api';
import { useNeedsYou } from './useNeedsYou';
import type { RunListRow } from '../api';

const row = (over: Partial<RunListRow>): RunListRow =>
  ({
    codename: 'sienna-beacon',
    codename_derived: false,
    id: 'r1',
    workflow_name: 'network-assessment',
    status: 'awaiting_approval',
    started_at: '2026-10-19T10:00:00Z',
    finished_at: null,
    trigger: 'manual',
    turns: 1,
    usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
    ...over,
  }) as RunListRow;

const mockApi = api as unknown as {
  getRegisteredHosts: ReturnType<typeof vi.fn>;
  getWorkflowRuns: ReturnType<typeof vi.fn>;
  approveRun: ReturnType<typeof vi.fn>;
  rejectRun: ReturnType<typeof vi.fn>;
  resumeRun: ReturnType<typeof vi.fn>;
};

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.getRegisteredHosts.mockResolvedValue([{ id: 'local', name: 'local', transport_kind: 'local' }]);
});
afterEach(cleanup);

describe('useNeedsYou', () => {
  it('merges blocking + failed and sorts blocking first (oldest wait on top)', async () => {
    mockApi.getWorkflowRuns.mockImplementation((p: { q?: string }) => {
      if (p.q?.includes('awaiting_approval')) {
        return Promise.resolve([
          row({ id: 'await-new', status: 'awaiting_approval', started_at: '2026-10-19T11:00:00Z' }),
          row({ id: 'await-old', status: 'awaiting_approval', started_at: '2026-10-19T09:00:00Z' }),
          row({ id: 'paused-1', status: 'paused', started_at: '2026-10-19T10:30:00Z' }),
        ]);
      }
      return Promise.resolve([row({ id: 'failed-1', status: 'failed', finished_at: '2026-10-19T12:00:00Z' })]);
    });

    const { result } = renderHook(() => useNeedsYou());
    await waitFor(() => expect(result.current.loading).toBe(false));

    const ids = result.current.items.map((i) => i.id);
    // awaiting (oldest first) → paused → failed
    expect(ids).toEqual(['await-old', 'await-new', 'paused-1', 'failed-1']);
    expect(result.current.items[0].kind).toBe('awaiting');
    expect(result.current.items[3].kind).toBe('failed');
  });

  it('a host error is non-fatal', async () => {
    mockApi.getWorkflowRuns.mockRejectedValue(new Error('boom'));
    const { result } = renderHook(() => useNeedsYou());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.items).toEqual([]);
    expect(result.current.error).toBeNull(); // per-request catch, not a page error
  });

  it('approve calls approveRun (local → no host arg) and refetches', async () => {
    mockApi.getWorkflowRuns.mockImplementation((p: { q?: string }) =>
      p.q?.includes('awaiting_approval')
        ? Promise.resolve([row({ id: 'gate-1', status: 'awaiting_approval' })])
        : Promise.resolve([]),
    );
    mockApi.approveRun.mockResolvedValue(undefined);

    const { result } = renderHook(() => useNeedsYou());
    await waitFor(() => expect(result.current.items.length).toBe(1));

    await act(async () => {
      result.current.approve(result.current.items[0]);
    });

    expect(mockApi.approveRun).toHaveBeenCalledWith('gate-1', undefined, undefined);
    // refetch fired (getRegisteredHosts called again after the mutation)
    await waitFor(() => expect(mockApi.getRegisteredHosts.mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it('resume on a remote paused run passes the host id', async () => {
    mockApi.getRegisteredHosts.mockResolvedValue([{ id: 'mini', name: 'mini', transport_kind: 'ssh' }]);
    mockApi.getWorkflowRuns.mockImplementation((p: { q?: string }) =>
      p.q?.includes('awaiting_approval')
        ? Promise.resolve([row({ id: 'p1', status: 'paused', host_id: 'mini' })])
        : Promise.resolve([]),
    );
    mockApi.resumeRun.mockResolvedValue(undefined);

    const { result } = renderHook(() => useNeedsYou());
    await waitFor(() => expect(result.current.items.length).toBe(1));

    await act(async () => {
      result.current.resume(result.current.items[0]);
    });
    expect(mockApi.resumeRun).toHaveBeenCalledWith('p1', 'mini');
  });
});
