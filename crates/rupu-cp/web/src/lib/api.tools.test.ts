import { describe, it, expect, vi, afterEach } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

describe('tools API', () => {
  it('getTools hits /api/tools and unwraps the tools array', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({
          tools: [
            {
              name: 'scm.prs.create',
              aliases: [],
              namespace: 'scm',
              effect: 'external',
              needs: ['scm'],
              description: 'Open a PR',
              input_schema: {},
              action_eligible: true,
            },
            {
              name: 'bash',
              aliases: [],
              namespace: 'core',
              effect: 'write',
              needs: [],
              description: 'Run a command',
              input_schema: {},
              action_eligible: false,
            },
          ],
        }),
      ),
    );

    const tools = await api.getTools();

    expect(fetchMock.mock.calls[0][0]).toBe('/api/tools');
    expect(tools).toHaveLength(2);
    expect(tools.find((t) => t.name === 'scm.prs.create')?.effect).toBe('external');
    expect(tools.find((t) => t.name === 'bash')?.action_eligible).toBe(false);
  });
});
