// @vitest-environment jsdom
// AutoflowPanel — the run page's claim actions. Requeue / Release call
// api.requeueClaim / api.releaseClaim (after a window.confirm) for a local
// run; a remote run's panel hides them and says so.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowRunContext } from '../lib/api';
import AutoflowPanel from './AutoflowPanel';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const CTX: AutoflowRunContext = {
  repo_ref: 'github:acme/widgets',
  issue_ref: 'github:acme/widgets/issues/42',
  entity: '42',
  workflow_name: 'fix-issue',
  status: 'failed',
  failure: null,
  cycle_id: 'cyc_1',
  workspace_path: null,
  host_id: null,
  prior_cycles: [],
  claim: {
    issue_ref: 'github:acme/widgets/issues/42',
    issue_display_ref: 'acme/widgets#42',
    repo_ref: 'github:acme/widgets',
    issue_title: null,
    issue_url: null,
    workflow: 'fix-issue',
    status: 'blocked',
    last_run_id: 'run-9',
    last_error: null,
    last_summary: null,
    pr_url: null,
    claim_owner: null,
    lease_expires_at: null,
    updated_at: '2026-06-01T00:00:00Z',
  },
};

describe('AutoflowPanel claim actions', () => {
  it('requeues and releases a local run’s claim after confirmation', async () => {
    const requeue = vi.spyOn(api, 'requeueClaim').mockResolvedValue({ wake_id: 'w' });
    const release = vi.spyOn(api, 'releaseClaim').mockResolvedValue({ released: true });
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(
      <MemoryRouter>
        <AutoflowPanel context={CTX} />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Requeue' }));
    expect(confirm).toHaveBeenLastCalledWith('Requeue this autoflow?');
    await waitFor(() => expect(requeue).toHaveBeenCalledWith('github:acme/widgets/issues/42'));
    expect(await screen.findByText('Requeued')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Release' }));
    expect(confirm).toHaveBeenLastCalledWith('Release this claim?');
    await waitFor(() => expect(release).toHaveBeenCalledWith('github:acme/widgets/issues/42'));
    expect(await screen.findByText('Released')).toBeInTheDocument();
  });

  it('does nothing when the confirmation is declined', () => {
    const requeue = vi.spyOn(api, 'requeueClaim').mockResolvedValue({ wake_id: 'w' });
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    render(
      <MemoryRouter>
        <AutoflowPanel context={CTX} host="local" />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Requeue' }));
    expect(requeue).not.toHaveBeenCalled();
  });

  it('shows the server’s error when an action fails', async () => {
    vi.spyOn(api, 'releaseClaim').mockRejectedValue(new Error('claim is held by a live worker'));
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(
      <MemoryRouter>
        <AutoflowPanel context={CTX} />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Release' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('claim is held by a live worker');
    // The buttons come back so the operator can retry.
    expect(screen.getByRole('button', { name: 'Release' })).toBeEnabled();
  });

  it('hides claim actions for a remote run', () => {
    render(
      <MemoryRouter>
        <AutoflowPanel context={CTX} host="host_prod" />
      </MemoryRouter>,
    );
    expect(screen.queryByRole('button', { name: 'Requeue' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Release' })).not.toBeInTheDocument();
    expect(screen.getByText(/available on the host that ran/)).toBeInTheDocument();
  });

  it('shows no actions and no note when the run has no claim on record', () => {
    render(
      <MemoryRouter>
        <AutoflowPanel context={{ ...CTX, claim: null }} host="host_prod" />
      </MemoryRouter>,
    );
    expect(screen.queryByRole('button', { name: 'Requeue' })).not.toBeInTheDocument();
    expect(screen.queryByText(/available on the host that ran/)).not.toBeInTheDocument();
    expect(screen.getByText('no active claim on record')).toBeInTheDocument();
  });

  it('links to all autoflow runs', () => {
    render(
      <MemoryRouter>
        <AutoflowPanel context={CTX} />
      </MemoryRouter>,
    );
    expect(screen.getByRole('link', { name: 'All autoflow runs' })).toHaveAttribute(
      'href',
      '/runs/autoflows',
    );
  });
});
