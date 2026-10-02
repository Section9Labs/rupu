// Test-only helpers for per-host loading. Imported by *.test.ts(x) files only.
import type { RegisteredHostView } from '../api';

export const REG_LOCAL: RegisteredHostView = { id: 'local', name: 'Local', transport_kind: 'local' };
export const REG_PROD: RegisteredHostView = { id: 'host_prod', name: 'prod', transport_kind: 'http_cp' };

export function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** The calls a list spy received for `host` (the per-host loader passes `host` on every call). */
export function callsFor(spy: { mock: { calls: unknown[][] } }, host: string): unknown[][] {
  return spy.mock.calls.filter((c) => (c[0] as { host?: string } | undefined)?.host === host);
}

/** A list-mock implementation that answers `rows` for `host` and `[]` for every other host. */
export function onlyHost<R>(host: string, rows: R[]) {
  return (p?: { host?: string }) => Promise.resolve(p?.host === host ? rows : []);
}

/** Let pending promise callbacks run (real timers). */
export const flush = () => new Promise<void>((r) => setTimeout(r, 0));
