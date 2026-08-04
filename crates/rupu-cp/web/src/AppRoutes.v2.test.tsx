// @vitest-environment jsdom
// AppRoutes shell branch — routing/redirect behavior driven by the
// `shell` flag (Task 4's `getShell()`). Under `shell="v2"` the v1 list
// routes (`/dashboard`, `/runs/agents`, `/findings`, `/workers`, …) redirect
// into the composite v2 IA (`/overview`, `/activity?tab=…`, `/security?tab=…`,
// `/fleet?tab=…`); under `shell="v1"` (or for routes outside the redirect
// map — detail pages, `/events`) nothing changes. See Composite.test.tsx for
// the tab-selection behavior those `?tab=` destinations rely on.
//
// `./pages/RunDetail` is mocked to a stub: it pulls in `@xyflow/react`, which
// every other test that touches it (RunDetail.test.tsx, RunGraph.edges.test.tsx,
// WorkflowEditorGraph.test.tsx) also mocks rather than mounting for real in
// jsdom. This test only cares that routing left `/runs/:id` alone, not that
// the graph itself renders.
//
// `./lib/api` is mocked broadly: every function any page reachable from the
// 8 cases below touches on mount, resolving to an empty/harmless value so no
// page's own data-fetching effect errors or hangs the test.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { api } from './lib/api';
import { ThemeProvider } from './components/theme/ThemeProvider';

vi.mock('./pages/RunDetail', () => ({
  __esModule: true,
  default: () => <div>run-detail-stub</div>,
}));

import { AppRoutes } from './App';

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

// jsdom has no matchMedia — `<ThemeToggle>` (mounted in both Layout and
// ShellV2) reads it via ThemeProvider. Same stub as Shell.test.tsx.
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

function mockApi() {
  // Shell v2 chrome (rail host footer, top-bar scope select, live pill).
  vi.spyOn(api, 'getHosts').mockResolvedValue([]);
  vi.spyOn(api, 'getProjects').mockResolvedValue([]);
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([]);
  vi.spyOn(api, 'subscribeEvents').mockImplementation(() => () => {});
  // CommandPalette's fetch-on-open sources (mounted by both Layout and
  // ShellV2, but only called if opened — mocked for safety/parity with
  // Shell.test.tsx's broad list).
  vi.spyOn(api, 'getRuns').mockResolvedValue([]);
  vi.spyOn(api, 'getAgents').mockResolvedValue([]);
  vi.spyOn(api, 'getWorkflows').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([]);
  vi.spyOn(api, 'getSessions').mockResolvedValue([]);
  vi.spyOn(api, 'getCoverage').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);
  vi.spyOn(api, 'getWorkers').mockResolvedValue([]);
  vi.spyOn(api, 'getFindings').mockResolvedValue({
    findings: [],
    summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
  });
  // Composite-tab destination pages reachable below.
  vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
  vi.spyOn(api, 'getEvents').mockResolvedValue([]);
  vi.spyOn(api, 'getDashboard').mockRejectedValue(new Error('no dashboard in test'));
  vi.spyOn(api, 'getRun').mockRejectedValue(new Error('no run detail in test'));
}

function LocationSpy() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

function renderApp(shell: 'v1' | 'v2', initialPath: string) {
  return render(
    <ThemeProvider>
      <MemoryRouter initialEntries={[initialPath]}>
        <LocationSpy />
        <AppRoutes shell={shell} />
      </MemoryRouter>
    </ThemeProvider>,
  );
}

beforeEach(() => {
  installLocalStorage();
  installMatchMedia();
  mockApi();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('AppRoutes shell branch', () => {
  it('v2: /dashboard redirects to /overview', async () => {
    renderApp('v2', '/dashboard');
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/overview'));
  });

  it('v2: /runs/agents redirects to /activity?tab=agents', async () => {
    renderApp('v2', '/runs/agents');
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/activity?tab=agents'));
  });

  it('v2: /findings redirects to /security?tab=findings', async () => {
    renderApp('v2', '/findings');
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/security?tab=findings'));
  });

  it('v2: /workers redirects to /fleet?tab=workers', async () => {
    renderApp('v2', '/workers');
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/fleet?tab=workers'));
  });

  it('v1: /dashboard stays at /dashboard', async () => {
    renderApp('v1', '/dashboard');
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/dashboard'));
  });

  it('v1: /overview stays at /overview (v2 routes exist under v1 too)', async () => {
    renderApp('v1', '/overview');
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/overview'));
  });

  it('v2: /runs/abc123 (detail) is left untouched', async () => {
    renderApp('v2', '/runs/abc123');
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/runs/abc123'));
    expect(await screen.findByText('run-detail-stub')).toBeInTheDocument();
  });

  it('v2: /events (wall display) survives untouched', async () => {
    renderApp('v2', '/events');
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/events'));
  });
});
