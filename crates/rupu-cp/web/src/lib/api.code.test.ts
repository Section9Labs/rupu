import { describe, it, expect, vi, afterEach } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

describe('project code API', () => {
  it('getProjectTree encodes ws_id in the path and path as a query param', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(new Response(JSON.stringify({ path: '', parent: null, entries: [] })));
    await api.getProjectTree('ws 1', 'src/a');
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toBe('/api/projects/ws%201/tree?path=src%2Fa');
  });

  it('getProjectTree omits path when at root', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(new Response(JSON.stringify({ path: '', parent: null, entries: [] })));
    await api.getProjectTree('ws1');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/projects/ws1/tree');
  });

  it('getProjectSource builds the source URL', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(new Response(JSON.stringify({ available: false })));
    await api.getProjectSource('ws1', 'src/main.rs');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/projects/ws1/source?path=src%2Fmain.rs');
  });
});
