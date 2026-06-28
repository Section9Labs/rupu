/**
 * Unit tests for host-related API helpers.
 * Tests getHosts / addHost / removeHost and the optional `host` param that is
 * threaded through run-list, run-detail, control, launch, and SSE helpers.
 *
 * Pattern mirrors src/lib/api.test.ts (mockFetch / vi.stubGlobal).
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { api, ApiError } from './api';
import type { HostView } from './api';

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
      statusText: status === 200 ? 'OK' : 'Error',
      text: () => Promise.resolve(text),
    }),
  );
}

function mockFetch204(): void {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue({
      ok: true,
      status: 204,
      statusText: 'No Content',
      text: () => Promise.resolve(''),
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------
// getHosts
// ---------------------------------------------------------------------------

describe('api.getHosts', () => {
  const sampleHostView: HostView = {
    id: 'local',
    name: 'Local',
    transport_kind: 'local',
    status: 'online',
    version: '0.9.0',
    active_run_count: 2,
  };

  it('calls GET /api/hosts', async () => {
    mockFetch(200, [sampleHostView]);
    const fetchSpy = vi.mocked(fetch);
    await api.getHosts();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/hosts');
    expect((fetchSpy.mock.calls[0][1] as RequestInit | undefined)?.method).toBeUndefined();
  });

  it('resolves typed HostView[] on 200', async () => {
    mockFetch(200, [sampleHostView]);
    const result = await api.getHosts();
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('local');
    expect(result[0].transport_kind).toBe('local');
    expect(result[0].status).toBe('online');
    expect(result[0].active_run_count).toBe(2);
  });

  it('includes base_url + capabilities for remote hosts', async () => {
    const remote: HostView = {
      id: 'h-abc',
      name: 'staging',
      transport_kind: 'http_cp',
      base_url: 'https://staging.example.com',
      status: 'online',
      version: '0.9.0',
      capabilities: { backends: ['anthropic'], scm_hosts: ['github.com'], permission_modes: ['bypass'] },
      active_run_count: 0,
      last_seen_at: '2026-06-27T10:00:00Z',
    };
    mockFetch(200, [remote]);
    const result = await api.getHosts();
    expect(result[0].base_url).toBe('https://staging.example.com');
    expect(result[0].capabilities?.backends).toEqual(['anthropic']);
  });

  it('throws ApiError on non-2xx', async () => {
    mockFetch(500, 'server error');
    await expect(api.getHosts()).rejects.toThrow(ApiError);
    await expect(api.getHosts()).rejects.toMatchObject({ status: 500 });
  });
});

// ---------------------------------------------------------------------------
// addHost
// ---------------------------------------------------------------------------

describe('api.addHost', () => {
  it('POSTs to /api/hosts with name + base_url', async () => {
    const created: HostView = {
      id: 'h-new', name: 'prod', transport_kind: 'http_cp',
      base_url: 'https://prod.example.com', status: 'offline',
      active_run_count: 0,
    };
    mockFetch(200, created);
    const fetchSpy = vi.mocked(fetch);
    const result = await api.addHost({ name: 'prod', base_url: 'https://prod.example.com' });

    const [url, init] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/hosts');
    expect(init.method).toBe('POST');
    const sentBody = JSON.parse(init.body as string);
    expect(sentBody.name).toBe('prod');
    expect(sentBody.base_url).toBe('https://prod.example.com');
    expect(sentBody.token).toBeUndefined();

    expect(result.id).toBe('h-new');
    expect(result.status).toBe('offline');
  });

  it('includes token in POST body when provided', async () => {
    const created: HostView = {
      id: 'h-tok', name: 'tok-host', transport_kind: 'http_cp',
      base_url: 'https://tok.example.com', status: 'offline', active_run_count: 0,
    };
    mockFetch(200, created);
    const fetchSpy = vi.mocked(fetch);
    await api.addHost({ name: 'tok-host', base_url: 'https://tok.example.com', token: 'secret-token' });

    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.token).toBe('secret-token');
  });

  it('throws ApiError on 501 (read-only deploy)', async () => {
    mockFetch(501, 'not available');
    await expect(api.addHost({ name: 'x', base_url: 'https://x.example.com' })).rejects.toMatchObject({ status: 501 });
  });
});

// ---------------------------------------------------------------------------
// removeHost
// ---------------------------------------------------------------------------

describe('api.removeHost', () => {
  it('sends DELETE /api/hosts/:id', async () => {
    mockFetch204();
    const fetchSpy = vi.mocked(fetch);
    await api.removeHost('h-abc');

    const [url, init] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/hosts/h-abc');
    expect(init.method).toBe('DELETE');
  });

  it('encodes host id in path', async () => {
    mockFetch204();
    const fetchSpy = vi.mocked(fetch);
    await api.removeHost('h/weird');
    const [url] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/hosts/h%2Fweird');
  });

  it('throws ApiError on 400 when removing local', async () => {
    mockFetch(400, 'cannot remove the built-in local host');
    await expect(api.removeHost('local')).rejects.toMatchObject({ status: 400 });
  });
});

// ---------------------------------------------------------------------------
// host param threaded through run-list helpers
// ---------------------------------------------------------------------------

describe('host param: getRuns', () => {
  it('omits ?host when not provided (backward compat)', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getRuns();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs');
  });

  it('appends ?host=<id> when host is provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getRuns({ host: 'h-abc' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs?host=h-abc');
  });

  it('combines offset + limit + host in query', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getRuns({ offset: 10, limit: 20, host: 'h-abc' });
    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toContain('offset=10');
    expect(url).toContain('limit=20');
    expect(url).toContain('host=h-abc');
  });
});

describe('host param: getWorkflowRuns', () => {
  it('omits host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getWorkflowRuns();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/workflows');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getWorkflowRuns({ host: 'h-abc' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/workflows?host=h-abc');
  });
});

describe('host param: getAgentRuns', () => {
  it('omits host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAgentRuns();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/agents');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAgentRuns({ host: 'h-abc' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/agents?host=h-abc');
  });
});

// ---------------------------------------------------------------------------
// host param threaded through getRun
// ---------------------------------------------------------------------------

describe('host param: getRun', () => {
  const payload = {
    run: { id: 'r1', workflow_name: 'wf', status: 'running', started_at: '2026-01-01T00:00:00Z' },
    steps: [],
  };

  it('omits ?host when not provided', async () => {
    mockFetch(200, payload);
    const fetchSpy = vi.mocked(fetch);
    await api.getRun('r1');
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/r1');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, payload);
    const fetchSpy = vi.mocked(fetch);
    await api.getRun('r1', { host: 'h-abc' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/r1?host=h-abc');
  });
});

// ---------------------------------------------------------------------------
// host param threaded through control calls
// ---------------------------------------------------------------------------

describe('host param: approveRun', () => {
  it('sends empty body when neither mode nor host provided', async () => {
    mockFetch(200, {});
    const fetchSpy = vi.mocked(fetch);
    await api.approveRun('r1');
    const init = fetchSpy.mock.calls[0][1] as RequestInit;
    expect(init.body).toBeUndefined();
  });

  it('includes host in POST body when provided', async () => {
    mockFetch(200, {});
    const fetchSpy = vi.mocked(fetch);
    await api.approveRun('r1', 'bypass', 'h-abc');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.mode).toBe('bypass');
    expect(sentBody.host).toBe('h-abc');
  });

  it('omits host field when not provided', async () => {
    mockFetch(200, {});
    const fetchSpy = vi.mocked(fetch);
    await api.approveRun('r1', 'ask');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.host).toBeUndefined();
  });
});

describe('host param: rejectRun', () => {
  it('includes host in POST body when provided', async () => {
    mockFetch(200, {});
    const fetchSpy = vi.mocked(fetch);
    await api.rejectRun('r1', 'not needed', 'h-abc');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.reason).toBe('not needed');
    expect(sentBody.host).toBe('h-abc');
  });

  it('omits host field when not provided', async () => {
    mockFetch(200, {});
    const fetchSpy = vi.mocked(fetch);
    await api.rejectRun('r1', 'not needed');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.host).toBeUndefined();
  });
});

describe('host param: cancelRun', () => {
  it('includes host in POST body when provided', async () => {
    mockFetch(200, {});
    const fetchSpy = vi.mocked(fetch);
    await api.cancelRun('r1', 'user request', 'h-abc');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.reason).toBe('user request');
    expect(sentBody.host).toBe('h-abc');
  });

  it('sends no body when neither reason nor host provided', async () => {
    mockFetch(200, {});
    const fetchSpy = vi.mocked(fetch);
    await api.cancelRun('r1');
    const init = fetchSpy.mock.calls[0][1] as RequestInit;
    expect(init.body).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// host param threaded through launch helpers
// ---------------------------------------------------------------------------

describe('host param: launchRun', () => {
  it('omits host field when not provided', async () => {
    mockFetch(200, { run_id: 'r-new' });
    const fetchSpy = vi.mocked(fetch);
    await api.launchRun('my-workflow');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.host).toBeUndefined();
  });

  it('includes host in POST body when provided', async () => {
    mockFetch(200, { run_id: 'r-new' });
    const fetchSpy = vi.mocked(fetch);
    await api.launchRun('my-workflow', { mode: 'bypass', host: 'h-abc' });
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.mode).toBe('bypass');
    expect(sentBody.host).toBe('h-abc');
  });
});

describe('host param: launchAgent', () => {
  it('includes host in POST body when provided', async () => {
    mockFetch(200, { run_id: 'r-agent' });
    const fetchSpy = vi.mocked(fetch);
    await api.launchAgent('my-agent', { prompt: 'go', host: 'h-abc' });
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.prompt).toBe('go');
    expect(sentBody.host).toBe('h-abc');
  });
});

describe('host param: startSession', () => {
  it('includes host in POST body when provided', async () => {
    mockFetch(200, { session_id: 'sess-1' });
    const fetchSpy = vi.mocked(fetch);
    await api.startSession('my-agent', { host: 'h-abc' });
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.host).toBe('h-abc');
  });

  it('omits host field when not provided', async () => {
    mockFetch(200, { session_id: 'sess-2' });
    const fetchSpy = vi.mocked(fetch);
    await api.startSession('my-agent');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.host).toBeUndefined();
  });
});

describe('host param: sendSessionMessage', () => {
  it('includes host in POST body when provided', async () => {
    mockFetch(200, { run_id: 'r-turn' });
    const fetchSpy = vi.mocked(fetch);
    await api.sendSessionMessage('sess-1', 'hello', 'h-abc');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.prompt).toBe('hello');
    expect(sentBody.host).toBe('h-abc');
  });

  it('omits host field when not provided (backward compat)', async () => {
    mockFetch(200, { run_id: 'r-turn' });
    const fetchSpy = vi.mocked(fetch);
    await api.sendSessionMessage('sess-1', 'hello');
    const sentBody = JSON.parse((fetchSpy.mock.calls[0][1] as RequestInit).body as string);
    expect(sentBody.host).toBeUndefined();
  });
});
