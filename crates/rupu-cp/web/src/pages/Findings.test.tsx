// @vitest-environment jsdom
// Global Findings — One Control Language migration (Phase 3, Task H). Covers
// the kit loading/empty states (the metric-tile severity filter is kept
// as-is — it's an intentional filter surface, not a FilterBar) and that the
// FindingsTable subject column (Summary) truncates per the table rules.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type FindingOut, type FindingsSummary } from '../lib/api';

import Findings from './Findings';

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/findings']}>
      <Findings />
    </MemoryRouter>,
  );
}

const SUMMARY: FindingsSummary = { total: 1, critical: 0, high: 1, medium: 0, low: 0, info: 0 };

const FINDING: FindingOut = {
  id: 'f1',
  scope: null,
  summary: 'SQL injection in the billing query builder',
  severity: 'high',
  evidence: { rationale: '' },
  declared_by: null,
  declared_at: '2026-07-01T00:00:00Z',
  ws_id: 'ws-1',
  project: 'my-project',
  target_id: 'src/billing.rs',
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Findings — kit loading/empty states', () => {
  it('shows the kit Spinner while the initial fetch is in flight', () => {
    vi.spyOn(api, 'getFindings').mockImplementation(() => new Promise(() => {}));
    renderPage();

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByText('Loading findings…')).toBeInTheDocument();
  });

  it('renders the kit EmptyState with the existing copy when there are no findings at all', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue({
      findings: [],
      summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
    });
    renderPage();

    await waitFor(() => expect(screen.getByText('No findings')).toBeInTheDocument());
    expect(
      screen.getByText(/run an assessment workflow to start recording findings/i),
    ).toBeInTheDocument();
  });

  it('renders the kit EmptyState when a severity tile narrows the list to zero', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue({ findings: [FINDING], summary: SUMMARY });
    renderPage();

    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: /critical/i }));

    await waitFor(() => expect(screen.getByText('No matches')).toBeInTheDocument());
    expect(screen.getByText('No critical findings.')).toBeInTheDocument();
  });

  it('renders the kit ErrorBanner on fetch failure', async () => {
    vi.spyOn(api, 'getFindings').mockRejectedValue(new Error('boom'));
    renderPage();

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });
});

describe('Findings — table rules', () => {
  it('the Summary column is the one flexible/truncating subject column', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue({ findings: [FINDING], summary: SUMMARY });
    renderPage();

    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());

    const subjectCell = screen.getByText(FINDING.summary).closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector(`[title="${FINDING.summary}"]`)).toBeInTheDocument();
  });
});

