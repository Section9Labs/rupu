// @vitest-environment jsdom
// Global Findings — One Control Language migration (Phase 3, Task H). Covers
// the kit loading/empty states (the metric-tile severity filter is kept
// as-is — it's an intentional filter surface, not a FilterBar) and that the
// FindingsTable subject column (Summary) truncates per the table rules.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
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
