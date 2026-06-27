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

import LauncherSheet, { repoToOption } from './LauncherSheet';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('repoToOption', () => {
  it('maps a RepoEntry to a ComboboxOption with platform:repo value', () => {
    expect(
      repoToOption({ platform: 'github', repo: 'o/r', default_branch: 'main', private: false }),
    ).toEqual({ value: 'github:o/r', label: 'o/r' });
  });
});

describe('LauncherSheet', () => {
  it('launches with the declared input + chosen mode, then navigates to the run', async () => {
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
    vi.spyOn(api, 'launchRun').mockRejectedValue(new Error('no launcher'));

    render(<LauncherSheet workflow="audit" declaredInputs={['target_dir']} onClose={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Launch' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('no launcher');
    expect(navigateMock).not.toHaveBeenCalled();
  });

  it('calls launchRun with working_dir when Directory mode is selected', async () => {
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    vi.spyOn(api, 'browseDir').mockResolvedValue({ path: '/h', parent: null, dirs: [] });
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    const launchSpy = vi.spyOn(api, 'launchRun').mockResolvedValue({ run_id: 'run-dir' });

    render(<LauncherSheet workflow="deploy" declaredInputs={[]} onClose={() => {}} />);

    fireEvent.click(screen.getByRole('button', { name: 'Directory' }));

    fireEvent.change(screen.getByLabelText('Directory path'), {
      target: { value: '/tmp/myproject' },
    });

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

  it('calls launchRun with target when Repository mode is selected', async () => {
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    const launchSpy = vi.spyOn(api, 'launchRun').mockResolvedValue({ run_id: 'run-repo' });

    render(<LauncherSheet workflow="deploy" declaredInputs={[]} onClose={() => {}} />);

    fireEvent.click(screen.getByRole('button', { name: 'Repository' }));

    fireEvent.change(screen.getByLabelText('Target'), {
      target: { value: 'github:owner/repo' },
    });

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
