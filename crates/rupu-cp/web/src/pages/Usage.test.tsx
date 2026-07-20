// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Usage from './Usage';
import { api, type UsageResponse, type OutlierRun, type UsageRunRow } from '../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function usageResponse(overrides: Partial<UsageResponse> = {}): UsageResponse {
  return {
    summary: {
      input_tokens: 1000,
      output_tokens: 500,
      cached_tokens: 0,
      total_tokens: 1500,
      cost_usd: 4.5,
      priced: true,
      runs: 3,
    },
    breakdown: [
      {
        provider: 'anthropic',
        model: '',
        agent: '',
        workflow: 'nightly-review',
        host_id: '',
        workspace_id: '',
        input_tokens: 1000,
        output_tokens: 500,
        cached_tokens: 0,
        total_tokens: 1500,
        cost_usd: 4.5,
        priced: true,
        runs: 3,
      },
    ],
    unpriced: { models: [], rows: 0 },
    hosts: [
      { host_id: 'local', name: 'local', transport_kind: 'local', state: 'ok', captured_at: new Date().toISOString(), reason: null },
    ],
    ...overrides,
  };
}

function runRow(overrides: Partial<UsageRunRow> = {}): UsageRunRow {
  return {
    run_id: 'run_1',
    started_at: new Date().toISOString(),
    workflow_name: 'nightly-review',
    agent: 'reviewer',
    provider: 'anthropic',
    model: 'claude',
    workspace_id: 'ws_1',
    host_id: 'local',
    input_tokens: 1000,
    output_tokens: 500,
    cached_tokens: 0,
    total_tokens: 1500,
    cost_usd: 4.5,
    priced: true,
    ...overrides,
  };
}

function renderUsage() {
  return render(
    <MemoryRouter>
      <Usage />
    </MemoryRouter>,
  );
}

function mockAll(opts: {
  usage?: UsageResponse;
  runs?: UsageRunRow[];
  outliers?: OutlierRun[];
} = {}) {
  vi.spyOn(api, 'getUsage').mockResolvedValue(opts.usage ?? usageResponse());
  vi.spyOn(api, 'getUsageRuns').mockResolvedValue(opts.runs ?? [runRow()]);
  vi.spyOn(api, 'getUsageOutliers').mockResolvedValue(opts.outliers ?? []);
}

