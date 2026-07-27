/**
 * Unit tests for explicit scope targeting on the destructive
 * agent/workflow/autoflow endpoints (fix for the resolver-precedence and
 * two-repos-same-name data-loss bugs — see
 * `rupu-cp/src/api/{agents,workflows,autoflows}.rs`).
 *
 * `deleteAgent`/`deleteWorkflow`/`setAutoflowEnabled` now accept an optional
 * `ScopeSelector` (built via `scopeSelectorFor(row)`) and thread it as
 * `?scope_kind=&scope_id=` query params, so the server resolves EXACTLY the
 * row the operator acted on instead of re-deriving "the" match for a bare
 * name (ambiguous when two different repos define the same name). The
 * delete endpoints also now return the server's resolved `{ scope,
 * scope_kind }` instead of discarding the response body, so a caller can
 * confirm what was actually removed.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { api, scopeSelectorFor } from './api';

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
// scopeSelectorFor
// ---------------------------------------------------------------------------

describe('scopeSelectorFor', () => {
  it('returns undefined for a row with no scope_kind (older fixture / not yet loaded)', () => {
    expect(scopeSelectorFor({})).toBeUndefined();
  });

  it('carries scope_kind + scope_id for a project row', () => {
    expect(scopeSelectorFor({ scope_kind: 'project', scope_id: 'ws_a' })).toEqual({
      scope_kind: 'project',
      scope_id: 'ws_a',
    });
  });

  it('carries scope_kind alone (scope_id undefined) for a global row', () => {
    const sel = scopeSelectorFor({ scope_kind: 'global' });
    expect(sel?.scope_kind).toBe('global');
    expect(sel?.scope_id).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// deleteAgent
// ---------------------------------------------------------------------------

describe('api.deleteAgent — scope targeting', () => {
  it('omits the query string when no target is given (back-compat implicit resolution)', async () => {
    mockFetch(200, { deleted: true, scope: 'global', scope_kind: 'global' });
    const fetchSpy = vi.mocked(fetch);

    await api.deleteAgent('reviewer');

    expect(fetchSpy.mock.calls[0][0]).toBe('/api/agents/reviewer');
    const init = fetchSpy.mock.calls[0][1] as RequestInit;
    expect(init.method).toBe('DELETE');
  });

  it('sends ?scope_kind=project&scope_id=<id> for a project-scoped target', async () => {
    mockFetch(200, { deleted: true, scope: 'proj-y', scope_kind: 'project' });
    const fetchSpy = vi.mocked(fetch);

    await api.deleteAgent('reviewer', { scope_kind: 'project', scope_id: 'ws_y' });

    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toContain('/api/agents/reviewer?');
    expect(url).toContain('scope_kind=project');
    expect(url).toContain('scope_id=ws_y');
  });

  it('sends ?scope_kind=global with no scope_id for a global-scoped target', async () => {
    mockFetch(200, { deleted: true, scope: 'global', scope_kind: 'global' });
    const fetchSpy = vi.mocked(fetch);

    await api.deleteAgent('reviewer', { scope_kind: 'global' });

    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toBe('/api/agents/reviewer?scope_kind=global');
  });

  it('resolves to the server-reported resolved scope, not a discarded void', async () => {
    mockFetch(200, { deleted: true, scope: 'proj-y', scope_kind: 'project' });

    const result = await api.deleteAgent('reviewer', { scope_kind: 'project', scope_id: 'ws_y' });

    expect(result).toEqual({ deleted: true, scope: 'proj-y', scope_kind: 'project' });
  });
});

// ---------------------------------------------------------------------------
// deleteWorkflow
// ---------------------------------------------------------------------------

describe('api.deleteWorkflow — scope targeting', () => {
  it('omits the query string when no target is given', async () => {
    mockFetch(200, { deleted: true, scope: 'global', scope_kind: 'global' });
    const fetchSpy = vi.mocked(fetch);

    await api.deleteWorkflow('nightly-sweep');

    expect(fetchSpy.mock.calls[0][0]).toBe('/api/workflows/nightly-sweep');
  });

  it('sends ?scope_kind=project&scope_id=<id> for a project-scoped target', async () => {
    mockFetch(200, { deleted: true, scope: 'proj-y', scope_kind: 'project' });
    const fetchSpy = vi.mocked(fetch);

    await api.deleteWorkflow('foo', { scope_kind: 'project', scope_id: 'ws_y' });

    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toContain('/api/workflows/foo?');
    expect(url).toContain('scope_kind=project');
    expect(url).toContain('scope_id=ws_y');
  });

  it('resolves to the server-reported resolved scope', async () => {
    mockFetch(200, { deleted: true, scope: 'my-project', scope_kind: 'project' });

    const result = await api.deleteWorkflow('foo', { scope_kind: 'project', scope_id: 'ws_a' });

    expect(result).toEqual({ deleted: true, scope: 'my-project', scope_kind: 'project' });
  });

  it('a 404 (explicit-scope mismatch) rejects rather than silently resolving elsewhere', async () => {
    mockFetch(404, 'workflow foo not found in the requested scope');

    await expect(
      api.deleteWorkflow('foo', { scope_kind: 'project', scope_id: 'no-such-ws' }),
    ).rejects.toMatchObject({ status: 404 });
  });
});

// ---------------------------------------------------------------------------
// setAutoflowEnabled
// ---------------------------------------------------------------------------

describe('api.setAutoflowEnabled — scope targeting', () => {
  it('omits the query string when no target is given', async () => {
    mockFetch(200, { name: 'nightly', enabled: false });
    const fetchSpy = vi.mocked(fetch);

    await api.setAutoflowEnabled('nightly', false);

    expect(fetchSpy.mock.calls[0][0]).toBe('/api/autoflows/nightly/disable');
  });

  it('sends ?scope_kind=project&scope_id=<id> on enable', async () => {
    mockFetch(200, { name: 'foo', enabled: true });
    const fetchSpy = vi.mocked(fetch);

    await api.setAutoflowEnabled('foo', true, { scope_kind: 'project', scope_id: 'ws_y' });

    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url).toContain('/api/autoflows/foo/enable?');
    expect(url).toContain('scope_kind=project');
    expect(url).toContain('scope_id=ws_y');
  });
});
