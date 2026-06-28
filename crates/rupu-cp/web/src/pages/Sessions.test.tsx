// @vitest-environment jsdom
// Sessions — verifies that the host filter drives the server request rather
// than filtering client-side, and that the Host column renders host_id.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';
import type { SessionSummary, HostView } from '../lib/api';
import Sessions from './Sessions';

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
  active_run_count: 1,
};

const REMOTE_SESSION: SessionSummary = {
  session_id: 'sess-abc123',
  agent_name: 'fix-bug',
  model: 'claude-3-5-sonnet',
  status: 'active',
  total_turns: 5,
  created_at: '2026-06-01T00:00:00Z',
  updated_at: '2026-06-01T01:00:00Z',
  scope: 'active',
  host_id: 'host_prod',
};

function stubDeps() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <Sessions />
    </MemoryRouter>,
  );
}

describe('Sessions host filter — server-driven', () => {
  it('default fetch is called with host: "local" (fast path, not fan-out)', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();

    await waitFor(() =>
      expect(sessionsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'local' })),
    );
  });

  it('"All hosts" option fetches without a host param (fan-out branch)', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByLabelText('Host filter')).toBeInTheDocument());

    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: '__all__' } });

    await waitFor(() => {
      const calls = sessionsSpy.mock.calls;
      const lastParams = calls[calls.length - 1]?.[0];
      expect(lastParams?.host).toBeUndefined();
    });
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);

    renderPage();
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'host_prod' } });

    await waitFor(() =>
      expect(sessionsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })),
    );
  });

  it('Host column renders host_id from the row', async () => {
    stubDeps();
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);

    renderPage();

    await waitFor(() => expect(screen.getByText('host_prod')).toBeInTheDocument());
  });

  it('Host column falls back to "local" when host_id is absent', async () => {
    stubDeps();
    const localSession: SessionSummary = { ...REMOTE_SESSION, host_id: undefined };
    vi.spyOn(api, 'getSessions').mockResolvedValue([localSession]);

    renderPage();

    await waitFor(() => expect(screen.getByText('local')).toBeInTheDocument());
  });
});
