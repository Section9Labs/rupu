/**
 * Customers API client: CRUD, assignment, the customer config layer, and the
 * optional `customer` scope param on every list/aggregate call (appended as
 * `customer=<slug|none>` only when non-null, so every existing call is
 * byte-identical).
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { api, ApiError, parseCustomerConflict } from './api';

function mockFetch(status: number, body: unknown) {
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  const fn = vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    text: () => Promise.resolve(text),
  });
  vi.stubGlobal('fetch', fn);
  return fn;
}

const urlOf = (fn: ReturnType<typeof vi.fn>): string => fn.mock.calls[0][0] as string;
const initOf = (fn: ReturnType<typeof vi.fn>): RequestInit => (fn.mock.calls[0][1] ?? {}) as RequestInit;

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('customers CRUD', () => {
  it('getCustomers sends archived + range', async () => {
    const f = mockFetch(200, []);
    await api.getCustomers({ archived: true, range: '7d' });
    expect(urlOf(f)).toBe('/api/customers?archived=1&range=7d');
  });

  it('getCustomers with no opts hits the bare path', async () => {
    const f = mockFetch(200, []);
    await api.getCustomers();
    expect(urlOf(f)).toBe('/api/customers');
  });

  it('getCustomer encodes the slug and passes range', async () => {
    const f = mockFetch(200, {});
    await api.getCustomer('acme', '30d');
    expect(urlOf(f)).toBe('/api/customers/acme?range=30d');
    const g = mockFetch(200, {});
    await api.getCustomer('acme');
    expect(urlOf(g)).toBe('/api/customers/acme');
  });

  it('createCustomer POSTs the body', async () => {
    const f = mockFetch(201, { slug: 'acme' });
    await api.createCustomer({ slug: 'acme', name: 'Acme', color: '#112233' });
    expect(urlOf(f)).toBe('/api/customers');
    expect(initOf(f).method).toBe('POST');
    expect(JSON.parse(initOf(f).body as string)).toEqual({ slug: 'acme', name: 'Acme', color: '#112233' });
  });

  it('updateCustomer PATCHes (empty string clears)', async () => {
    const f = mockFetch(200, { slug: 'acme' });
    await api.updateCustomer('acme', { notes: '' });
    expect(urlOf(f)).toBe('/api/customers/acme');
    expect(initOf(f).method).toBe('PATCH');
    expect(JSON.parse(initOf(f).body as string)).toEqual({ notes: '' });
  });

  it('archiveCustomer picks archive / unarchive', async () => {
    const a = mockFetch(200, {});
    await api.archiveCustomer('acme', true);
    expect(urlOf(a)).toBe('/api/customers/acme/archive');
    expect(initOf(a).method).toBe('POST');
    const u = mockFetch(200, {});
    await api.archiveCustomer('acme', false);
    expect(urlOf(u)).toBe('/api/customers/acme/unarchive');
  });

  it('deleteCustomer resolves on 204', async () => {
    const f = mockFetch(204, '');
    await expect(api.deleteCustomer('acme')).resolves.toBeUndefined();
    expect(initOf(f).method).toBe('DELETE');
    expect(urlOf(f)).toBe('/api/customers/acme');
  });

  it('deleteCustomer rejects with ApiError(409) carrying the conflicting projects', async () => {
    mockFetch(409, { error: 'customer has projects', projects: [{ ws_id: 'ws_1', path: '/p' }] });
    const err = await api.deleteCustomer('acme').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(409);
    expect(JSON.parse((err as ApiError).body).projects[0].ws_id).toBe('ws_1');
  });

  it('assignProject PUTs, unassignProject DELETEs', async () => {
    const a = mockFetch(200, { ws_id: 'ws_1' });
    await api.assignProject('acme', 'ws_1');
    expect(urlOf(a)).toBe('/api/customers/acme/projects/ws_1');
    expect(initOf(a).method).toBe('PUT');
    const d = mockFetch(204, '');
    await expect(api.unassignProject('acme', 'ws_1')).resolves.toBeUndefined();
    expect(urlOf(d)).toBe('/api/customers/acme/projects/ws_1');
    expect(initOf(d).method).toBe('DELETE');
  });

  it('unassignProject on a project not assigned rejects with 404', async () => {
    mockFetch(404, { error: 'not assigned' });
    const err = await api.unassignProject('acme', 'ws_1').catch((e: unknown) => e);
    expect((err as ApiError).status).toBe(404);
  });

  it('getCustomerConfig / putCustomerConfig target the customer layer', async () => {
    const g = mockFetch(200, {});
    await api.getCustomerConfig('acme');
    expect(urlOf(g)).toBe('/api/config?customer=acme');
    const p = mockFetch(200, { ok: true });
    await expect(api.putCustomerConfig('acme', { raw: 'x = 1' })).resolves.toBeUndefined();
    expect(urlOf(p)).toBe('/api/config/customer/acme');
    expect(initOf(p).method).toBe('PUT');
    expect(JSON.parse(initOf(p).body as string)).toEqual({ raw: 'x = 1' });
  });
});

describe('customer scope param', () => {
  it('getWorkflowRuns includes customer=<slug>', async () => {
    const f = mockFetch(200, []);
    await api.getWorkflowRuns({ customer: 'acme', limit: 10 });
    expect(urlOf(f)).toBe('/api/runs/workflows?limit=10&customer=acme');
  });

  it('customer: null omits the param; undefined too', async () => {
    const f = mockFetch(200, []);
    await api.getWorkflowRuns({ customer: null, limit: 10 });
    expect(urlOf(f)).toBe('/api/runs/workflows?limit=10');
    const g = mockFetch(200, []);
    await api.getRuns();
    expect(urlOf(g)).toBe('/api/runs');
  });

  it("customer: 'none' sends customer=none", async () => {
    const f = mockFetch(200, []);
    await api.getRuns({ customer: 'none' });
    expect(urlOf(f)).toBe('/api/runs?customer=none');
  });

  it('every scoped list / aggregate call carries it', async () => {
    const cases: Array<[string, () => Promise<unknown>]> = [
      ['/api/runs/agents', () => api.getAgentRuns({ customer: 'acme' })],
      ['/api/sessions', () => api.getSessions({ customer: 'acme' })],
      ['/api/findings', () => api.getFindings({ customer: 'acme' })],
      ['/api/projects', () => api.getProjects({ customer: 'acme' })],
      ['/api/usage?', () => api.getUsage(undefined, 'model', undefined, undefined, 'acme')],
      ['/api/usage/timeline', () => api.getUsageTimeline({ customer: 'acme' })],
      ['/api/usage/runs', () => api.getUsageRuns(undefined, undefined, 'acme')],
      ['/api/dashboard', () => api.getDashboard('30d', undefined, 'acme')],
    ];
    for (const [prefix, call] of cases) {
      const f = mockFetch(200, []);
      await call();
      const url = urlOf(f);
      expect(url, prefix).toContain(prefix);
      expect(url, prefix).toContain('customer=acme');
    }
  });

  it('existing calls stay byte-identical without a customer', async () => {
    const f = mockFetch(200, []);
    await api.getProjects();
    expect(urlOf(f)).toBe('/api/projects');
    const g = mockFetch(200, {});
    await api.getFindings({ wsId: 'ws_1' });
    expect(urlOf(g)).toBe('/api/findings?ws_id=ws_1');
    const h = mockFetch(200, {});
    await api.getDashboard('7d', 'h1');
    expect(urlOf(h)).toBe('/api/dashboard?range=7d&host=h1');
    const i = mockFetch(200, {});
    await api.getUsageTimeline();
    expect(urlOf(i)).toBe('/api/usage/timeline');
  });
});

describe('parseCustomerConflict', () => {
  it('parses the 409 body of a delete', () => {
    const e = new ApiError(409, 'x', JSON.stringify({ error: 'has projects', projects: [{ ws_id: 'ws_1', path: '/p' }] }));
    expect(parseCustomerConflict(e)).toEqual({ error: 'has projects', projects: [{ ws_id: 'ws_1', path: '/p' }] });
  });

  it('drops malformed project entries', () => {
    const e = new ApiError(
      409,
      'x',
      JSON.stringify({ error: 'e', projects: [{ ws_id: 'ws_1', path: '/p' }, { ws_id: 3 }, null, 'nope'] }),
    );
    expect(parseCustomerConflict(e)?.projects).toEqual([{ ws_id: 'ws_1', path: '/p' }]);
  });

  it('is null for anything that is not a parseable 409 conflict', () => {
    expect(parseCustomerConflict(new ApiError(409, 'x', 'not json'))).toBeNull();
    expect(parseCustomerConflict(new ApiError(409, 'x', JSON.stringify({ error: 'archived' })))).toBeNull();
    expect(parseCustomerConflict(new ApiError(409, 'x', 'null'))).toBeNull();
    expect(
      parseCustomerConflict(new ApiError(404, 'x', JSON.stringify({ error: 'e', projects: [] }))),
    ).toBeNull();
    expect(parseCustomerConflict(new Error('boom'))).toBeNull();
    expect(parseCustomerConflict('409')).toBeNull();
  });
});
