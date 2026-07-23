// @vitest-environment jsdom
// Coverage list — One Control Language migration (Phase 3, Task H). Covers
// the kit loading/empty states and the table-rules subject/fit columns (no
// filters exist on this page — the win here is chrome, not FilterBar).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type CoverageSummary } from '../lib/api';

import Coverage from './Coverage';

const TARGET: CoverageSummary = {
  ws_id: 'ws-1',
  project: 'my-project',
  target_id: 'src/auth.rs',
  assertion_lines: 12,
  has_catalog: true,
  findings: 2,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/coverage']}>
      <Coverage />
    </MemoryRouter>,
  );
}

describe('Coverage — kit loading/empty states', () => {
  it('shows the kit Spinner while the initial fetch is in flight', () => {
    vi.spyOn(api, 'getCoverage').mockImplementation(() => new Promise(() => {}));
    renderPage();

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByText('Loading coverage…')).toBeInTheDocument();
  });

  it('renders the kit EmptyState with the existing copy when there is no coverage data', async () => {
    vi.spyOn(api, 'getCoverage').mockResolvedValue([]);
    renderPage();

    await waitFor(() => expect(screen.getByText('No coverage data')).toBeInTheDocument());
    expect(screen.getByText(/run an assessment workflow to start recording/i)).toBeInTheDocument();
  });

  it('renders the kit ErrorBanner on fetch failure', async () => {
    vi.spyOn(api, 'getCoverage').mockRejectedValue(new Error('boom'));
    renderPage();

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });
});

describe('Coverage — table rules', () => {
  it('the target column is the one flexible/truncating subject column', async () => {
    vi.spyOn(api, 'getCoverage').mockResolvedValue([TARGET]);
    renderPage();

    await waitFor(() => expect(screen.getByText('src/auth.rs')).toBeInTheDocument());

    const subjectCell = screen.getByText('src/auth.rs').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="src/auth.rs"]')).toBeInTheDocument();
  });

  it('the Catalog column is a fit (nowrap) column', async () => {
    vi.spyOn(api, 'getCoverage').mockResolvedValue([TARGET]);
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('src/auth.rs')).toBeInTheDocument());

    const header = Array.from(container.querySelectorAll('thead th')).find((th) =>
      th.textContent?.includes('Catalog'),
    );
    expect(header?.className).toMatch(/whitespace-nowrap/);
  });
});
