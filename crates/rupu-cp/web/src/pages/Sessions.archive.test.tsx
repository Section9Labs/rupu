// @vitest-environment jsdom
// Sessions — archive/restore/delete row actions, now rendered via the kit's
// `Button` `ring`/`ring-danger` variants (same handlers/confirmations).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type SessionSummary, type HostView } from '../lib/api';
import Sessions from './Sessions';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// Real SessionSummary fixture — active scope (default tab).
const ACTIVE_SESSION: SessionSummary = {
  session_id: 'sess-abc123',
  agent_name: 'fix-bug',
  model: 'claude-3-5-sonnet',
  status: 'active',
  total_turns: 5,
  created_at: '2026-06-01T00:00:00Z',
  updated_at: '2026-06-01T01:00:00Z',
  scope: 'active',
  host_id: 'local',
};

const ARCHIVED_SESSION: SessionSummary = {
  ...ACTIVE_SESSION,
  scope: 'archived',
};

const LOCAL_HOST: HostView = {
  id: 'local',
  name: 'Local',
  transport_kind: 'local',
  status: 'online',
  active_run_count: 0,
};

function stubHosts() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <Sessions />
    </MemoryRouter>,
  );
}

describe('Sessions row archive/delete', () => {
  it('archives an active session from the row action (no confirm)', async () => {
    stubHosts();
    vi.spyOn(api, 'getSessions').mockResolvedValue([ACTIVE_SESSION]);
    const archive = vi.spyOn(api, 'archiveSession').mockResolvedValue();
    const confirmSpy = vi.spyOn(window, 'confirm');

    renderPage();

    const archiveBtn = await screen.findByRole('button', { name: /archive session/i });
    // Rendered via the kit ring-button idiom (compact ring pill).
    expect(archiveBtn.className).toMatch(/ring-1/);
    fireEvent.click(archiveBtn);

    // Threads the row's own host_id (operator-reported gap: a fanned-out
    // remote-host row must not silently hit the local session mutator).
    await waitFor(() => expect(archive).toHaveBeenCalledWith('sess-abc123', 'local'));
    // Archive must NOT gate behind window.confirm.
    expect(confirmSpy).not.toHaveBeenCalled();
  });

  it('deletes an active session after confirm', async () => {
    stubHosts();
    vi.spyOn(api, 'getSessions').mockResolvedValue([ACTIVE_SESSION]);
    const del = vi.spyOn(api, 'deleteSession').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();

    const deleteBtn = await screen.findByRole('button', { name: /delete session/i });
    expect(deleteBtn.className).toMatch(/ring-1/);
    fireEvent.click(deleteBtn);

    expect(window.confirm).toHaveBeenCalled();
    await waitFor(() => expect(del).toHaveBeenCalledWith('sess-abc123', 'local'));
  });

  it('does not delete when confirm is cancelled', async () => {
    stubHosts();
    vi.spyOn(api, 'getSessions').mockResolvedValue([ACTIVE_SESSION]);
    const del = vi.spyOn(api, 'deleteSession').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: /delete session/i }));

    expect(window.confirm).toHaveBeenCalled();
    await new Promise((r) => setTimeout(r, 50));
    expect(del).not.toHaveBeenCalled();
  });

  it('restores an archived session from the Archived tab row action', async () => {
    stubHosts();
    // First call (active tab): empty; second call (archived tab): one archived session.
    vi.spyOn(api, 'getSessions')
      .mockResolvedValueOnce([])
      .mockResolvedValue([ARCHIVED_SESSION]);
    const restore = vi.spyOn(api, 'restoreSession').mockResolvedValue();

    renderPage();

    // Switch to the Archived tab.
    fireEvent.click(await screen.findByRole('button', { name: 'Archived' }));

    fireEvent.click(await screen.findByRole('button', { name: /restore session/i }));
    await waitFor(() => expect(restore).toHaveBeenCalledWith('sess-abc123', 'local'));
  });

  it('archives a remote-host session with that host id, not local', async () => {
    stubHosts();
    const REMOTE_SESSION: SessionSummary = { ...ACTIVE_SESSION, host_id: 'host_prod' };
    vi.spyOn(api, 'getSessions').mockResolvedValue([REMOTE_SESSION]);
    const archive = vi.spyOn(api, 'archiveSession').mockResolvedValue();

    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: /archive session/i }));

    await waitFor(() => expect(archive).toHaveBeenCalledWith('sess-abc123', 'host_prod'));
  });
});
