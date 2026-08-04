// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getShell } from './shell';

function setMeta(content: string | null) {
  document.querySelector('meta[name="rupu-shell"]')?.remove();
  if (content !== null) {
    const m = document.createElement('meta');
    m.name = 'rupu-shell';
    m.content = content;
    document.head.appendChild(m);
  }
}

function installLocalStorage() {
  const store = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => store.set(k, String(v)),
    removeItem: (k: string) => store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => Array.from(store.keys())[i] ?? null,
    get length() {
      return store.size;
    },
  });
}

describe('getShell', () => {
  beforeEach(() => {
    installLocalStorage();
  });

  afterEach(() => {
    setMeta(null);
    localStorage.removeItem('rupu.cp.shell');
  });

  it('defaults to v1 with no meta tag', () => {
    expect(getShell()).toBe('v1');
  });

  it('reads v2 from the meta tag', () => {
    setMeta('v2');
    expect(getShell()).toBe('v2');
  });

  it('treats unknown meta values as v1', () => {
    setMeta('v3');
    expect(getShell()).toBe('v1');
  });

  it('honours the DEV localStorage override when no meta tag exists', () => {
    localStorage.setItem('rupu.cp.shell', 'v2');
    expect(getShell()).toBe('v2'); // vitest runs with DEV=true
  });

  it('meta tag beats the localStorage override', () => {
    setMeta('v1');
    localStorage.setItem('rupu.cp.shell', 'v2');
    expect(getShell()).toBe('v1');
  });
});
