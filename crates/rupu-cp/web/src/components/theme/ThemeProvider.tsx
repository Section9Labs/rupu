// Light/dark theme runtime for the rupu CP web app.
//
// The palette is already tokenized in `styles.css` (`:root` = light,
// `[data-theme="dark"]` = dark) and `tailwind.config.ts` keys `darkMode` off the
// `data-theme` attribute on <html>. This provider is the runtime that flips that
// attribute. It tracks the user's *preference* (`theme`: light/dark/system) and
// the *resolved* `mode` (light/dark) that's actually applied. When the
// preference is `system`, the resolved mode follows the OS colour scheme live.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

export type Theme = 'light' | 'dark' | 'system';
export type Mode = 'light' | 'dark';

const STORAGE_KEY = 'rupu.cp.theme';
const DARK_QUERY = '(prefers-color-scheme: dark)';

export interface ThemeContextValue {
  /** The user's preference. */
  theme: Theme;
  /** The resolved mode actually applied to <html>. */
  mode: Mode;
  /** Set the preference; persists to localStorage and re-resolves the mode. */
  setTheme: (next: Theme) => void;
}

// Exported so context-free consumers (e.g. `useThemeColors`) can read the value
// WITHOUT throwing when rendered outside a provider — they fall back to the
// `data-theme` attribute. App code should still use the `useTheme()` hook.
export const ThemeContext = createContext<ThemeContextValue | null>(null);

// --- defensive env helpers (the app is CSR, but guard for SSR/tests) --------

function readStoredTheme(): Theme {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === 'light' || raw === 'dark' || raw === 'system') return raw;
  } catch {
    // localStorage may be unavailable (privacy mode, SSR) — fall through.
  }
  return 'system';
}

function persistTheme(theme: Theme): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // Best-effort; ignore write failures.
  }
}

function prefersDark(): boolean {
  try {
    return window.matchMedia(DARK_QUERY).matches;
  } catch {
    return false;
  }
}

/** Resolve a preference to a concrete light/dark mode. */
function resolveMode(theme: Theme): Mode {
  if (theme === 'system') return prefersDark() ? 'dark' : 'light';
  return theme;
}

function applyMode(mode: Mode): void {
  try {
    document.documentElement.dataset.theme = mode;
  } catch {
    // No document (SSR) — nothing to apply.
  }
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  // Initialise from storage synchronously so the first paint matches the
  // no-flash inline script in index.html.
  const [theme, setThemeState] = useState<Theme>(() => readStoredTheme());
  const [mode, setMode] = useState<Mode>(() => resolveMode(readStoredTheme()));

  // Apply the resolved mode to <html> SYNCHRONOUSLY during render (before any
  // child renders) so inline consumers that read computed CSS variables in the
  // same commit — `useThemeColors()` powering the graph/chart/editor colors —
  // observe the new theme immediately on toggle, with no stale-by-one-frame gap.
  // The DOM write is idempotent; the effect below is the SSR/no-document safety
  // net.
  if (typeof document !== 'undefined' && document.documentElement.dataset.theme !== mode) {
    applyMode(mode);
  }

  // Apply the resolved mode to <html> whenever it changes (safety net for the
  // render-phase write above; also covers environments without `document`).
  useEffect(() => {
    applyMode(mode);
  }, [mode]);

  // Keep `mode` in sync with the preference.
  useEffect(() => {
    setMode(resolveMode(theme));
  }, [theme]);

  // While following the system, react to OS colour-scheme changes live.
  useEffect(() => {
    if (theme !== 'system') return;
    let mql: MediaQueryList;
    try {
      mql = window.matchMedia(DARK_QUERY);
    } catch {
      return;
    }
    const onChange = (e: MediaQueryListEvent) => {
      setMode(e.matches ? 'dark' : 'light');
    };
    mql.addEventListener('change', onChange);
    return () => mql.removeEventListener('change', onChange);
  }, [theme]);

  const setTheme = useCallback((next: Theme) => {
    persistTheme(next);
    setThemeState(next);
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({ theme, mode, setTheme }),
    [theme, mode, setTheme],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) {
    throw new Error('useTheme must be used within a <ThemeProvider>');
  }
  return ctx;
}

export default ThemeProvider;
