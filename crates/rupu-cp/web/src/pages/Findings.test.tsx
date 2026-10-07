// @vitest-environment jsdom
// Global Findings — One Control Language migration (Phase 3, Task H). Covers
// the kit loading/empty states (the metric-tile severity filter is kept
// as-is — it's an intentional filter surface, not a FilterBar) and that the
// FindingsTable subject column (Summary) truncates per the table rules.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type FindingOut, type FindingsResponse } from '../lib/api';

import Findings from './Findings';

function renderPage(entry = '/findings') {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Findings />
    </MemoryRouter>,
  );
}

const SEV_ORDER = ['critical', 'high', 'medium', 'low', 'info'];

/** A well-formed response: the severity facets are derived from the rows. */
function resp(findings: FindingOut[], over: Partial<FindingsResponse> = {}): FindingsResponse {
  const count = (s: string) => findings.filter((f) => f.severity === s).length;
  return {
    findings,
    summary: {
      total: findings.length,
      critical: count('critical'),
      high: count('high'),
      medium: count('medium'),
      low: count('low'),
      info: count('info'),
    },
    facets: { severity: SEV_ORDER.map((value) => ({ value, count: count(value) })) },
    tags_unavailable: [],
    ...over,
  };
}

const FINDING: FindingOut = {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
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
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([]));
    renderPage();

    await waitFor(() => expect(screen.getByText('No findings')).toBeInTheDocument());
    expect(
      screen.getByText(/run an assessment workflow to start recording findings/i),
    ).toBeInTheDocument();
  });


  it('renders the kit ErrorBanner on fetch failure', async () => {
    vi.spyOn(api, 'getFindings').mockRejectedValue(new Error('boom'));
    renderPage();

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
  });
});

describe('Findings — table rules', () => {
  it('the Summary column is the one flexible/truncating subject column', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING]));
    renderPage();

    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());

    const subjectCell = screen.getByText(FINDING.summary).closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector(`[title="${FINDING.summary}"]`)).toBeInTheDocument();
  });
});

describe('Findings — query bar', () => {
  it('sends ?q= to the server', async () => {
    const spy = vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING]));
    renderPage('/security?tab=findings&q=tag%3Aneeds-poc');
    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy).toHaveBeenCalledWith({ q: 'tag:needs-poc' });
  });

  it('fetches with no arguments when the query is empty', async () => {
    const spy = vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING]));
    renderPage();
    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(spy).toHaveBeenCalledWith();
  });

  it('a severity tile toggles a severity: token', async () => {
    const spy = vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING]));
    renderPage();
    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: /filter by high/i }));
    await waitFor(() =>
      expect(spy.mock.calls[spy.mock.calls.length - 1][0]).toEqual({ q: 'severity:high' }),
    );

    fireEvent.click(screen.getByRole('button', { name: /filter by high/i }));
    await waitFor(() => expect(spy.mock.calls[spy.mock.calls.length - 1]).toEqual([]));
  });

  it('a severity tile replaces an existing severity token and keeps the rest', async () => {
    const spy = vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING]));
    renderPage('/security?tab=findings&q=severity%3Alow%20tag%3Aneeds-poc');
    await waitFor(() => expect(spy).toHaveBeenCalled());

    fireEvent.click(screen.getByRole('button', { name: /filter by high/i }));
    await waitFor(() =>
      expect(spy.mock.calls[spy.mock.calls.length - 1][0]).toEqual({ q: 'tag:needs-poc severity:high' }),
    );
  });

  it('warns which projects have unreadable tags', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(
      resp([{ ...FINDING, ws_id: 'ws1', project: 'billing-api' }], { tags_unavailable: ['ws1'] }),
    );
    renderPage();
    const banner = await screen.findByText(/couldn't be read/i);
    expect(banner).toHaveTextContent('billing-api');
  });

  it('falls back to the workspace id when no row names the project', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING], { tags_unavailable: ['ws-gone'] }));
    renderPage();
    expect(await screen.findByText(/couldn't be read/i)).toHaveTextContent('ws-gone');
  });

  it('shows no banner when every tag log was readable', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING]));
    renderPage();
    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());
    expect(screen.queryByText(/couldn't be read/i)).toBeNull();
  });

  it('a locally invalid query is flagged on its chip and never fetched', async () => {
    const spy = vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING]));
    renderPage('/security?tab=findings&q=sevrity%3Ax');
    const chip = await screen.findByText('sevrity:x');
    expect(chip.closest('span[title]')?.className).toMatch(/text-err/);
    expect(spy).not.toHaveBeenCalled();
  });

  it('says which query matched nothing', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([]));
    renderPage('/security?tab=findings&q=tag%3Anope');
    expect(await screen.findByText('No matches')).toBeInTheDocument();
    expect(screen.getByText(/No findings match/)).toHaveTextContent('tag:nope');
  });

  it('shows a server rejection of the query', async () => {
    vi.spyOn(api, 'getFindings').mockRejectedValue(new Error('bad query'));
    renderPage('/security?tab=findings&q=tag%3Aa');
    expect(await screen.findByRole('alert')).toHaveTextContent('bad query');
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

  async function loaded() {
    // A fake server: honours a lone `severity:<x>` token, like the real one.
    vi.spyOn(api, 'getFindings').mockImplementation(async (opts) => {
      const m = /severity:(\w+)/.exec(opts?.q ?? '');
      return resp(m ? ROWS.filter((r) => r.severity === m[1]) : ROWS);
    });
    renderPage();
    await waitFor(() => expect(screen.getByText('Full alpha')).toBeInTheDocument());
  }

  it('exports exactly the rows the filters leave, not the whole list', async () => {
    const exportSpy = vi.spyOn(api, 'exportFindings').mockResolvedValue(new Blob(['x']));
    await loaded();
    fireEvent.click(screen.getByRole('button', { name: /filter by critical/i }));
    await waitFor(() => expect(screen.queryByText('Full alpha')).not.toBeInTheDocument());
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
    fireEvent.click(screen.getByRole('button', { name: /filter by medium/i }));
    await waitFor(() => expect(screen.getByText('No matches')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Export report' })).toBeDisabled();
  });

  it('is not offered while there are no findings at all', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([]));
    renderPage();
    await waitFor(() => expect(screen.getByText('No findings')).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Export report' })).toBeNull();
  });
});
