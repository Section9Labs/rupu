import { describe, it, expect, vi, afterEach } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

describe('model catalog API', () => {
  it('getModelCatalog GETs /api/models', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('[]'));
    await api.getModelCatalog();
    expect(fetchMock.mock.calls[0][0]).toBe('/api/models');
    expect(fetchMock.mock.calls[0][1]?.method).toBeUndefined();
  });

  it('refreshModels() POSTs an empty object body (refresh every provider)', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('[]'));
    await api.refreshModels();
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/models/refresh');
    expect(init?.method).toBe('POST');
    expect(init?.body).toBe('{}');
  });

  it("refreshModels('x') POSTs just that provider", async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('[]'));
    await api.refreshModels('x');
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/models/refresh');
    expect(init?.method).toBe('POST');
    expect(init?.body).toBe('{"provider":"x"}');
  });
});
