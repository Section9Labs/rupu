// @vitest-environment jsdom
// Projects list — One Control Language migration (Phase 3, Task H). Covers
// the kit loading/empty states and the table-rules subject/fit columns (no
// filters exist on this page — the win here is chrome, not FilterBar).
//
// UsageBarChart is mocked to keep recharts out of the test.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type ProjectRow } from '../lib/api';

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

import Projects from './Projects';
import { scopedEntry, withCustomerScope } from '../lib/customerScopeTestUtils';
import { UNKNOWN_CUSTOMER_TITLE } from '../components/customers/CustomerChip';

const USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: null,
  priced: true,
  runs: 0,
};

const ROWS: ProjectRow[] = [
  {
    ws_id: 'ws-1',
    name: 'my-project',
    path: '/Users/matt/code/my-project',
    created_at: '2026-07-01T00:00:00Z',
    usage: USAGE,
    run_count: 4,
    customer: null,
  },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/projects']}>
      {withCustomerScope(<Projects />)}
    </MemoryRouter>,
  );
}

describe('Projects — kit loading/empty states', () => {
  it('shows the kit Spinner while the initial fetch is in flight', () => {
    vi.spyOn(api, 'getProjects').mockImplementation(() => new Promise(() => {}));
    renderPage();

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByText('Loading projects…')).toBeInTheDocument();
  });

  it('renders the kit EmptyState with the existing copy when there are no projects', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    renderPage();

    await waitFor(() => expect(screen.getByText('No projects yet')).toBeInTheDocument());
    expect(
      screen.getByText(/run an agent against a directory to register it as a project/i),
    ).toBeInTheDocument();
  });

  it('renders the kit ErrorBanner on fetch failure', async () => {
    vi.spyOn(api, 'getProjects').mockRejectedValue(new Error('boom'));
    renderPage();

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });
});

describe('Projects — table rules', () => {
  it('the name column is the one flexible/truncating subject column', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue(ROWS);
    renderPage();

    await waitFor(() => expect(screen.getByText('my-project')).toBeInTheDocument());

    const subjectCell = screen.getByText('my-project').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="my-project"]')).toBeInTheDocument();
  });

  it('the Runs column is a fit (nowrap) column', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue(ROWS);
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('my-project')).toBeInTheDocument());

    const runsHeader = Array.from(container.querySelectorAll('thead th')).find((th) =>
      th.textContent?.includes('Runs'),
    );
    expect(runsHeader?.className).toMatch(/whitespace-nowrap/);
  });
});

describe('Projects — customer scope and column', () => {
  const ROW_BASE = ROWS[0];
  const MIXED: ProjectRow[] = [
    {
      ...ROW_BASE,
      ws_id: 'ws-a',
      name: 'acme-api',
      customer: { slug: 'acme', name: 'Acme', tint: { light: '#2255aa', dark: '#88aaee' }, archived: false },
      usage: { ...USAGE, cost_usd: 2, pricing_error: 'acme layer broken' },
    },
    { ...ROW_BASE, ws_id: 'ws-b', name: 'loose-tool', customer: null },
    // The CP can't say whose this is: the key is absent.
    (({ customer: _omit, ...rest }) => ({ ...rest, ws_id: 'ws-c', name: 'mystery' }))(ROW_BASE) as ProjectRow,
  ];

  it('shows the CUSTOMER column with all three states and marks a mispriced cost', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue(MIXED);
    renderPage();
    await screen.findAllByText('acme-api');
    expect(screen.getAllByRole('columnheader').map((h) => h.textContent)).toContain('Customer');
    expect(screen.getAllByText('Acme').length).toBeGreaterThan(0);
    expect(screen.getAllByText('No customer').length).toBeGreaterThan(0);
    const unknown = screen.getAllByText('Unknown customer');
    expect(unknown[0]).toHaveAttribute('title', UNKNOWN_CUSTOMER_TITLE);
    expect(screen.getAllByTitle('acme layer broken').length).toBeGreaterThan(0);
  });

  it('passes the global scope as customer, and none for Unassigned', async () => {
    const spy = vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    render(
      <MemoryRouter initialEntries={[scopedEntry('acme', '/projects')]}>
        {withCustomerScope(<Projects />)}
      </MemoryRouter>,
    );
    await waitFor(() => expect(spy).toHaveBeenCalledWith({ customer: 'acme' }));
    expect(await screen.findByText('No project is assigned to this customer yet.')).toBeInTheDocument();
    cleanup();
    spy.mockClear();
    render(
      <MemoryRouter initialEntries={[scopedEntry('none', '/projects')]}>
        {withCustomerScope(<Projects />)}
      </MemoryRouter>,
    );
    await waitFor(() => expect(spy).toHaveBeenCalledWith({ customer: 'none' }));
  });

  it('unscoped, it calls getProjects with no argument, as before', async () => {
    const spy = vi.spyOn(api, 'getProjects').mockResolvedValue(ROWS);
    renderPage();
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0]).toEqual([]);
  });
});
