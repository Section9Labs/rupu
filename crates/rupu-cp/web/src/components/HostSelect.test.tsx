// @vitest-environment jsdom
// HostSelect — lists hosts from api.getHosts(), defaults to local, emits
// chosen id via onChange. Falls back to a plain "Local" option on error.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api, type HostView } from '../lib/api';
import HostSelect from './HostSelect';

const LOCAL: HostView = {
  id: 'local',
  name: 'Local',
  transport_kind: 'local',
  status: 'online',
  active_run_count: 0,
};
const REMOTE: HostView = {
  id: 'h-abc',
  name: 'staging',
  transport_kind: 'http_cp',
  status: 'online',
  active_run_count: 2,
};
const STALE_REMOTE: HostView = {
  id: 'h-xyz',
  name: 'prod',
  transport_kind: 'http_cp',
  status: 'stale',
  active_run_count: 0,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('HostSelect', () => {
  it('defaults to the local host and renders all options after load', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL, REMOTE]);
    const onChange = vi.fn();

    render(<HostSelect value="local" onChange={onChange} />);

    // Initial render shows the fallback option (hosts not yet loaded).
    expect(screen.getByLabelText('Host')).toBeInTheDocument();

    // After the async fetch resolves, both hosts appear.
    await waitFor(() => expect(screen.getByText('Local')).toBeInTheDocument());
    expect(screen.getByText('staging')).toBeInTheDocument();
    expect(screen.getByLabelText('Host')).toHaveValue('local');
  });

  it('emits the chosen host id when selection changes', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL, REMOTE]);
    const onChange = vi.fn();

    render(<HostSelect value="local" onChange={onChange} />);

    await waitFor(() => screen.getByText('staging'));

    fireEvent.change(screen.getByLabelText('Host'), { target: { value: 'h-abc' } });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith('h-abc');
  });

  it('shows an offline/stale status indicator in the option label', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL, STALE_REMOTE]);

    render(<HostSelect value="local" onChange={vi.fn()} />);

    await waitFor(() => screen.getByText(/prod/));
    // stale hosts include the status in parens
    expect(screen.getByText(/prod \(stale\)/)).toBeInTheDocument();
    // online hosts do NOT have a status suffix
    expect(screen.queryByText(/Local \(online\)/)).not.toBeInTheDocument();
  });

  it('shows a plain "Local" fallback when getHosts fails', async () => {
    vi.spyOn(api, 'getHosts').mockRejectedValue(new Error('network error'));

    render(<HostSelect value="local" onChange={vi.fn()} />);

    // Falls back to the static Local option (no error visible to user).
    await waitFor(() => expect(screen.getByText('Local')).toBeInTheDocument());
    expect(screen.getByLabelText('Host')).toHaveValue('local');
  });

  it('is disabled when the disabled prop is set', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL]);

    render(<HostSelect value="local" onChange={vi.fn()} disabled />);

    expect(screen.getByLabelText('Host')).toBeDisabled();
  });
});
