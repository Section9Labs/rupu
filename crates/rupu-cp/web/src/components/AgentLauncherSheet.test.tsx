// @vitest-environment jsdom
// AgentLauncherSheet — tests for the Single-run / Session launch-kind toggle.
// Pure-logic `buildAgentLaunch` tests are kept in the same file.
//
// Stubs: api.launchAgent, api.startSession, api.getRepos, api.getProjects,
// api.browseDir (TargetPicker deps), and useNavigate.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api } from '../lib/api';
import { WORKSPACE_ITEM, type TargetItem } from '../lib/targetItems';
import { buildAgentLaunch } from './AgentLauncherSheet';

// Stub useNavigate — keep the rest of react-router-dom intact.
const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

import AgentLauncherSheet from './AgentLauncherSheet';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

// ---------------------------------------------------------------------------
// Pure-logic tests (no DOM)
// ---------------------------------------------------------------------------

describe('buildAgentLaunch', () => {
  it('directory item sends working_dir only', () => {
    const dirItem: TargetItem = { kind: 'directory', label: '/tmp/x', resolved: { working_dir: '/tmp/x' } };
    expect(buildAgentLaunch('hi', 'ask', dirItem)).toEqual({
      prompt: 'hi', mode: 'ask', working_dir: '/tmp/x',
    });
  });
  it('repo item sends target only; blank prompt omitted', () => {
    const repoItem: TargetItem = { kind: 'repo', label: 'github:o/r', resolved: { target: 'github:o/r' } };
    expect(buildAgentLaunch('  ', 'bypass', repoItem)).toEqual({
      mode: 'bypass', target: 'github:o/r',
    });
  });
  it('workspace item sends neither target nor dir', () => {
    expect(buildAgentLaunch('go', 'ask', WORKSPACE_ITEM)).toEqual({ prompt: 'go', mode: 'ask' });
  });
});

// ---------------------------------------------------------------------------
// Component tests
// ---------------------------------------------------------------------------

function stubTargetPickerDeps() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([]);
  vi.spyOn(api, 'getRepos').mockResolvedValue([]);
  vi.spyOn(api, 'getProjects').mockResolvedValue([]);
  vi.spyOn(api, 'browseDir').mockResolvedValue({ path: '/', parent: null, dirs: [] });
}

