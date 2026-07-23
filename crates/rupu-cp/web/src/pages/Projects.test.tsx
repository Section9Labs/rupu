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
  },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/projects']}>
      <Projects />
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
