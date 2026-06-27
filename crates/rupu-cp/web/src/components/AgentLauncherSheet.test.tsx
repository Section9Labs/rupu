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
      mode: 'ask',
    }));
    expect(launchSpy).not.toHaveBeenCalled();
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/sessions/ses-001'));
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
