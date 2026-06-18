/**
 * Unit tests for the rupu API client.
 * Tests the `request` wrapper via the exported `api` object.
 * EventSource subscribe helpers are not covered here (fiddly to mock in jsdom).
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { api, ApiError } from './api';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function mockFetch(status: number, body: unknown): void {
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue({
      ok: status >= 200 && status < 300,
      status,
      statusText: status === 200 ? 'OK' : 'Not Found',
      text: () => Promise.resolve(text),
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------
// request wrapper — success path
// ---------------------------------------------------------------------------

describe('api.getDashboard', () => {
  it('resolves typed on 200 JSON', async () => {
    const payload = {
      runs: { total: 3, by_status: { pending: 1, running: 1, completed: 1, failed: 0, awaiting_approval: 0, rejected: 0 } },
      recent_runs: [{ id: 'r1', workflow_name: 'wf', status: 'running', started_at: '2026-01-01T00:00:00Z' }],
      sessions: { total: 2, active: 1, archived: 1 },
      workers: { total: 1 },
      coverage: { targets: 5, assertions: 42 },
    };
    mockFetch(200, payload);

    const result = await api.getDashboard();
    expect(result.runs.total).toBe(3);
    expect(result.recent_runs[0].id).toBe('r1');
    expect(result.workers.total).toBe(1);
  });
});

describe('api.getRuns', () => {
  it('returns an array of RunRecord on 200', async () => {
    const runs = [
      { id: 'abc', workflow_name: 'demo', status: 'completed', started_at: '2026-01-01T00:00:00Z' },
    ];
    mockFetch(200, runs);

    const result = await api.getRuns();
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('abc');
    expect(result[0].status).toBe('completed');
  });
});

describe('api.getRun', () => {
  it('resolves run + steps on 200', async () => {
    const payload = {
      run: { id: 'r1', workflow_name: 'wf', status: 'running', started_at: '2026-01-01T00:00:00Z' },
      steps: [{ run_id: 'r1', step_id: 'classify', success: true }],
    };
    mockFetch(200, payload);

    const result = await api.getRun('r1');
    expect(result.run.id).toBe('r1');
    expect(result.steps[0].step_id).toBe('classify');
  });
});

// ---------------------------------------------------------------------------
// request wrapper — error path
// ---------------------------------------------------------------------------

describe('ApiError', () => {
  it('is thrown with status 404 on a not-found response', async () => {
    mockFetch(404, 'not found');

    await expect(api.getRuns()).rejects.toThrow(ApiError);
    await expect(api.getRuns()).rejects.toMatchObject({ status: 404 });
  });

  it('is thrown with status 500 on server error', async () => {
    mockFetch(500, 'internal server error');

    await expect(api.getDashboard()).rejects.toThrow(ApiError);
    await expect(api.getDashboard()).rejects.toMatchObject({ status: 500 });
  });

  it('carries the response body text', async () => {
    mockFetch(422, 'invalid input');

    try {
      await api.getAgents();
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).body).toBe('invalid input');
    }
  });
});

// ---------------------------------------------------------------------------
// 204 No Content → undefined (not a parse error)
// ---------------------------------------------------------------------------

describe('204 No Content', () => {
  it('resolves to undefined without throwing', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 204,
        statusText: 'No Content',
        text: () => Promise.resolve(''),
      }),
    );

    // Use getWorkers() as a convenient typed call; result will be undefined
    const result = await api.getWorkers();
    expect(result).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// URL encoding — paths with slashes / special chars
// ---------------------------------------------------------------------------

describe('encodeURIComponent in paths', () => {
  it('encodes run id in getRun', async () => {
    mockFetch(200, { run: { id: 'r/1', workflow_name: 'wf', status: 'pending', started_at: '' }, steps: [] });
    const fetchSpy = vi.mocked(fetch);
    await api.getRun('r/1');
    const calledUrl = (fetchSpy.mock.calls[0][0] as string);
    expect(calledUrl).toBe('/api/runs/r%2F1');
  });

  it('encodes coverage target id in getCoverageDetail', async () => {
    mockFetch(200, { target_id: 'foo/bar', assertions: [], findings: [] });
    const fetchSpy = vi.mocked(fetch);
    await api.getCoverageDetail('foo/bar');
    const calledUrl = (fetchSpy.mock.calls[0][0] as string);
    expect(calledUrl).toBe('/api/coverage/foo%2Fbar');
  });
});
