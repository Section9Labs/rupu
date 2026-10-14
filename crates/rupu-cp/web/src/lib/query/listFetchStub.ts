// Test-only: a `fetch` stub for the server-filtered list pages. It records
// every request URL and answers `/api/hosts/registered` with `hosts`, list
// requests with `answer(url)`, and anything else with `[]`.
import { vi } from 'vitest';
import type { RegisteredHostView } from '../api';

export interface StubAnswer {
  status?: number;
  body: unknown;
  headers?: Record<string, string>;
}

export function stubListFetch(
  hosts: RegisteredHostView[],
  answer: (url: URL) => StubAnswer | Promise<StubAnswer> | undefined,
): URL[] {
  const urls: URL[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: string) => {
      const url = new URL(input, 'http://cp.test');
      urls.push(url);
      const a: StubAnswer = url.pathname === '/api/hosts/registered' ? { body: hosts } : ((await answer(url)) ?? { body: [] });
      const status = a.status ?? 200;
      return {
        ok: status >= 200 && status < 300,
        status,
        statusText: String(status),
        headers: new Headers(a.headers ?? {}),
        text: () => Promise.resolve(JSON.stringify(a.body)),
      };
    }),
  );
  return urls;
}

/** The list requests among `urls` for `path`. */
export const listCalls = (urls: URL[], path: string) => urls.filter((u) => u.pathname === path);

/** A 400 the list endpoints answer for a query they can't parse. */
export const parseError = (error: string): StubAnswer => ({
  status: 400,
  body: { error, token: 0, code: 'unknown_key', start: 0, end: 1 },
});
