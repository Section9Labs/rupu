// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { ShellStateProvider, useShellState, SHELL_STATE_KEY } from './shellState';

// jsdom's localStorage is unreliable under this Node version — install a
// simple in-memory implementation we fully control (same discipline as
// ThemeProvider.test.tsx / lib/shell.test.ts).
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

const wrapper = ({ children }: { children: React.ReactNode }) => (
  <ShellStateProvider>{children}</ShellStateProvider>
);

beforeEach(() => {
  installLocalStorage();
});

afterEach(() => {
  localStorage.removeItem(SHELL_STATE_KEY);
  vi.unstubAllGlobals();
});

describe('ShellStateProvider / useShellState', () => {
  it('defaults to all-projects / 30d', () => {
    const { result } = renderHook(() => useShellState(), { wrapper });
    expect(result.current.scope).toBeNull();
    expect(result.current.range).toBe('30d');
  });

  it('persists and restores scope + range', () => {
    const first = renderHook(() => useShellState(), { wrapper });
    act(() => {
      first.result.current.setScope('ws-1');
      first.result.current.setRange('7d');
    });
    first.unmount();
    const second = renderHook(() => useShellState(), { wrapper });
    expect(second.result.current.scope).toBe('ws-1');
    expect(second.result.current.range).toBe('7d');
  });

  it('survives malformed stored JSON', () => {
    localStorage.setItem(SHELL_STATE_KEY, '{nope');
    const { result } = renderHook(() => useShellState(), { wrapper });
    expect(result.current.range).toBe('30d');
  });

  it('throws outside the provider', () => {
    // Rendering a hook that throws produces two independent noise sources
    // even though the throw is caught here: React's own dev-mode
    // "above error occurred" console.error, and jsdom's separate reporting
    // of the uncaught exception that propagates out of the synchronous
    // event dispatch React uses internally. Suppress both to keep test
    // output pristine (repo standard).
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const onWindowError = (e: ErrorEvent) => e.preventDefault();
    window.addEventListener('error', onWindowError);
    try {
      expect(() => renderHook(() => useShellState())).toThrow();
    } finally {
      window.removeEventListener('error', onWindowError);
      errorSpy.mockRestore();
    }
  });
});
