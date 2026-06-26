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
});
