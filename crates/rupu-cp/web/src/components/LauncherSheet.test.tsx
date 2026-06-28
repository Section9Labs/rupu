// @vitest-environment jsdom
// LauncherSheet — fills a declared input, picks a mode, launches, and asserts
// the launch call shape + navigation to the new run. `api.launchRun` and
// react-router's `useNavigate` are both stubbed.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api } from '../lib/api';

// Stub useNavigate — keep the rest of react-router-dom intact.
const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

import LauncherSheet from './LauncherSheet';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('LauncherSheet', () => {
  it('launches with the declared input + chosen mode, then navigates to the run', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    const launchSpy = vi.spyOn(api, 'launchRun').mockResolvedValue({ run_id: 'run-xyz' });

    render(<LauncherSheet workflow="audit" declaredInputs={['target_dir']} onClose={() => {}} />);

    // Fill the declared input.
    fireEvent.change(screen.getByLabelText('Input target_dir'), {
      target: { value: 'src/lib' },
    });
    // Pick Bypass.
    fireEvent.change(screen.getByLabelText('Permission mode'), { target: { value: 'bypass' } });

    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));

    await waitFor(() =>
      expect(launchSpy).toHaveBeenCalledWith('audit', {
        inputs: { target_dir: 'src/lib' },
        mode: 'bypass',
        target: undefined,
        working_dir: undefined,
      }),
    );
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/runs/run-xyz'));
  });

  it('surfaces an error and does not navigate when the launch fails', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    vi.spyOn(api, 'launchRun').mockRejectedValue(new Error('no launcher'));

    render(<LauncherSheet workflow="audit" declaredInputs={['target_dir']} onClose={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('no launcher');
    expect(navigateMock).not.toHaveBeenCalled();
  });

  it('renders the TargetPicker and sends working_dir when a directory item is selected', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    vi.spyOn(api, 'browseDir').mockResolvedValue({ path: '/tmp/myproject', parent: null, dirs: [] });
    const launchSpy = vi.spyOn(api, 'launchRun').mockResolvedValue({ run_id: 'run-dir' });

    render(<LauncherSheet workflow="deploy" declaredInputs={[]} onClose={() => {}} />);

    // Type a path into the TargetPicker; inferFreeTextItem creates a directory item.
    const picker = screen.getByPlaceholderText('search projects, repos, or a path…');
    fireEvent.focus(picker);
    fireEvent.change(picker, { target: { value: '/tmp/myproject' } });
    // The directory item is the first (and only matching) result; select it with Enter.
    fireEvent.keyDown(picker, { key: 'Enter' });

    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));

    await waitFor(() =>
      expect(launchSpy).toHaveBeenCalledWith('deploy', {
        inputs: undefined,
        mode: 'ask',
        target: undefined,
        working_dir: '/tmp/myproject',
      }),
    );
  });

  it('remote host launch: passes host to launchRun and carries ?host= in navigate', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([
      { id: 'local', name: 'Local', transport_kind: 'local', status: 'online', active_run_count: 0 },
      { id: 'host_x', name: 'host_x', transport_kind: 'http_cp', status: 'online', active_run_count: 0 },
    ]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    const launchSpy = vi.spyOn(api, 'launchRun').mockResolvedValue({ run_id: 'run-remote' });

    render(<LauncherSheet workflow="audit" declaredInputs={[]} onClose={() => {}} />);

    // Wait for HostSelect to populate with the remote option.
    await waitFor(() => expect(screen.getByRole('option', { name: 'host_x' })).toBeInTheDocument());

    // Select the remote host.
    fireEvent.change(screen.getByLabelText('Host'), { target: { value: 'host_x' } });

    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));

    await waitFor(() =>
      expect(launchSpy).toHaveBeenCalledWith('audit', expect.objectContaining({ host: 'host_x' })),
    );
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/runs/run-remote?host=host_x'));
  });

  it('local host launch: does not append ?host= to navigate', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([
      { id: 'local', name: 'Local', transport_kind: 'local', status: 'online', active_run_count: 0 },
    ]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    vi.spyOn(api, 'launchRun').mockResolvedValue({ run_id: 'run-local' });

    render(<LauncherSheet workflow="audit" declaredInputs={[]} onClose={() => {}} />);

    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));

    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/runs/run-local'));
  });

  it('sends target when a repo item is selected via the TargetPicker', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([]);
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    const launchSpy = vi.spyOn(api, 'launchRun').mockResolvedValue({ run_id: 'run-repo' });

    render(<LauncherSheet workflow="deploy" declaredInputs={[]} onClose={() => {}} />);

    // Type a repo ref; inferFreeTextItem creates a repo item.
    const picker = screen.getByPlaceholderText('search projects, repos, or a path…');
    fireEvent.focus(picker);
    fireEvent.change(picker, { target: { value: 'github:owner/repo' } });
    fireEvent.keyDown(picker, { key: 'Enter' });

    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));

    await waitFor(() =>
      expect(launchSpy).toHaveBeenCalledWith('deploy', {
        inputs: undefined,
        mode: 'ask',
        target: 'github:owner/repo',
        working_dir: undefined,
      }),
    );
  });
});