describe('Usage page', () => {
  it('loads the model pivot by default and renders the headline spend', async () => {
    mockAll();

    renderUsage();

    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'model'));
    await waitFor(() => expect(api.getUsageRuns).toHaveBeenCalledWith('30d'));
    // Both the headline and the breakdown table's total render "$4.50" —
    // assert on presence, not a single unique match.
    await waitFor(() => expect(screen.getAllByText('$4.50').length).toBeGreaterThan(0));
  });

  it('renders the graph from the flat run rows (not from a separate timeline fetch)', async () => {
    mockAll({ runs: [runRow({ model: 'claude' })] });

    renderUsage();

    // The stacked-area legend renders the pivot key ('claude', the default
    // model pivot) once the run rows have been bucketed by buildTimeline.
    // The breakdown table (built from the same runs) renders it too, so
    // assert on presence rather than a single unique match.
    await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));
    expect(screen.queryByText(/No usage recorded yet/)).not.toBeInTheDocument();
  });

  it('re-fetches with the new pivot when the pivot picker changes', async () => {
    mockAll();

    renderUsage();
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'model'));

    fireEvent.click(screen.getByRole('button', { name: 'workflow' }));

    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'workflow'));
    expect(await screen.findByText('Breakdown by Workflow')).toBeInTheDocument();
  });

  it('does not refetch getUsageRuns when the pivot changes — buildTimeline just re-stacks in memory', async () => {
    mockAll();

    renderUsage();
    await waitFor(() => expect(api.getUsageRuns).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: 'workflow' }));
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'workflow'));

    // Give any stray effect a tick, then assert the runs fetch was never repeated.
    await waitFor(() => expect(screen.getByText('Breakdown by Workflow')).toBeInTheDocument());
    expect(api.getUsageRuns).toHaveBeenCalledTimes(1);
  });

  it('shows the unpriced banner, naming the models, when the gap is non-zero', async () => {
    mockAll({ usage: usageResponse({ unpriced: { models: ['mystery-model'], rows: 7 } }) });

    renderUsage();

    expect(await screen.findByText(/mystery-model/)).toBeInTheDocument();
    expect(screen.getByText(/1 model unpriced/)).toBeInTheDocument();
  });

  it('does not render the unpriced banner when everything is priced', async () => {
    mockAll();

    renderUsage();

    await screen.findByText('Cost outliers');
    expect(screen.queryByText(/unpriced/)).not.toBeInTheDocument();
  });

  it('lists outlier runs linking to /runs/:id', async () => {
    const outlier = { run_id: 'run-42', workflow_name: 'nightly-review', cost_usd: 12, baseline_usd: 3, ratio: 4, started_at: new Date().toISOString() };
    mockAll({ outliers: [outlier] });

    renderUsage();

    const link = await screen.findByRole('link', { name: 'nightly-review' });
    expect(link).toHaveAttribute('href', '/runs/run-42');
  });

  it('shows the "no outliers" message when the outlier list is empty', async () => {
    mockAll();

    renderUsage();

    expect(await screen.findByText(/No cost outliers in this window/)).toBeInTheDocument();
  });

  it('switches the requested range when a range button is clicked', async () => {
    mockAll();

    renderUsage();
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'model'));

    fireEvent.click(screen.getByRole('button', { name: '7d' }));

    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('7d', 'model'));
    await waitFor(() => expect(api.getUsageOutliers).toHaveBeenCalledWith('7d'));
    await waitFor(() => expect(api.getUsageRuns).toHaveBeenCalledWith('7d'));
  });

  it('shows an error state without a prior successful load', async () => {
    vi.spyOn(api, 'getUsage').mockRejectedValue(new Error('boom'));
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);

    renderUsage();

    expect(await screen.findByText(/Could not load usage/)).toBeInTheDocument();
  });

  it('labels the local-only timeline graph and outliers panel, and maps the host pivot to friendly names, on a multi-host fleet', async () => {
    mockAll({
      usage: usageResponse({
        hosts: [
          { host_id: 'local', name: 'local', transport_kind: 'local', state: 'ok', captured_at: new Date().toISOString(), reason: null },
          { host_id: 'host_01KWREMOTE', name: 'staging-box', transport_kind: 'http_cp', state: 'ok', captured_at: new Date().toISOString(), reason: null },
        ],
      }),
      // The breakdown table is now built from these same run rows (Fix 1),
      // not from `data.breakdown` above — the run needs the host id under
      // test for the table's host-pivot mapping to have anything to map.
      runs: [runRow({ host_id: 'host_01KWREMOTE' })],
    });

    renderUsage();

    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'model'));

    // The timeline graph (local-only, no host fan-out) is labeled, distinct
    // from the fleet-wide headline number above it.
    expect(screen.getByText(/local host only/i)).toBeInTheDocument();
    // The outliers panel (also local-only) is labeled too.
    expect(screen.getByText(/this host only/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'host' }));
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'host'));

    // The host-pivot breakdown row shows the friendly host name, not the raw
    // id — scoped to the breakdown table since the freshness strip above
    // also renders each host's `name`.
    const table = await screen.findByRole('table');
    expect(await within(table).findByText('staging-box')).toBeInTheDocument();
    expect(within(table).queryByText('host_01KWREMOTE')).not.toBeInTheDocument();
  });

  // `data.breakdown` (from `getUsage`, drives the checkbox table) and the
  // flat run rows (from `getUsageRuns`, drive the graph via `buildTimeline`)
  // are two independently-mocked payloads in these tests — they must agree
  // on the pivot-key strings (`model: 'claude'` / `'gpt'`) for a checkbox
  // toggle here to have any observable effect on the graph's legend.
  function modelBreakdown(model: string, cost: number): UsageResponse['breakdown'][number] {
    return {
      provider: 'anthropic', model, agent: '', workflow: '', host_id: '', workspace_id: '',
      input_tokens: 1000, output_tokens: 500, cached_tokens: 0, total_tokens: 1500,
      cost_usd: cost, priced: true, runs: 1,
    };
  }

  describe('interactive filtering', () => {
    // Root-cause regression: the breakdown table used to be built from
    // `data.breakdown` (fleet-wide, from `GET /api/usage`) while the graph
    // was built from `getUsageRuns` — two different datasets that happened
    // to agree in every other test in this file because both mocks were
    // set up with matching model strings. Here they're deliberately made to
    // disagree: `data.breakdown` names a model that appears in NO run row.
    // If the table were still sourced from `data.breakdown`, it would show
    // "nowhere-in-runs" (and toggling it would do nothing to the graph,
    // which has no such series). After Fix 1 the table is built from the
    // same run rows as the graph, so it must show "claude" instead, and
    // toggling it must exclude it from both the table's own exclusion
    // count and the graph's legend.
    it('builds the breakdown table from the same run rows as the graph, not from data.breakdown', async () => {
      mockAll({
        usage: usageResponse({ breakdown: [modelBreakdown('nowhere-in-runs', 99)] }),
        runs: [runRow({ model: 'claude' })],
      });

      renderUsage();

      const table = await screen.findByRole('table');
      expect(await within(table).findByText('claude')).toBeInTheDocument();
      expect(within(table).queryByText('nowhere-in-runs')).not.toBeInTheDocument();

      fireEvent.click(within(table).getByRole('checkbox', { name: 'claude' }));

      expect(await screen.findByText('Excluded (1) · reset')).toBeInTheDocument();
      // The sole run was `claude` — excluding it empties the graph entirely
      // (proving the toggle reached `buildTimeline`'s shared row source,
      // rather than a no-op key the graph never had).
      await waitFor(() => expect(screen.getByText(/No usage recorded yet/)).toBeInTheDocument());
    });

    it('toggling a breakdown row shows "Excluded (1)" and does not refetch getUsageRuns', async () => {
      mockAll({
        usage: usageResponse({ breakdown: [modelBreakdown('claude', 3), modelBreakdown('gpt', 1.5)] }),
        runs: [runRow({ model: 'claude' }), runRow({ run_id: 'run_2', model: 'gpt' })],
      });

      renderUsage();
      await waitFor(() => expect(api.getUsageRuns).toHaveBeenCalledTimes(1));
      await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));

      const table = screen.getByRole('table');
      fireEvent.click(within(table).getByRole('checkbox', { name: 'claude' }));

      expect(await screen.findByText('Excluded (1) · reset')).toBeInTheDocument();
      expect(api.getUsageRuns).toHaveBeenCalledTimes(1);
      // The excluded model's series disappears from the graph's legend (the
      // table row itself still shows "claude", struck through, so assert on
      // the legend specifically).
      const legend = screen.getByRole('list');
      await waitFor(() => expect(within(legend).queryByText('claude')).not.toBeInTheDocument());
      expect(within(legend).getByText('gpt')).toBeInTheDocument();
    });

    it('toggling an outlier excludes its run and shows the chip', async () => {
      const outlier: OutlierRun = {
        run_id: 'run_1',
        workflow_name: 'nightly-review',
        cost_usd: 900,
        baseline_usd: 1,
        ratio: 900,
        started_at: new Date().toISOString(),
      };
      mockAll({ runs: [runRow({ run_id: 'run_1' })], outliers: [outlier] });

      renderUsage();
      await screen.findByRole('link', { name: 'nightly-review' });

      fireEvent.click(screen.getByRole('checkbox', { name: 'run_1' }));

      expect(await screen.findByText('Excluded (1) · reset')).toBeInTheDocument();
    });

    it('reset clears both excluded sets', async () => {
      mockAll({
        usage: usageResponse({ breakdown: [modelBreakdown('claude', 3)] }),
        runs: [runRow({ model: 'claude' })],
      });

      renderUsage();
      // Before any exclusion: the table row + the graph legend both say
      // "claude" (2 matches).
      await waitFor(() => expect(screen.getAllByText('claude')).toHaveLength(2));

      const table = screen.getByRole('table');
      fireEvent.click(within(table).getByRole('checkbox', { name: 'claude' }));
      const chip = await screen.findByText('Excluded (1) · reset');
      // Excluded: the graph's only series vanishes (empty state), so only the
      // struck-through table row still says "claude" (1 match).
      await waitFor(() => expect(screen.getAllByText('claude')).toHaveLength(1));

      fireEvent.click(chip);

      expect(screen.queryByText(/Excluded \(/)).not.toBeInTheDocument();
      // The series is back once the exclusion is cleared — 2 matches again.
      await waitFor(() => expect(screen.getAllByText('claude')).toHaveLength(2));
    });

    it('changing pivot re-stacks the graph without refetching getUsageRuns', async () => {
      mockAll({
        runs: [runRow({ workflow_name: 'nightly-scan' }), runRow({ run_id: 'run_2', workflow_name: 'pr-review' })],
      });

      renderUsage();
      await waitFor(() => expect(api.getUsageRuns).toHaveBeenCalledTimes(1));
      // Both the graph legend and the (now run-sourced) breakdown table
      // render "claude" — assert on presence, not a single unique match.
      await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));

      fireEvent.click(screen.getByRole('button', { name: 'workflow' }));

      await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'workflow'));
      await waitFor(() => expect(screen.getAllByText('nightly-scan').length).toBeGreaterThan(0));
      expect(screen.getAllByText('pr-review').length).toBeGreaterThan(0);
      expect(api.getUsageRuns).toHaveBeenCalledTimes(1);
    });
  });
});
