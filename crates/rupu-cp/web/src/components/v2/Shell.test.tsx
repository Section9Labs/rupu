// @vitest-environment jsdom
// Shell v2 chrome — rail (nav + brand + host footer), top bar (scope select,
// range segmented, search → command palette, live pill, theme toggle) and
// the `<Outlet/>` body. Mocks `../../lib/api` broadly: the rail/top-bar's own
// calls (`getHosts`, `getProjects`, `subscribeEvents`, `getRegisteredHosts`)
// plus every source the mounted `<CommandPalette shell="v2">` fetches on
// open (see `CommandPalette.v2.test.tsx` for the same list).

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, act, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api } from '../../lib/api';
import type { RunEvent } from '../../lib/api';
import { ThemeProvider } from '../theme/ThemeProvider';
import { SHELL_STATE_KEY } from './shellState';

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

// jsdom has no matchMedia — `<ThemeToggle>` (mounted in the top bar) reads it
// via ThemeProvider. Same minimal stub as ThemeProvider.test.tsx.
function installMatchMedia() {
  vi.stubGlobal(
    'matchMedia',
    vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => true,
    })),
  );
}

// Captured so tests can invoke the mocked subscription's onEvent handler.
let subscribeOnEvent: Parameters<typeof api.subscribeEvents>[0] | undefined;

function mockApi() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([]);
  vi.spyOn(api, 'getProjects').mockResolvedValue([]);
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([]);
  vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => {
    subscribeOnEvent = onEvent;
    return () => {};
  });
  // CommandPalette's fetch-on-open sources.
  vi.spyOn(api, 'getRuns').mockResolvedValue([]);
  vi.spyOn(api, 'getAgents').mockResolvedValue([]);
  vi.spyOn(api, 'getWorkflows').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([]);
  vi.spyOn(api, 'getSessions').mockResolvedValue([]);
  vi.spyOn(api, 'getCoverage').mockResolvedValue([]);
  vi.spyOn(api, 'getFindings').mockResolvedValue({
    findings: [],
    summary: { total: 0, by_severity: {} } as never,
  });
  vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);
  vi.spyOn(api, 'getWorkers').mockResolvedValue([]);
}

import Shell from './Shell';

function renderShell(initialPath = '/overview') {
  return render(
    <ThemeProvider>
      <MemoryRouter initialEntries={[initialPath]}>
        <Routes>
          <Route element={<Shell />}>
            <Route path="*" element={<div>body</div>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </ThemeProvider>,
  );
}

beforeEach(() => {
  installLocalStorage();
  installMatchMedia();
  subscribeOnEvent = undefined;
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('Shell v2', () => {
  it('renders all seven nav labels + Settings, in order', async () => {
    mockApi();
    renderShell();

    const nav = screen.getByRole('navigation');
    const labels = ['Overview', 'Activity', 'Projects', 'Security', 'Library', 'Fleet', 'Usage', 'Settings'];
    const links = within(nav).getAllByRole('link');
    expect(links.map((l) => l.textContent)).toEqual(labels);
  });

  it('shows rupu + cp in the rail header', async () => {
    mockApi();
    renderShell();
    expect(screen.getByText('rupu')).toBeInTheDocument();
    expect(screen.getByText('cp')).toBeInTheDocument();
  });

  it('marks the row matching the current route aria-current="page"', async () => {
    mockApi();
    renderShell('/overview');
    const link = screen.getByRole('link', { name: 'Overview' });
    expect(link).toHaveAttribute('aria-current', 'page');
  });

  it('the search button opens the command palette', async () => {
    mockApi();
    renderShell();
    fireEvent.click(screen.getByRole('button', { name: /jump to run, project, agent, finding/i }));
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
  });

  it('live pill starts connecting and flips to live on the mocked event', async () => {
    mockApi();
    renderShell();
    const pill = screen.getByText('connecting');
    expect(pill).toHaveAttribute('data-state', 'connecting');

    await waitFor(() => expect(subscribeOnEvent).toBeDefined());
    act(() => subscribeOnEvent?.({} as RunEvent));

    expect(await screen.findByText('live')).toHaveAttribute('data-state', 'live');
  });

  it('range segmented reflects 30d default and persists 7d to localStorage', async () => {
    mockApi();
    renderShell();
    const group = screen.getByRole('group', { name: /range/i });
    expect(within(group).getByRole('button', { name: '30d' })).toHaveAttribute('aria-pressed', 'true');

    fireEvent.click(within(group).getByRole('button', { name: '7d' }));

    await waitFor(() => {
      const raw = localStorage.getItem(SHELL_STATE_KEY);
      expect(raw && JSON.parse(raw).range).toBe('7d');
    });
  });
});