describe('AgentLauncherSheet toggle', () => {
  it('default (run) path: calls launchAgent and navigates to /runs/:id', async () => {
    stubTargetPickerDeps();
    const launchSpy = vi.spyOn(api, 'launchAgent').mockResolvedValue({ run_id: 'run-abc' });
    const sessionSpy = vi.spyOn(api, 'startSession').mockResolvedValue({ session_id: 'ses-xyz' });

    render(<AgentLauncherSheet agent="triage" onClose={() => {}} />);

    fireEvent.change(screen.getByLabelText('Prompt'), { target: { value: 'fix the bug' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));

    await waitFor(() => expect(launchSpy).toHaveBeenCalledWith('triage', {
      prompt: 'fix the bug',
      mode: 'ask',
    }));
    expect(sessionSpy).not.toHaveBeenCalled();
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/runs/run-abc'));
  });

  it('session path: calls startSession (not launchAgent) and navigates to /sessions/:id', async () => {
    stubTargetPickerDeps();
    const launchSpy = vi.spyOn(api, 'launchAgent').mockResolvedValue({ run_id: 'run-abc' });
    const sessionSpy = vi.spyOn(api, 'startSession').mockResolvedValue({ session_id: 'ses-001' });

    render(<AgentLauncherSheet agent="triage" onClose={() => {}} />);

    // Switch to Session mode.
    fireEvent.click(screen.getByRole('button', { name: 'Session' }));

    fireEvent.change(screen.getByLabelText('Prompt'), { target: { value: 'hello session' } });
    fireEvent.click(screen.getByRole('button', { name: 'Start session' }));

    await waitFor(() => expect(sessionSpy).toHaveBeenCalledWith('triage', {
      prompt: 'hello session',
      mode: 'bypass',
    }));
    expect(launchSpy).not.toHaveBeenCalled();
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/sessions/ses-001'));
  });

  it('switching to session resets ask mode to bypass and never submits ask', async () => {
    stubTargetPickerDeps();
    const sessionSpy = vi.spyOn(api, 'startSession').mockResolvedValue({ session_id: 'ses-bypass' });
    vi.spyOn(api, 'launchAgent').mockResolvedValue({ run_id: 'run-abc' });

    render(<AgentLauncherSheet agent="triage" onClose={() => {}} />);

    // Default mode is 'ask'; switch to session without touching the mode select.
    fireEvent.click(screen.getByRole('button', { name: 'Session' }));

    fireEvent.click(screen.getByRole('button', { name: 'Start session' }));

    await waitFor(() => expect(sessionSpy).toHaveBeenCalled());
    const calledMode = (sessionSpy.mock.calls[0][1] as { mode: string }).mode;
    expect(calledMode).not.toBe('ask');
    expect(calledMode).toBe('bypass');
  });

  it('remote host run: passes host to launchAgent and carries ?host= in navigate', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([
      { id: 'local', name: 'Local', transport_kind: 'local', status: 'online', active_run_count: 0 },
      { id: 'host_x', name: 'host_x', transport_kind: 'http_cp', status: 'online', active_run_count: 0 },
    ]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'browseDir').mockResolvedValue({ path: '/', parent: null, dirs: [] });
    const launchSpy = vi.spyOn(api, 'launchAgent').mockResolvedValue({ run_id: 'run-remote' });

    render(<AgentLauncherSheet agent="triage" onClose={() => {}} />);

    // Wait for HostSelect to populate then pick the remote host.
    await waitFor(() => expect(screen.getByRole('option', { name: 'host_x' })).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Host'), { target: { value: 'host_x' } });

    fireEvent.click(screen.getByRole('button', { name: 'Run' }));

    await waitFor(() =>
      expect(launchSpy).toHaveBeenCalledWith('triage', expect.objectContaining({ host: 'host_x' })),
    );
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/runs/run-remote?host=host_x'));
  });

  it('local run: navigate URL has no ?host= query string', async () => {
    stubTargetPickerDeps();
    vi.spyOn(api, 'launchAgent').mockResolvedValue({ run_id: 'run-local' });

    render(<AgentLauncherSheet agent="triage" onClose={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));

    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/runs/run-local'));
  });

  it('session kind is ENABLED for a remote host', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([
      { id: 'local', name: 'Local', transport_kind: 'local', status: 'online', active_run_count: 0 },
      { id: 'host_x', name: 'host_x', transport_kind: 'http_cp', status: 'online', active_run_count: 0 },
    ]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'browseDir').mockResolvedValue({ path: '/', parent: null, dirs: [] });

    render(<AgentLauncherSheet agent="triage" onClose={() => {}} />);

    await waitFor(() => expect(screen.getByRole('option', { name: 'host_x' })).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Host'), { target: { value: 'host_x' } });

    // Session button must be ENABLED for remote hosts.
    const sessionBtn = screen.getByRole('button', { name: 'Session' });
    expect(sessionBtn).not.toBeDisabled();
    // The old "local only" note must not appear.
    expect(screen.queryByText(/Sessions run on the local host only/i)).not.toBeInTheDocument();
  });

  it('remote session launch: calls startSession with host and navigates to /sessions/:id?host=', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([
      { id: 'local', name: 'Local', transport_kind: 'local', status: 'online', active_run_count: 0 },
      { id: 'host_x', name: 'host_x', transport_kind: 'http_cp', status: 'online', active_run_count: 0 },
    ]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'browseDir').mockResolvedValue({ path: '/', parent: null, dirs: [] });
    const sessionSpy = vi.spyOn(api, 'startSession').mockResolvedValue({ session_id: 'ses-remote-1' });
    vi.spyOn(api, 'launchAgent').mockResolvedValue({ run_id: 'run-remote' });

    render(<AgentLauncherSheet agent="triage" onClose={() => {}} />);

    await waitFor(() => expect(screen.getByRole('option', { name: 'host_x' })).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Host'), { target: { value: 'host_x' } });

    // Switch to session mode on the remote host.
    fireEvent.click(screen.getByRole('button', { name: 'Session' }));

    fireEvent.click(screen.getByRole('button', { name: 'Start session' }));

    await waitFor(() =>
      expect(sessionSpy).toHaveBeenCalledWith('triage', expect.objectContaining({ host: 'host_x' })),
    );
    await waitFor(() =>
      expect(navigateMock).toHaveBeenCalledWith('/sessions/ses-remote-1?host=host_x'),
    );
  });

  it('shows the session hint only in session mode', () => {
    stubTargetPickerDeps();

    render(<AgentLauncherSheet agent="triage" onClose={() => {}} />);

    expect(screen.queryByText(/multi-turn chat/i)).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Session' }));
    expect(screen.getByText(/multi-turn chat/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Single run' }));
    expect(screen.queryByText(/multi-turn chat/i)).toBeNull();
  });
});
