/**
 * Unit tests for Task 5: threading `host` param through agent/autoflow/session + graph/usage helpers.
 * Mirrors the Slice-1 host-threading pattern from api.host.test.ts.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { api } from './api';

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

afterEach(() => {
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------
// getAgentRuns
// ---------------------------------------------------------------------------

describe('host param: getAgentRuns', () => {
  it('omits ?host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAgentRuns();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/agents');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAgentRuns({ host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/agents?host=h1');
  });

  it('combines lifecycle + host in query', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAgentRuns({ lifecycle: 'active', host: 'h1' });
    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toContain('lifecycle=active');
    expect(url).toContain('host=h1');
  });
});

// ---------------------------------------------------------------------------
// getAutoflowRuns
// ---------------------------------------------------------------------------

describe('host param: getAutoflowRuns', () => {
  it('omits ?host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAutoflowRuns();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/autoflows');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAutoflowRuns({ host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/autoflows?host=h1');
  });

  it('combines offset + limit + host in query', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAutoflowRuns({ offset: 5, limit: 10, host: 'h1' });
    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toContain('offset=5');
    expect(url).toContain('limit=10');
    expect(url).toContain('host=h1');
  });
});

// ---------------------------------------------------------------------------
// getAutoflowEvents
// ---------------------------------------------------------------------------

describe('host param: getAutoflowEvents', () => {
  it('omits ?host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAutoflowEvents();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/autoflows/events');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getAutoflowEvents({ host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/autoflows/events?host=h1');
  });
});

// ---------------------------------------------------------------------------
// getSessions
// ---------------------------------------------------------------------------

describe('host param: getSessions', () => {
  it('omits ?host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getSessions();
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sessions');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getSessions({ host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sessions?host=h1');
  });

  it('combines scope + host in query', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getSessions({ scope: 'active', host: 'h1' });
    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toContain('scope=active');
    expect(url).toContain('host=h1');
  });
});

// ---------------------------------------------------------------------------
// getSession
// ---------------------------------------------------------------------------

describe('host param: getSession', () => {
  const payload = {
    session_id: 'sess-1',
    agent_name: 'test-agent',
    model: 'claude-opus',
    status: 'active',
    total_turns: 5,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    scope: 'project',
  };

  it('omits ?host when not provided', async () => {
    mockFetch(200, payload);
    const fetchSpy = vi.mocked(fetch);
    await api.getSession('sess-1');
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sessions/sess-1');
  });

  it('appends ?host=<id> when provided via opts', async () => {
    mockFetch(200, payload);
    const fetchSpy = vi.mocked(fetch);
    await api.getSession('sess-1', { host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sessions/sess-1?host=h1');
  });

  it('encodes special characters in host param', async () => {
    mockFetch(200, payload);
    const fetchSpy = vi.mocked(fetch);
    await api.getSession('sess-1', { host: 'h/weird' });
    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toContain('host=h%2Fweird');
  });
});

// ---------------------------------------------------------------------------
// getSessionRuns
// ---------------------------------------------------------------------------

describe('host param: getSessionRuns', () => {
  it('omits ?host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getSessionRuns('sess-1');
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sessions/sess-1/runs');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getSessionRuns('sess-1', { host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sessions/sess-1/runs?host=h1');
  });
});

// ---------------------------------------------------------------------------
// getRunGraph
// ---------------------------------------------------------------------------

describe('host param: getRunGraph', () => {
  const payload = {
    run: { id: 'r1', workflow_name: 'wf', status: 'running', started_at: '2026-01-01T00:00:00Z' },
    workflow: { steps: [] },
    step_results: [],
    units: [],
  };

  it('omits ?host when not provided', async () => {
    mockFetch(200, payload);
    const fetchSpy = vi.mocked(fetch);
    await api.getRunGraph('r1');
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/r1/graph');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, payload);
    const fetchSpy = vi.mocked(fetch);
    await api.getRunGraph('r1', { host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/r1/graph?host=h1');
  });
});

// ---------------------------------------------------------------------------
// getRunUsageTimeline
// ---------------------------------------------------------------------------

describe('host param: getRunUsageTimeline', () => {
  it('omits ?host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getRunUsageTimeline('r1');
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/r1/usage-timeline');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getRunUsageTimeline('r1', { host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/runs/r1/usage-timeline?host=h1');
  });
});

// ---------------------------------------------------------------------------
// getSessionUsageTimeline
// ---------------------------------------------------------------------------

describe('host param: getSessionUsageTimeline', () => {
  it('omits ?host when not provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getSessionUsageTimeline('sess-1');
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sessions/sess-1/usage-timeline');
  });

  it('appends ?host=<id> when provided', async () => {
    mockFetch(200, []);
    const fetchSpy = vi.mocked(fetch);
    await api.getSessionUsageTimeline('sess-1', { host: 'h1' });
    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sessions/sess-1/usage-timeline?host=h1');
  });
});
