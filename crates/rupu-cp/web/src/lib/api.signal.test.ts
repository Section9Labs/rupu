/**
 * The per-host list engine aborts a request it stops waiting for (a host that
 * never answers, or a filter change). That only closes the connection if the
 * api call actually hands the signal to `fetch`; otherwise abandoned requests
 * pile up against the browser's per-origin connection limit.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';

afterEach(() => {
  vi.unstubAllGlobals();
});

const EMPTY_USAGE = JSON.stringify({
  summary: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 0 },
  breakdown: [],
  unpriced: { models: [], rows: 0 },
  hosts: [],
});

function stubFetch(body: string) {
  const fetchMock = vi.fn(async (_path: string, _init?: RequestInit) => new Response(body, { status: 200 }));
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

const CASES: [string, (signal: AbortSignal) => Promise<unknown>, string][] = [
  ['getRuns', (signal) => api.getRuns({ host: 'local', signal }), '[]'],
  ['getWorkflowRuns', (signal) => api.getWorkflowRuns({ host: 'local', signal }), '[]'],
  ['getAutoflowRuns', (signal) => api.getAutoflowRuns({ host: 'local', signal }), '[]'],
  ['getAutoflowEvents', (signal) => api.getAutoflowEvents({ host: 'local', signal }), '[]'],
  ['getAgentRuns', (signal) => api.getAgentRuns({ host: 'local', signal }), '[]'],
  ['getSessions', (signal) => api.getSessions({ host: 'local', signal }), '[]'],
  ['getUsage', (signal) => api.getUsage(undefined, 'model', 'local', signal), EMPTY_USAGE],
];

describe('list + usage calls forward an AbortSignal to fetch', () => {
  it.each(CASES)('%s', async (_name, call, body) => {
    const fetchMock = stubFetch(body);
    const controller = new AbortController();
    await call(controller.signal);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][1]?.signal).toBe(controller.signal);
  });

  it('a call without a signal still sends none (existing callers are unchanged)', async () => {
    const fetchMock = stubFetch('[]');
    await api.getRuns({ host: 'local' });
    expect(fetchMock.mock.calls[0][1]?.signal).toBeUndefined();
  });

  it('an aborted signal rejects the call rather than answering', async () => {
    // Mirrors what a real fetch does with an aborted signal.
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_path: string, init?: RequestInit) => {
        if (init?.signal?.aborted) throw new DOMException('aborted', 'AbortError');
        return new Response('[]', { status: 200 });
      }),
    );
    const controller = new AbortController();
    controller.abort();
    await expect(api.getRuns({ signal: controller.signal })).rejects.toThrow(/aborted/);
  });
});
