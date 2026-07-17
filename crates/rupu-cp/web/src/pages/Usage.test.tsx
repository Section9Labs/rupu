// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Usage from './Usage';
import { api, type UsageResponse, type OutlierRun } from '../lib/api';

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

function renderUsage() {
  return render(
    <MemoryRouter>
      <Usage />
    </MemoryRouter>,
  );
}

describe('Usage page', () => {
  it('loads the model pivot by default and renders the headline spend', async () => {
    vi.spyOn(api, 'getUsage').mockResolvedValue(usageResponse());
    vi.spyOn(api, 'getUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);

    renderUsage();

    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'model'));
    // Both the headline and the breakdown table's total render "$4.50" —
    // assert on presence, not a single unique match.
    await waitFor(() => expect(screen.getAllByText('$4.50').length).toBeGreaterThan(0));
  });

  it('re-fetches with the new pivot when the pivot picker changes', async () => {
    vi.spyOn(api, 'getUsage').mockResolvedValue(usageResponse());
    vi.spyOn(api, 'getUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);

    renderUsage();
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'model'));

    fireEvent.click(screen.getByRole('button', { name: 'workflow' }));

    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'workflow'));
    expect(await screen.findByText('Breakdown by Workflow')).toBeInTheDocument();
  });

  it('shows the unpriced banner, naming the models, when the gap is non-zero', async () => {
    vi.spyOn(api, 'getUsage').mockResolvedValue(
      usageResponse({ unpriced: { models: ['mystery-model'], rows: 7 } }),
    );
    vi.spyOn(api, 'getUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);

    renderUsage();

    expect(await screen.findByText(/mystery-model/)).toBeInTheDocument();
    expect(screen.getByText(/1 model unpriced/)).toBeInTheDocument();
  });

  it('does not render the unpriced banner when everything is priced', async () => {
    vi.spyOn(api, 'getUsage').mockResolvedValue(usageResponse());
    vi.spyOn(api, 'getUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);

    renderUsage();

    await screen.findByText('Cost outliers');
    expect(screen.queryByText(/unpriced/)).not.toBeInTheDocument();
  });

  it('lists outlier runs linking to /runs/:id', async () => {
    const outlier: OutlierRun = {
      run_id: 'run-42',
      workflow_name: 'nightly-review',
      cost_usd: 12,
      baseline_usd: 3,
      ratio: 4,
      started_at: new Date().toISOString(),
    };
    vi.spyOn(api, 'getUsage').mockResolvedValue(usageResponse());
    vi.spyOn(api, 'getUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([outlier]);

    renderUsage();

    const link = await screen.findByRole('link', { name: 'nightly-review' });
    expect(link).toHaveAttribute('href', '/runs/run-42');
  });

  it('shows the "no outliers" message when the outlier list is empty', async () => {
    vi.spyOn(api, 'getUsage').mockResolvedValue(usageResponse());
    vi.spyOn(api, 'getUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);

    renderUsage();

    expect(await screen.findByText(/No cost outliers in this window/)).toBeInTheDocument();
  });

  it('switches the requested range when a range button is clicked', async () => {
    vi.spyOn(api, 'getUsage').mockResolvedValue(usageResponse());
    vi.spyOn(api, 'getUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);

    renderUsage();
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('30d', 'model'));

    fireEvent.click(screen.getByRole('button', { name: '7d' }));

    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith('7d', 'model'));
    await waitFor(() => expect(api.getUsageOutliers).toHaveBeenCalledWith('7d'));
  });

  it('shows an error state without a prior successful load', async () => {
    vi.spyOn(api, 'getUsage').mockRejectedValue(new Error('boom'));
    vi.spyOn(api, 'getUsageTimeline').mockResolvedValue([]);
    vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);

    renderUsage();

    expect(await screen.findByText(/Could not load usage/)).toBeInTheDocument();
  });
});
