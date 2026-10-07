// @vitest-environment jsdom
// Global Findings — One Control Language migration (Phase 3, Task H). Covers
// the kit loading/empty states (the metric-tile severity filter is kept
// as-is — it's an intentional filter surface, not a FilterBar) and that the
// FindingsTable subject column (Summary) truncates per the table rules.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useNavigate } from 'react-router-dom';
import { api, ApiError, type FindingOut, type FindingsResponse, type TagAcrossResult } from '../lib/api';

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
      resp([{ ...FINDING, ws_id: 'ws1', project: 'billing-api' }], {
        tags_unavailable: [{ ws_id: 'ws1', project: 'billing-api' }],
      }),
    );
    renderPage();
    const banner = await screen.findByText(/couldn't be read/i);
    expect(banner).toHaveTextContent('billing-api');
  });

  it('names the project even when no row of that workspace is in the answer', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(
      resp([FINDING], { tags_unavailable: [{ ws_id: 'ws_bill', project: 'billing-api' }] }),
    );
    renderPage();
    const banner = await screen.findByText(/couldn't be read/i);
    expect(banner).toHaveTextContent('billing-api');
    expect(banner).not.toHaveTextContent('ws_bill');
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

  it('replaces stale results with a visible error when the query turns invalid', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([FINDING]));
    function Go() {
      const navigate = useNavigate();
      return <button onClick={() => navigate('/security?tab=findings&q=sevrity%3Ax')}>go-bad</button>;
    }
    render(
      <MemoryRouter initialEntries={['/security?tab=findings&q=tag%3Aa']}>
        <Go />
        <Findings />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());

    fireEvent.click(screen.getByText('go-bad'));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/unknown/i);
    expect(screen.queryByText(FINDING.summary)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Export report' })).toBeNull();
    expect(screen.queryByRole('button', { name: /filter by high/i })).toBeNull();
  });

  it('drops an earlier server error when the query turns invalid', async () => {
    vi.spyOn(api, 'getFindings').mockRejectedValue(new Error('server said no'));
    function Go() {
      const navigate = useNavigate();
      return <button onClick={() => navigate('/security?tab=findings&q=sevrity%3Ax')}>go-bad</button>;
    }
    render(
      <MemoryRouter initialEntries={['/security?tab=findings&q=tag%3Aa']}>
        <Go />
        <Findings />
      </MemoryRouter>,
    );
    expect(await screen.findByText('server said no')).toBeInTheDocument();
    fireEvent.click(screen.getByText('go-bad'));
    await waitFor(() => expect(screen.queryByText('server said no')).toBeNull());
    expect(screen.getByRole('alert')).toHaveTextContent(/unknown/i);
  });

  /** The page under a router, with buttons that navigate to `q` values. */
  function renderWithNav(start: string, targets: Record<string, string>) {
    function Nav() {
      const navigate = useNavigate();
      return (
        <>
          {Object.entries(targets).map(([label, q]) => (
            <button key={label} onClick={() => navigate(`/security?tab=findings&q=${encodeURIComponent(q)}`)}>
              {label}
            </button>
          ))}
        </>
      );
    }
    return render(
      <MemoryRouter initialEntries={[`/security?tab=findings&q=${encodeURIComponent(start)}`]}>
        <Nav />
        <Findings />
      </MemoryRouter>,
    );
  }

  it('drops the old rows and export when the server rejects the next query', async () => {
    vi.spyOn(api, 'getFindings').mockImplementation(async (opts) => {
      if (opts?.q === 'tag:b') throw new Error('server rejected tag:b');
      return resp([FINDING]);
    });
    renderWithNav('tag:a', { 'go-b': 'tag:b' });
    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Export report' })).toBeInTheDocument();

    fireEvent.click(screen.getByText('go-b'));

    expect(await screen.findByText('server rejected tag:b')).toBeInTheDocument();
    expect(screen.queryByText(FINDING.summary)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Export report' })).toBeNull();
    expect(screen.queryByRole('button', { name: /filter by high/i })).toBeNull();
    // The bar stays, so the query can be fixed.
    expect(screen.getByRole('combobox', { name: 'Filter findings' })).toBeInTheDocument();
  });

  it("never shows the previous query's rows while the next one loads", async () => {
    const OTHER: FindingOut = { ...FINDING, id: 'f2', summary: 'Hardcoded token in the deploy script' };
    let release: (r: FindingsResponse) => void = () => {};
    vi.spyOn(api, 'getFindings').mockImplementation((opts) =>
      opts?.q === 'tag:b'
        ? new Promise<FindingsResponse>((r) => {
            release = r;
          })
        : Promise.resolve(resp([FINDING])),
    );
    renderWithNav('tag:a', { 'go-b': 'tag:b' });
    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());

    fireEvent.click(screen.getByText('go-b'));

    await waitFor(() => expect(screen.queryByText(FINDING.summary)).not.toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Export report' })).toBeNull();
    expect(screen.getByRole('combobox', { name: 'Filter findings' })).toBeInTheDocument();
    expect(screen.getByText('Loading findings…')).toBeInTheDocument();

    release(resp([OTHER]));
    expect(await screen.findByText(OTHER.summary)).toBeInTheDocument();
    expect(screen.queryByText('Loading findings…')).toBeNull();
  });

  it('an invalid query fixed back to a valid one fetches and shows its rows', async () => {
    const OTHER: FindingOut = { ...FINDING, id: 'f2', summary: 'Hardcoded token in the deploy script' };
    const spy = vi
      .spyOn(api, 'getFindings')
      .mockImplementation(async (opts) => resp(opts?.q === 'tag:b' ? [OTHER] : [FINDING]));
    renderWithNav('tag:a', { 'go-bad': 'sevrity:x', 'go-b': 'tag:b' });
    await waitFor(() => expect(screen.getByText(FINDING.summary)).toBeInTheDocument());

    fireEvent.click(screen.getByText('go-bad'));
    expect(await screen.findByRole('alert')).toHaveTextContent(/unknown/i);

    fireEvent.click(screen.getByText('go-b'));
    expect(await screen.findByText(OTHER.summary)).toBeInTheDocument();
    expect(spy).toHaveBeenLastCalledWith({ q: 'tag:b' });
    expect(screen.queryByText(FINDING.summary)).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('fixing an invalid shared query keeps the bar mounted while the fetch is pending', async () => {
    vi.spyOn(api, 'getFindings').mockImplementation(() => new Promise<FindingsResponse>(() => {}));
    renderWithNav('sevrity:x', { 'go-b': 'tag:b' });
    const bar = screen.getByRole('combobox', { name: 'Filter findings' });
    expect(await screen.findByRole('alert')).toHaveTextContent(/unknown/i);

    fireEvent.click(screen.getByText('go-b'));

    await waitFor(() => expect(api.getFindings).toHaveBeenCalledWith({ q: 'tag:b' }));
    expect(screen.getByRole('combobox', { name: 'Filter findings' })).toBe(bar);
    expect(bar).toBeInTheDocument();
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

describe('Findings — bulk tagging', () => {
  const A: FindingOut = { ...FINDING, id: 'fa', summary: 'Alpha issue' };
  const B: FindingOut = { ...FINDING, id: 'fb', summary: 'Beta issue' };
  const outcome = (id: string) => ({
    workspaces: [{ ws_id: 'ws-1', outcomes: [{ finding_id: id, before: [], after: ['triaged'] }] }],
    unknown: [],
  });

  it('selects a row, tags it, reloads, clears the selection and keeps the result', async () => {
    const get = vi.spyOn(api, 'getFindings').mockResolvedValue(resp([A, B]));
    const tag = vi.spyOn(api, 'tagFindings').mockResolvedValue(outcome('fa'));
    renderPage();
    await waitFor(() => expect(screen.getByText('Alpha issue')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fa' }));
    expect(screen.getByText('1 selected')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Tag…' }));
    const input = screen.getByRole('combobox', { name: 'Tag selected findings' });
    fireEvent.change(input, { target: { value: 'triaged' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(tag).toHaveBeenCalledWith(['fa'], { add: ['triaged'] }));
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByText('1 selected')).toBeNull());
    expect(screen.getByRole('status')).toHaveTextContent('Tagged 1 finding.');
    expect(screen.getByRole('checkbox', { name: 'Select fa' })).not.toBeChecked();
  });

  /** Opens Tag… on the bar and submits `tag`. */
  function tagSelected(tag: string) {
    fireEvent.click(screen.getByRole('button', { name: 'Tag…' }));
    const input = screen.getByRole('combobox', { name: 'Tag selected findings' });
    fireEvent.change(input, { target: { value: tag } });
    fireEvent.keyDown(input, { key: 'Enter' });
  }

  it('keeps a row ticked while a bulk change was in flight', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([A, B]));
    let finish!: (r: TagAcrossResult) => void;
    vi.spyOn(api, 'tagFindings').mockImplementation(() => new Promise((r) => { finish = r; }));
    renderPage();
    await waitFor(() => expect(screen.getByText('Alpha issue')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fa' }));
    tagSelected('triaged');
    await waitFor(() => expect(finish).toBeDefined());
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fb' }));

    await act(async () => finish(outcome('fa')));
    await waitFor(() => expect(screen.getByText('1 selected')).toBeInTheDocument());
    expect(screen.getByRole('checkbox', { name: 'Select fb' })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Select fa' })).not.toBeChecked();
  });

  it('sends a selection over the server limit in batches; a batch of deleted findings reads as gone', async () => {
    const rows = Array.from({ length: 1001 }, (_, i): FindingOut => ({ ...FINDING, id: `f${i}`, summary: `Issue ${i}` }));
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp(rows));
    // f1000 was deleted since the list loaded: the server answers its batch,
    // where every id is unknown, with a 404.
    const gone = JSON.stringify({ error: 'unknown finding id(s): f1000' });
    const tag = vi.spyOn(api, 'tagFindings').mockImplementation(async (ids: string[]) => {
      if (ids.includes('f1000')) throw new ApiError(404, gone, gone);
      return {
        workspaces: [{ ws_id: 'ws-1', outcomes: ids.map((id) => ({ finding_id: id, before: [], after: ['triaged'] })) }],
        unknown: [],
      };
    });
    renderPage();
    await waitFor(() => expect(screen.getByText('Issue 0')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('checkbox', { name: 'Select all' }));
    expect(screen.getByText('1001 selected')).toBeInTheDocument();
    tagSelected('triaged');

    await waitFor(() => expect(tag).toHaveBeenCalledTimes(2));
    expect(tag.mock.calls[0][0]).toHaveLength(1000);
    expect(tag.mock.calls[1][0]).toEqual(['f1000']);
    expect(await screen.findByRole('alert')).toHaveTextContent('Tagged 1000 findings. 1 finding no longer exists.');
  }, 30000);

  it('stops at a failed batch and says what was applied before it', async () => {
    const rows = Array.from({ length: 1001 }, (_, i): FindingOut => ({ ...FINDING, id: `f${i}`, summary: `Issue ${i}` }));
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp(rows));
    const tag = vi.spyOn(api, 'tagFindings')
      .mockImplementationOnce(async (ids: string[]) => ({
        workspaces: [{ ws_id: 'ws-1', outcomes: ids.map((id) => ({ finding_id: id, before: [], after: ['triaged'] })) }],
        unknown: [],
      }))
      .mockRejectedValueOnce(new Error('server went away'));
    renderPage();
    await waitFor(() => expect(screen.getByText('Issue 0')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('checkbox', { name: 'Select all' }));
    tagSelected('triaged');

    await waitFor(() => expect(tag).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      "Tagged 1000 findings. The other 1 finding wasn't changed: server went away",
    );
    // Only what was applied leaves the selection.
    expect(screen.getByText('1 selected')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Select f1000' })).toBeChecked();
  }, 30000);

  it("never shows a bulk result from an earlier query on the next one's list", async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([A, B]));
    let finish!: (r: TagAcrossResult) => void;
    const tag = vi.spyOn(api, 'tagFindings').mockImplementation(() => new Promise((r) => { finish = r; }));
    function Nav() {
      const navigate = useNavigate();
      return <button onClick={() => navigate('/findings?q=tag%3Ab')}>go-b</button>;
    }
    render(
      <MemoryRouter initialEntries={['/findings?q=tag%3Aa']}>
        <Nav />
        <Findings />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('Alpha issue')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fa' }));
    tagSelected('triaged');
    await waitFor(() => expect(tag).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByText('go-b'));
    await waitFor(() => expect(screen.queryByText('1 selected')).toBeNull());
    await act(async () => finish(outcome('fa')));

    await waitFor(() => expect(api.getFindings).toHaveBeenCalledWith({ q: 'tag:b' }));
    expect(screen.queryByText(/Tagged 1 finding/)).toBeNull();
  });

  it('keeps the bulk result when the reload after it fails', async () => {
    vi.spyOn(api, 'getFindings')
      .mockResolvedValueOnce(resp([A, B]))
      .mockRejectedValueOnce(new Error('list unavailable'));
    vi.spyOn(api, 'tagFindings').mockResolvedValue(outcome('fa'));
    renderPage();
    await waitFor(() => expect(screen.getByText('Alpha issue')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fa' }));
    tagSelected('triaged');

    expect(await screen.findByText('list unavailable')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Tagged 1 finding.');
  });

  it('neither counts nor sends a selected row whose tags became unreadable', async () => {
    const C: FindingOut = { ...FINDING, id: 'fc', summary: 'Gamma issue' };
    const B2: FindingOut = { ...B, ws_id: 'ws-2', project: 'billing-api' };
    vi.spyOn(api, 'getFindings')
      .mockResolvedValueOnce(resp([A, B2, C]))
      .mockResolvedValue(resp([A, B2, C], { tags_unavailable: [{ ws_id: 'ws-2', project: 'billing-api' }] }));
    let finish!: (r: TagAcrossResult) => void;
    const tag = vi.spyOn(api, 'tagFindings')
      .mockImplementationOnce(() => new Promise((r) => { finish = r; }))
      .mockResolvedValue(outcome('fc'));
    renderPage();
    await waitFor(() => expect(screen.getByText('Alpha issue')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fa' }));
    tagSelected('triaged');
    await waitFor(() => expect(finish).toBeDefined());
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fb' }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fc' }));
    await act(async () => finish(outcome('fa')));

    // The reload marks ws-2 unreadable: fb drops out of the count.
    await waitFor(() => expect(screen.getByRole('checkbox', { name: 'Select fb' })).toBeDisabled());
    expect(screen.getByText('1 selected')).toBeInTheDocument();
    tagSelected('triaged');
    await waitFor(() => expect(tag).toHaveBeenCalledTimes(2));
    expect(tag.mock.calls[1][0]).toEqual(['fc']);
  });

  it('disables the checkbox of a finding whose workspace tags are unreadable', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(
      resp([A, { ...B, ws_id: 'ws-2', project: 'billing-api' }], {
        tags_unavailable: [{ ws_id: 'ws-2', project: 'billing-api' }],
      }),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('Alpha issue')).toBeInTheDocument());
    expect(screen.getByRole('checkbox', { name: 'Select fa' })).toBeEnabled();
    const blocked = screen.getByRole('checkbox', { name: 'Select fb' });
    expect(blocked).toBeDisabled();
    expect(blocked.getAttribute('title')).toMatch(/billing-api/);
  });

  it('clears the selection when the query changes', async () => {
    vi.spyOn(api, 'getFindings').mockResolvedValue(resp([A, B]));
    function Nav() {
      const navigate = useNavigate();
      return <button onClick={() => navigate('/security?tab=findings&q=tag%3Ab')}>go-b</button>;
    }
    render(
      <MemoryRouter initialEntries={['/security?tab=findings&q=tag%3Aa']}>
        <Nav />
        <Findings />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('Alpha issue')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select fa' }));
    expect(screen.getByText('1 selected')).toBeInTheDocument();

    fireEvent.click(screen.getByText('go-b'));
    await waitFor(() => expect(screen.queryByText('1 selected')).toBeNull());
    await waitFor(() => expect(screen.getByRole('checkbox', { name: 'Select fa' })).not.toBeChecked());
  });
});
