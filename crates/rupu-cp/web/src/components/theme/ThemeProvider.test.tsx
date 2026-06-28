// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, act } from '@testing-library/react';
import { ThemeProvider, useTheme } from './ThemeProvider';
import ThemeToggle from './ThemeToggle';

const STORAGE_KEY = 'rupu.cp.theme';

// jsdom has no matchMedia — install a controllable mock. `dark` decides what
// `(prefers-color-scheme: dark)` reports; listeners are captured so tests can
// fire a live `change` event.
type MqlListener = (e: MediaQueryListEvent) => void;
let mqlListeners: MqlListener[] = [];

function installMatchMedia(dark: boolean) {
  mqlListeners = [];
  let matches = dark;
  vi.stubGlobal(
    'matchMedia',
    vi.fn().mockImplementation((query: string) => ({
      get matches() {
        return matches;
      },
      media: query,
      onchange: null,
      addEventListener: (_: string, cb: MqlListener) => mqlListeners.push(cb),
      removeEventListener: (_: string, cb: MqlListener) => {
        mqlListeners = mqlListeners.filter((l) => l !== cb);
      },
      addListener: (cb: MqlListener) => mqlListeners.push(cb),
      removeListener: (cb: MqlListener) => {
        mqlListeners = mqlListeners.filter((l) => l !== cb);
      },
      dispatchEvent: () => true,
    })),
  );
  return {
    setMatches(next: boolean) {
      matches = next;
      const evt = { matches: next } as MediaQueryListEvent;
      mqlListeners.forEach((l) => l(evt));
    },
  };
}

// jsdom's localStorage is unreliable under this Node version — install a
// simple in-memory implementation we fully control.
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

// A tiny probe that surfaces the context values into the DOM for assertions.
function Probe() {
  const { theme, mode } = useTheme();
  return (
    <div>
      <span data-testid="theme">{theme}</span>
      <span data-testid="mode">{mode}</span>
    </div>
  );
}

beforeEach(() => {
  installLocalStorage();
  delete document.documentElement.dataset.theme;
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('ThemeProvider', () => {
  it('default (no stored value) resolves from the media query → dark', () => {
    installMatchMedia(true);
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    expect(screen.getByTestId('theme')).toHaveTextContent('system');
    expect(screen.getByTestId('mode')).toHaveTextContent('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
  });

  it('default (no stored value) resolves from the media query → light', () => {
    installMatchMedia(false);
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    expect(screen.getByTestId('mode')).toHaveTextContent('light');
    expect(document.documentElement.dataset.theme).toBe('light');
  });

  it('reads a stored value on init', () => {
    localStorage.setItem(STORAGE_KEY, 'dark');
    installMatchMedia(false); // system would be light; stored 'dark' must win
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    expect(screen.getByTestId('theme')).toHaveTextContent('dark');
    expect(screen.getByTestId('mode')).toHaveTextContent('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
  });

  it('follows live OS colour-scheme changes while in system mode', () => {
    const mql = installMatchMedia(false);
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    expect(document.documentElement.dataset.theme).toBe('light');
    act(() => mql.setMatches(true));
    expect(screen.getByTestId('mode')).toHaveTextContent('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
  });
});

describe('ThemeToggle', () => {
  it('cycles system → light → dark and persists to localStorage', () => {
    installMatchMedia(false);
    render(
      <ThemeProvider>
        <ThemeToggle />
        <Probe />
      </ThemeProvider>,
    );
    const button = screen.getByRole('button');

    // Starts at system (no stored value).
    expect(screen.getByTestId('theme')).toHaveTextContent('system');

    fireEvent.click(button);
    expect(screen.getByTestId('theme')).toHaveTextContent('light');
    expect(localStorage.getItem(STORAGE_KEY)).toBe('light');
    expect(document.documentElement.dataset.theme).toBe('light');

    fireEvent.click(button);
    expect(screen.getByTestId('theme')).toHaveTextContent('dark');
    expect(localStorage.getItem(STORAGE_KEY)).toBe('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');

    fireEvent.click(button);
    expect(screen.getByTestId('theme')).toHaveTextContent('system');
    expect(localStorage.getItem(STORAGE_KEY)).toBe('system');
  });
});
