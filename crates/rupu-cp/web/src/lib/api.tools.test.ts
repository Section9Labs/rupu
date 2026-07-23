import { describe, it, expect, vi, afterEach } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

describe('tools API', () => {
  it('getTools hits /api/tools and unwraps the tools array', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({
          tools: [
            { name: 'scm.prs.create', description: 'Open a PR', input_schema: {}, kind: 'write' },
            { name: 'scm.repos.list', description: 'List repos', input_schema: {}, kind: 'read' },
          ],
        }),
      ),
    );

    const tools = await api.getTools();

    expect(fetchMock.mock.calls[0][0]).toBe('/api/tools');
    expect(tools).toHaveLength(2);
    expect(tools.find((t) => t.name === 'scm.prs.create')?.kind).toBe('write');
    expect(tools.find((t) => t.name === 'scm.repos.list')?.kind).toBe('read');
  });
});
