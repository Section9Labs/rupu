import { describe, expect, it, vi } from 'vitest';
import { ApiError } from './api';
import type { HostRollup } from './netflow';
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
      json: async () => ({ flows: [], hosts: [], dropped_total: 0, asn_loaded: true }),
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

describe('HostRollup optionality', () => {
  it('handles the percentile keys entirely absent, not null', () => {
    // p50_ms/p95_ms have `skip_serializing_if` on the Rust side, so an
    // unobservable percentile OMITS the key — it is never serialized as
    // `null`. Parse a JSON string built without those keys at all (not an
    // object literal with `p50_ms: undefined`, which is a different runtime
    // shape) so the fixture matches what the server actually sends.
    const rollup: HostRollup = JSON.parse(
      '{"host":"api.github.com","port":443,"calls":3,"bytes_in":null,"bytes_out":120,"errors":0}',
    );

    expect('p50_ms' in rollup).toBe(false);
    expect(rollup.p50_ms).toBeUndefined();
    // The trap this type shape guards against: `!== null` reads true for
    // an absent key, so a naive guard would wrongly treat it as present.
    expect(rollup.p50_ms !== null).toBe(true);
    // The correct guard.
    expect(rollup.p50_ms === undefined).toBe(true);
  });
});
