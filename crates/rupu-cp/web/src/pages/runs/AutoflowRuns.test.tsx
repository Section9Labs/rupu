// @vitest-environment jsdom
// AutoflowRuns — the Claims tab lists active autoflow claims, each with
// Requeue + Release controls. The page's mount fetch (events + cycles) is
// stubbed so the test can switch to Claims and drive the per-row actions.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowClaim } from '../../lib/api';
import AutoflowRuns from './AutoflowRuns';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const CLAIM: AutoflowClaim = {
  issue_ref: 'github:acme/widgets#42',
  issue_display_ref: 'acme/widgets#42',
  repo_ref: 'github:acme/widgets',
  issue_title: 'Flaky retry path',
  issue_url: 'https://example.test/issues/42',
  workflow: 'fix-issue',
  status: 'await_human',
  last_run_id: 'run-9',
  last_error: null,
  last_summary: 'Waiting on reviewer',
  pr_url: 'https://example.test/pr/7',
  claim_owner: 'worker-1',
  lease_expires_at: null,
  updated_at: '2026-06-01T00:00:00Z',
};

function stubPage() {
  vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <AutoflowRuns />
    </MemoryRouter>,
  );
}

describe('AutoflowRuns — Claims tab', () => {
  it('renders a claim with its workflow and status, and lists Requeue + Release', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([CLAIM]);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));

    await waitFor(() => expect(screen.getByText('acme/widgets#42')).toBeInTheDocument());
    expect(screen.getByText('fix-issue')).toBeInTheDocument();
    expect(screen.getByText('Await Human')).toBeInTheDocument();
    expect(screen.getByText('Requeue')).toBeInTheDocument();
    expect(screen.getByText('Release')).toBeInTheDocument();
  });

  it('shows the empty state when there are no claims', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));

    await waitFor(() => expect(screen.getByText('No active claims')).toBeInTheDocument());
  });

  it('calls releaseClaim(issue_ref) when Release is confirmed', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([CLAIM]);
    const release = vi.spyOn(api, 'releaseClaim').mockResolvedValue({ released: true });
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));
    await waitFor(() => expect(screen.getByText('Release')).toBeInTheDocument());

    fireEvent.click(screen.getByText('Release'));
    await waitFor(() => expect(release).toHaveBeenCalledWith('github:acme/widgets#42'));
  });

  it('calls requeueClaim(issue_ref) when Requeue is confirmed', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([CLAIM]);
    const requeue = vi.spyOn(api, 'requeueClaim').mockResolvedValue({ wake_id: 'wake-1' });
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));
    await waitFor(() => expect(screen.getByText('Requeue')).toBeInTheDocument());

    fireEvent.click(screen.getByText('Requeue'));
    await waitFor(() => expect(requeue).toHaveBeenCalledWith('github:acme/widgets#42'));
  });
});
