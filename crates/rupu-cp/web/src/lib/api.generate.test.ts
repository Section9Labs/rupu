import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

describe('generate api', () => {
  it('posts a description to generateAgent and returns raw', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({ raw: 'name: x', provider: 'anthropic', model: 'claude-sonnet-4-6', attempts: 1 }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    );
    const out = await api.generateAgent({ description: 'a helpful agent' });
    expect(out.raw).toContain('name: x');
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toContain('/api/agents/generate');
    expect(init?.method).toBe('POST');
  });
});
