// @vitest-environment jsdom
// Coverage Templates — One Control Language migration (Phase 3, Task H).
// Covers the kit loading/error state and the table-rules subject/fit columns
// (no filters exist on this page — the win here is chrome, not FilterBar).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type TemplateSummary } from '../lib/api';

import CoverageTemplates from './CoverageTemplates';

const TEMPLATE: TemplateSummary = {
  name: 'owasp-top-10',
  version: 3,
  description: 'OWASP Top 10 web application concerns.',
  concern_count: 10,
  severity_breakdown: { critical: 1, high: 4, medium: 3, low: 2, info: 0 },
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/coverage/templates']}>
      <CoverageTemplates />
    </MemoryRouter>,
  );
}

describe('CoverageTemplates — kit loading/error state', () => {
  it('shows the kit Spinner while the initial fetch is in flight', () => {
    vi.spyOn(api, 'getCoverageTemplates').mockImplementation(() => new Promise(() => {}));
    renderPage();

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByText('Loading templates…')).toBeInTheDocument();
  });

  it('renders the kit ErrorBanner on fetch failure', async () => {
    vi.spyOn(api, 'getCoverageTemplates').mockRejectedValue(new Error('boom'));
    renderPage();

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });
});

describe('CoverageTemplates — table rules', () => {
  it('the Template column is the one flexible/truncating subject column', async () => {
    vi.spyOn(api, 'getCoverageTemplates').mockResolvedValue([TEMPLATE]);
    renderPage();

    await waitFor(() => expect(screen.getByText('owasp-top-10')).toBeInTheDocument());

    const subjectCell = screen.getByText('owasp-top-10').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="owasp-top-10"]')).toBeInTheDocument();
  });

  it('the Concerns column is a fit (nowrap) column', async () => {
    vi.spyOn(api, 'getCoverageTemplates').mockResolvedValue([TEMPLATE]);
    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('owasp-top-10')).toBeInTheDocument());

    const header = Array.from(container.querySelectorAll('thead th')).find((th) =>
      th.textContent?.includes('Concerns'),
    );
    expect(header?.className).toMatch(/whitespace-nowrap/);
  });
});
