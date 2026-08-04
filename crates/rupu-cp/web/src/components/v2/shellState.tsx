// Shell v2 global state: workspace scope + time range.
//
// Backs the v2 top bar (Task 8) and every v2 page that needs to know "which
// workspace" and "how far back" to query. Persists to localStorage so the
// selection survives reloads and navigation, mirroring the read/write
// discipline in `SidebarGroup.readGroupState`/`writeGroupState` and the
// context+provider shape of `theme/ThemeProvider.tsx`.

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

export type ShellRange = '7d' | '30d' | 'all';

export interface ShellStateValue {
  /** Workspace id; null = all projects. */
  scope: string | null;
  /** Lookback window; defaults to '30d'. */
  range: ShellRange;
  setScope(next: string | null): void;
  setRange(next: ShellRange): void;
}

export const SHELL_STATE_KEY = 'rupu.cp.shell.v2';

const DEFAULT_RANGE: ShellRange = '30d';
const DEFAULT_SCOPE: string | null = null;

interface StoredShellState {
  scope: string | null;
  range: ShellRange;
}

function isValidRange(value: unknown): value is ShellRange {
  return value === '7d' || value === '30d' || value === 'all';
}

function isValidScope(value: unknown): value is string | null {
  return value === null || typeof value === 'string';
}

function readStoredState(): StoredShellState {
  try {
    const raw = window.localStorage.getItem(SHELL_STATE_KEY);
    if (!raw) return { scope: DEFAULT_SCOPE, range: DEFAULT_RANGE };
    const parsed = JSON.parse(raw);
    if (typeof parsed !== 'object' || parsed === null) {
      return { scope: DEFAULT_SCOPE, range: DEFAULT_RANGE };
    }
    const scope = isValidScope(parsed.scope) ? parsed.scope : DEFAULT_SCOPE;
    const range = isValidRange(parsed.range) ? parsed.range : DEFAULT_RANGE;
    return { scope, range };
  } catch (e) {
    console.warn('shell: malformed shell state, falling back to defaults', e);
    return { scope: DEFAULT_SCOPE, range: DEFAULT_RANGE };
  }
}

function persistState(state: StoredShellState): void {
  try {
    window.localStorage.setItem(SHELL_STATE_KEY, JSON.stringify(state));
  } catch {
    // private mode / quota — fail silently; in-memory state still works
  }
}

const ShellStateContext = createContext<ShellStateValue | null>(null);

export function ShellStateProvider({ children }: { children: ReactNode }): JSX.Element {
  const [state, setState] = useState<StoredShellState>(() => readStoredState());

  const setScope = useCallback((next: string | null) => {
    setState((prev) => {
      const updated = { ...prev, scope: next };
      persistState(updated);
      return updated;
    });
  }, []);

  const setRange = useCallback((next: ShellRange) => {
    setState((prev) => {
      const updated = { ...prev, range: next };
      persistState(updated);
      return updated;
    });
  }, []);

  const value = useMemo<ShellStateValue>(
    () => ({ scope: state.scope, range: state.range, setScope, setRange }),
    [state, setScope, setRange],
  );

  return <ShellStateContext.Provider value={value}>{children}</ShellStateContext.Provider>;
}

export function useShellState(): ShellStateValue {
  const ctx = useContext(ShellStateContext);
  if (!ctx) {
    throw new Error('useShellState must be used within a <ShellStateProvider>');
  }
  return ctx;
}

export default ShellStateProvider;
