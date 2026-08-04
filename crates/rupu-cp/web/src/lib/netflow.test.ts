import { describe, expect, it, vi } from 'vitest';
import { ApiError } from './api';
import { fetchNetflowGraph, fetchRunNetflow, formatBytes } from './netflow';

describe('formatBytes', () => {
  it('renders unknown as an em dash, never as 0 B', () => {
    // The distinction the whole Fidelity system exists to preserve:
    // "we could not see it" is not "it was zero".
    expect(formatBytes(null)).toBe('—');
    expect(formatBytes(undefined)).toBe('—');
    expect(formatBytes(0)).toBe('0 B');
  });

  it('scales to KB and MB', () => {
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });
});

describe('fetch helpers', () => {
  it('encodes the run id into the path', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ flows: [], hosts: [], dropped: 0, asn_loaded: true }),
    });
    vi.stubGlobal('fetch', fetchMock);

    await fetchRunNetflow('run/with slash');
    expect(fetchMock).toHaveBeenCalledWith('/api/runs/run%2Fwith%20slash/netflow');
    vi.unstubAllGlobals();
  });

  it('omits the scope parameter entirely when global', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ nodes: [], edges: [] }),
    });
    vi.stubGlobal('fetch', fetchMock);

    await fetchNetflowGraph();
    expect(fetchMock).toHaveBeenCalledWith('/api/netflow/graph');
    vi.unstubAllGlobals();
  });

  it('includes the scope parameter when given', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ nodes: [], edges: [] }),
    });
    vi.stubGlobal('fetch', fetchMock);

    await fetchNetflowGraph('project:ws_1');
    expect(fetchMock).toHaveBeenCalledWith('/api/netflow/graph?scope=project%3Aws_1');
    vi.unstubAllGlobals();
  });

  it('throws on a non-ok response rather than returning junk', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 500 }));
    await expect(fetchRunNetflow('r1')).rejects.toThrow(/500/);
    vi.unstubAllGlobals();
  });

  it('throws an ApiError carrying the status, matching the rest of the client', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 404 }));
    try {
      await fetchRunNetflow('missing');
      expect.unreachable('expected fetchRunNetflow to throw');
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).status).toBe(404);
    }
    vi.unstubAllGlobals();
  });
});
