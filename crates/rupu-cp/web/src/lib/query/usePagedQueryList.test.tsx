// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, renderHook, waitFor } from '@testing-library/react';
import { ApiError } from '../api';
import { usePagedQueryList } from './usePagedQueryList';
import { WORKFLOW_RUN_FIELDS } from './lists/workflowRuns';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

type Fetch = Parameters<typeof usePagedQueryList<{ id: string }>>[0]['fetch'];

function useList(q: string, fetch: Fetch) {
  return usePagedQueryList<{ id: string }>({ q, fields: WORKFLOW_RUN_FIELDS, scope: ['ws_1'], fetch });
}

describe('usePagedQueryList', () => {
  it('sends q, and asks for facets on the first page of a scope only', async () => {
    const fetch = vi.fn<Fetch>(async (p) => {
      p.onFacets?.({ facets: { status: { failed: 2 } }, partial: false });
      return [{ id: 'a' }];
    });
    const { result, rerender } = renderHook(({ q }) => useList(q, fetch), { initialProps: { q: '' } });
    await waitFor(() => expect(result.current.rows).toEqual([{ id: 'a' }]));
    expect(result.current.facets.status).toEqual([{ value: 'failed', count: 2 }]);
    rerender({ q: 'status:failed' });
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
    expect(fetch.mock.calls.map((c) => [c[0].q, c[0].offset, c[0].onFacets !== undefined])).toEqual([
      ['', 0, true],
      ['status:failed', 0, false],
    ]);
  });

  it('asks nothing while the query doesn’t parse', async () => {
    const fetch = vi.fn<Fetch>(async () => [{ id: 'a' }]);
    const { result } = renderHook(() => useList('colour:red', fetch));
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(fetch).not.toHaveBeenCalled();
    expect(result.current.rows).toEqual([]);
    expect(result.current.queryError).toBe('unknown key `colour` (quote the text to search for it)');
  });

  it('reads a 400 parse error as the query’s error and no rows, not a list error', async () => {
    const fetch = vi.fn<Fetch>(() =>
      Promise.reject(new ApiError(400, 'x', JSON.stringify({ error: 'server says no', token: 0, code: 'bad_value', start: 0, end: 3 }))),
    );
    const { result } = renderHook(() => useList('status:failed', fetch));
    await waitFor(() => expect(result.current.queryError).toBe('server says no'));
    expect(result.current.error).toBeNull();
    expect(result.current.rows).toEqual([]);
  });
});
