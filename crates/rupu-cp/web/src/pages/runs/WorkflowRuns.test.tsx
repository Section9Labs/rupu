// @vitest-environment jsdom
// WorkflowRuns — verifies that the host filter drives the server request rather
// than filtering client-side over a fan-out result set.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { HostView } from '../../lib/api';
import WorkflowRuns from './WorkflowRuns';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const LOCAL_HOST: HostView = {
  id: 'local',
  name: 'Local',
  transport_kind: 'local',
  status: 'online',
  active_run_count: 0,
};
const REMOTE_HOST: HostView = {
  id: 'host_prod',
  name: 'prod',
  transport_kind: 'http_cp',
  status: 'online',
  active_run_count: 2,
};

function stubDeps() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <WorkflowRuns />
    </MemoryRouter>,
  );
}

describe('WorkflowRuns archived mode — kind-filtered fetch', () => {
  it('clicking Archived calls getArchivedRuns with kind="workflow"', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    const archivedSpy = vi.spyOn(api, 'getArchivedRuns').mockResolvedValue([]);

    renderPage();
    // Wait for initial active-tab fetch to settle.
    await waitFor(() => expect(screen.getByText('Archived')).toBeInTheDocument());

    fireEvent.click(screen.getByText('Archived'));

    await waitFor(() =>
      expect(archivedSpy).toHaveBeenCalledWith('workflow'),
    );
  });
});

describe('WorkflowRuns host filter — server-driven', () => {
  it('default fetch is called with host: "local" (fast path, not fan-out)', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'local' })),
    );
  });

  it('"All hosts" option fetches without a host param (fan-out branch)', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByLabelText('Host filter')).toBeInTheDocument());

    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: '__all__' } });

    await waitFor(() => {
      const calls = runsSpy.mock.calls;
      const lastParams = calls[calls.length - 1]?.[0];
      expect(lastParams?.host).toBeUndefined();
    });
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);

    renderPage();
    // Wait for the remote host option to appear (fetched from api.getHosts).
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'host_prod' } });

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })),
    );
  });
});
