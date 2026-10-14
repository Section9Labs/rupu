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
import { commitQuery } from '../components/query/queryTestUtils';
import { scopedEntry, withCustomerScope } from '../lib/customerScopeTestUtils';

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
      {withCustomerScope(<CoverageTemplates />)}
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

describe('CoverageTemplates — under a customer scope', () => {
  it('says the catalog is shared by every customer', async () => {
    vi.spyOn(api, 'getCoverageTemplates').mockResolvedValue([TEMPLATE]);
    render(
      <MemoryRouter initialEntries={[scopedEntry('acme', '/coverage/templates')]}>
        {withCustomerScope(<CoverageTemplates />)}
      </MemoryRouter>,
    );
    expect(await screen.findByRole('note')).toHaveTextContent(
      'Concern templates aren’t per customer — every customer shares this catalog.',
    );
  });
});

describe('CoverageTemplates query line', () => {
  const ROWS: TemplateSummary[] = [
    TEMPLATE,
    { name: 'stride', version: 1, description: 'Threat categories.', concern_count: 6, severity_breakdown: { medium: 4, low: 2 } },
    { name: 'cwe-top-25', version: 3, description: 'Most dangerous weaknesses.', concern_count: 25, severity_breakdown: { high: 25 } },
  ];
  const mount = (entry: string) =>
    render(<MemoryRouter initialEntries={[entry]}>{withCustomerScope(<CoverageTemplates />)}</MemoryRouter>);
  const names = () => ['owasp-top-10', 'stride', 'cwe-top-25'].filter((n) => screen.queryByText(n) !== null);

  it('narrows the table to the query in ?q=', async () => {
    vi.spyOn(api, 'getCoverageTemplates').mockResolvedValue(ROWS);
    mount('/coverage/templates?q=severity>=high version:3 -owasp');
    await waitFor(() => expect(names()).toEqual(['cwe-top-25']));
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 3');
  });

  it('narrows the table as a query is committed', async () => {
    vi.spyOn(api, 'getCoverageTemplates').mockResolvedValue(ROWS);
    mount('/coverage/templates');
    await waitFor(() => expect(names()).toHaveLength(3));
    commitQuery('Filter templates', 'severity<high');
    await waitFor(() => expect(names()).toEqual(['owasp-top-10', 'stride']));
  });

  it('says so when the query keeps nothing', async () => {
    vi.spyOn(api, 'getCoverageTemplates').mockResolvedValue(ROWS);
    mount('/coverage/templates?q=name:pci');
    expect(await screen.findByText('No templates match the filter.')).toBeInTheDocument();
  });
});