describe('Findings — profile / owner / CWE filters', () => {
  function full(id: string, summary: string, owner: string, cwe: string[]): FindingOut {
    return {
      ...FINDING,
      id,
      summary,
      profile: 'full',
      report_summary: {
        owner,
        product: 'Notebin',
        cwe,
        root_cause: 'rc',
        chain: [],
        completeness: { filled: 9, total: 11, gaps: [] },
        has_poc: false,
        verification_status: null,
      },
    };
  }

  const ROWS: FindingOut[] = [
    full('a', 'Full alpha', 'Team A', ['CWE-639']),
    full('b', 'Full beta', 'Team B', ['CWE-862']),
    { ...FINDING, id: 'c', summary: 'Summary gamma', profile: 'summary', concern_id: 'cwe-top25:cwe-639-idor' },
    { ...FINDING, id: 'd', summary: 'Summary delta' },
  ];
  const SUM4: FindingsSummary = { total: 4, critical: 0, high: 4, medium: 0, low: 0, info: 0 };

  async function loaded() {
    vi.spyOn(api, 'getFindings').mockResolvedValue({ findings: ROWS, summary: SUM4 });
    renderPage();
    await waitFor(() => expect(screen.getByText('Full alpha')).toBeInTheDocument());
  }

  it('the Full reports pill hides summary rows, Summaries hides full rows, All restores', async () => {
    await loaded();
    fireEvent.click(screen.getByRole('button', { name: 'Full reports' }));
    expect(screen.getByText('Full alpha')).toBeInTheDocument();
    expect(screen.getByText('Full beta')).toBeInTheDocument();
    expect(screen.queryByText('Summary gamma')).not.toBeInTheDocument();
    expect(screen.queryByText('Summary delta')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Summaries' }));
    expect(screen.queryByText('Full alpha')).not.toBeInTheDocument();
    expect(screen.getByText('Summary gamma')).toBeInTheDocument();
    expect(screen.getByText('Summary delta')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'All' }));
    expect(screen.getByText('Full alpha')).toBeInTheDocument();
    expect(screen.getByText('Summary delta')).toBeInTheDocument();
  });

  it('the Owner select keeps only matching rows and lists distinct owners', async () => {
    await loaded();
    const owner = screen.getByLabelText('Owner filter');
    const options = Array.from(owner.querySelectorAll('option')).map((o) => o.textContent);
    expect(options).toEqual(['All owners', 'Team A', 'Team B']);

    fireEvent.change(owner, { target: { value: 'Team B' } });
    expect(screen.getByText('Full beta')).toBeInTheDocument();
    expect(screen.queryByText('Full alpha')).not.toBeInTheDocument();
    expect(screen.queryByText('Summary gamma')).not.toBeInTheDocument();
  });

  it('the CWE select matches report_summary.cwe or the concern-derived CWE', async () => {
    await loaded();
    const cwe = screen.getByLabelText('CWE filter');
    const options = Array.from(cwe.querySelectorAll('option')).map((o) => o.textContent);
    expect(options).toEqual(['All CWEs', 'CWE-639', 'CWE-862']);

    fireEvent.change(cwe, { target: { value: 'CWE-639' } });
    expect(screen.getByText('Full alpha')).toBeInTheDocument();
    expect(screen.getByText('Summary gamma')).toBeInTheDocument();
    expect(screen.queryByText('Full beta')).not.toBeInTheDocument();
    expect(screen.queryByText('Summary delta')).not.toBeInTheDocument();
  });

  it('combines with the severity filter and leaves the metric totals untouched', async () => {
    const rows = [
      ...ROWS,
      { ...ROWS[0], id: 'e', summary: 'Full crit', severity: 'critical' },
    ];
    vi.spyOn(api, 'getFindings').mockResolvedValue({
      findings: rows,
      summary: { ...SUM4, total: 5, critical: 1 },
    });
    renderPage();
    await waitFor(() => expect(screen.getByText('Full alpha')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Full reports' }));
    fireEvent.click(screen.getByRole('button', { name: /critical/i }));
    expect(screen.getByText('Full crit')).toBeInTheDocument();
    expect(screen.queryByText('Full alpha')).not.toBeInTheDocument();
    // Tiles still report the unfiltered totals.
    expect(screen.getByRole('button', { name: /critical/i })).toHaveTextContent('1');
  });

  it('shows the no-matches state when the filters exclude everything', async () => {
    await loaded();
    fireEvent.change(screen.getByLabelText('Owner filter'), { target: { value: 'Team A' } });
    fireEvent.change(screen.getByLabelText('CWE filter'), { target: { value: 'CWE-862' } });
    expect(screen.getByText('No matches')).toBeInTheDocument();
  });
});

describe('Findings — export report', () => {
  const full = (id: string, summary: string, severity = 'high'): FindingOut => ({
    ...FINDING, id, summary, severity, profile: 'full',
  });
  const ROWS: FindingOut[] = [
    full('a', 'Full alpha'),
    full('b', 'Full beta', 'critical'),
    { ...FINDING, id: 'c', summary: 'Summary gamma', profile: 'summary' },
  ];
  const SUM: FindingsSummary = { total: 3, critical: 1, high: 2, medium: 0, low: 0, info: 0 };

  async function loaded() {
    vi.spyOn(api, 'getFindings').mockResolvedValue({ findings: ROWS, summary: SUM });
    renderPage();
    await waitFor(() => expect(screen.getByText('Full alpha')).toBeInTheDocument());
  }

  it('exports exactly the rows the filters leave, not the whole list', async () => {
    const exportSpy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    await loaded();
    fireEvent.click(screen.getByRole('button', { name: /critical/i }));
    fireEvent.click(screen.getByRole('button', { name: 'Export report' }));

    const dialog = screen.getByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Export' }));
    await waitFor(() => expect(exportSpy).toHaveBeenCalled());
    expect(exportSpy.mock.calls[0][0]).toEqual(
      expect.objectContaining({ format: 'md', ids: ['b'], include_summaries: false }),
    );
    expect(exportSpy.mock.calls[0][0]).not.toHaveProperty('ws_id');
  });

  it('is disabled when the filters leave nothing to export', async () => {
    await loaded();
    fireEvent.change(screen.getByLabelText('Owner filter'), { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: /medium/i }));
    await waitFor(() => expect(screen.getByText('No matches')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Export report' })).toBeDisabled();
  });

  it('is not offered while there are no findings at all', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue({
      findings: [],
      summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
    });
    renderPage();
    await waitFor(() => expect(screen.getByText('No findings')).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Export report' })).toBeNull();
  });
});
