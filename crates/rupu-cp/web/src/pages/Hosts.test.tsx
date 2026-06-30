// @vitest-environment jsdom
// Hosts list — rows from mocked getHosts; local pinned first; Add Host form
// calls addHost; Remove button absent for local. Tunnel node enrollment shows
// the one-time command + token-once warning.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type HostView } from '../lib/api';

import Hosts from './Hosts';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const LOCAL_HOST: HostView = {
  id: 'local',
  name: 'local',
  transport_kind: 'local',
  status: 'online',
  version: '0.9.0',
  active_run_count: 0,
  last_seen_at: new Date().toISOString(),
};

const REMOTE_HOST: HostView = {
  id: 'host-abc123',
  name: 'prod-east',
  transport_kind: 'http_cp',
  base_url: 'https://rupu.prod-east.example.com',
  status: 'online',
  version: '0.9.0',
  active_run_count: 2,
  last_seen_at: new Date().toISOString(),
};

const TUNNEL_HOST: HostView = {
  id: 'node_01JZTESTNODE001',
  name: 'my-box',
  transport_kind: 'tunnel',
  status: 'offline',
  active_run_count: 0,
};

const SSH_HOST: HostView = {
  id: 'host_01JZTESTSSH001',
  name: 'prod-ssh',
  transport_kind: 'ssh',
  status: 'online',
  active_run_count: 0,
};

const BUCKET_HOST: HostView = {
  id: 'host_01JZTESTBKT001',
  name: 'drop-bucket',
  transport_kind: 'bucket',
  status: 'online',
  active_run_count: 0,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('Hosts page', () => {
  it('renders both hosts from getHosts', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // "local" appears multiple times (name, shortId, transport badge) — use
    // findAllByText and just assert at least one element is in the document.
    const localEls = await screen.findAllByText('local');
    expect(localEls.length).toBeGreaterThan(0);
    // "prod-east" is unique — appears only once as the host name
    expect(await screen.findByText('prod-east')).toBeInTheDocument();
  });

  it('shows local host before remote host (local pinned first)', async () => {
    // Pass remote first, then local — the sort should still put local first
    vi.spyOn(api, 'getHosts').mockResolvedValue([REMOTE_HOST, LOCAL_HOST]);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // Wait for both rows to be in the DOM
    await screen.findByText('prod-east');
    // "local" appears multiple times — findAllByText works here
    await screen.findAllByText('local');

    // tbody rows — first data row should be the local host
    const rows = document.querySelectorAll('tbody tr');
    expect(rows.length).toBeGreaterThanOrEqual(2);
    const firstRowText = rows[0].textContent ?? '';
    expect(firstRowText).toContain('local');
    // Remote host should be in the second row
    const secondRowText = rows[1].textContent ?? '';
    expect(secondRowText).toContain('prod-east');
  });

  it('does NOT render a Remove button for the local host', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    await screen.findByText('prod-east');

    // Only one Remove button — for the remote host, not for local
    const removeButtons = screen.queryAllByRole('button', { name: /remove host/i });
    expect(removeButtons).toHaveLength(1);
    expect(removeButtons[0]).toHaveAccessibleName('Remove host prod-east');
  });

  it('Add Host form submits and calls api.addHost', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    const addSpy = vi.spyOn(api, 'addHost').mockResolvedValue({
      ...REMOTE_HOST,
      id: 'host-new1',
      name: 'staging',
      base_url: 'https://rupu.staging.example.com',
    });

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // Open the Add host form
    fireEvent.click(await screen.findByRole('button', { name: 'Add host' }));

    // Fill in the form
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'staging' } });
    fireEvent.change(screen.getByLabelText('Base URL'), {
      target: { value: 'https://rupu.staging.example.com' },
    });
    fireEvent.change(screen.getByLabelText(/Token/i), {
      target: { value: 'my-token' },
    });

    // Submit
    fireEvent.click(screen.getByRole('button', { name: 'Add host' }));

    await waitFor(() => expect(addSpy).toHaveBeenCalledTimes(1));
    expect(addSpy.mock.calls[0][0]).toMatchObject({
      name: 'staging',
      base_url: 'https://rupu.staging.example.com',
      token: 'my-token',
    });

    // List must refresh after a successful add (initial load + post-add refresh)
    await waitFor(() => expect(vi.mocked(api.getHosts)).toHaveBeenCalledTimes(2));
  });

  it('shows loading state before data arrives', () => {
    // Never resolves
    vi.spyOn(api, 'getHosts').mockImplementation(() => new Promise(() => {}));

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    expect(screen.getByText(/loading hosts/i)).toBeInTheDocument();
  });

  it('shows error when getHosts fails', async () => {
    vi.spyOn(api, 'getHosts').mockRejectedValue(new Error('network failure'));

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    expect(await screen.findByText(/network failure/i)).toBeInTheDocument();
  });

  // ── Tunnel node enrollment ──────────────────────────────────────────────────

  it('tunnel mode: calls enrollNode and shows command + token-once warning', async () => {
    const FAKE_TOKEN = 'abc123deadbeef';
    const FAKE_COMMAND = `rupu node --cp-url wss://<your-cp-host>:7878 --token ${FAKE_TOKEN}`;

    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    const enrollSpy = vi.spyOn(api, 'enrollNode').mockResolvedValue({
      host: TUNNEL_HOST,
      command: FAKE_COMMAND,
      token: FAKE_TOKEN,
    });

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // Open the Add host form.
    fireEvent.click(await screen.findByRole('button', { name: 'Add host' }));

    // Select "Tunnel node" transport.
    fireEvent.click(screen.getByRole('radio', { name: /tunnel node/i }));

    // Fill in the name.
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'my-box' } });

    // Submit.
    fireEvent.click(screen.getByRole('button', { name: /enroll node/i }));

    // enrollNode must be called with the trimmed name.
    await waitFor(() => expect(enrollSpy).toHaveBeenCalledTimes(1));
    expect(enrollSpy.mock.calls[0][0]).toMatchObject({ name: 'my-box' });

    // The command must appear in the one-time panel.
    expect(await screen.findByText(FAKE_COMMAND)).toBeInTheDocument();

    // The token-once warning must render.
    expect(screen.getByText(/token shown once/i)).toBeInTheDocument();
  });

  // ── SSH host ────────────────────────────────────────────────────────────────

  it('ssh mode: calls addSshHost with correct args and refreshes list', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    const addSshSpy = vi.spyOn(api, 'addSshHost').mockResolvedValue(SSH_HOST);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // Open the Add host form.
    fireEvent.click(await screen.findByRole('button', { name: 'Add host' }));

    // Select "SSH host" transport.
    fireEvent.click(screen.getByRole('radio', { name: /ssh host/i }));

    // Fill in name and destination.
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'prod-ssh' } });
    fireEvent.change(screen.getByLabelText('Destination'), { target: { value: 'deploy@prod.example.com' } });
    fireEvent.change(screen.getByLabelText(/^Port/i), { target: { value: '2222' } });

    // Submit.
    fireEvent.click(screen.getByRole('button', { name: /add ssh host/i }));

    await waitFor(() => expect(addSshSpy).toHaveBeenCalledTimes(1));
    expect(addSshSpy.mock.calls[0][0]).toMatchObject({
      name: 'prod-ssh',
      host: 'deploy@prod.example.com',
      port: 2222,
    });

    // Form closes after success.
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: /add ssh host/i })).not.toBeInTheDocument(),
    );

    // List must refresh after a successful add.
    await waitFor(() => expect(vi.mocked(api.getHosts)).toHaveBeenCalledTimes(2));
  });

  it('ssh mode: omits port when blank and identity_file when blank', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    const addSshSpy = vi.spyOn(api, 'addSshHost').mockResolvedValue(SSH_HOST);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'Add host' }));
    fireEvent.click(screen.getByRole('radio', { name: /ssh host/i }));
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'prod-ssh' } });
    fireEvent.change(screen.getByLabelText('Destination'), { target: { value: 'user@host.example.com' } });
    // leave Port and Identity file blank
    fireEvent.click(screen.getByRole('button', { name: /add ssh host/i }));

    await waitFor(() => expect(addSshSpy).toHaveBeenCalledTimes(1));
    const call = addSshSpy.mock.calls[0][0];
    expect(call.port).toBeUndefined();
    expect(call.identity_file).toBeUndefined();
  });

  // ── Bucket host ─────────────────────────────────────────────────────────────

  it('bucket mode: calls addBucketHost with correct args and refreshes list', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    const addBucketSpy = vi.spyOn(api, 'addBucketHost').mockResolvedValue(BUCKET_HOST);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // Open the Add host form.
    fireEvent.click(await screen.findByRole('button', { name: 'Add host' }));

    // Select "Bucket (dead-drop)" transport.
    fireEvent.click(screen.getByRole('radio', { name: /bucket/i }));

    // Fill in name and bucket URL.
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'drop-bucket' } });
    fireEvent.change(screen.getByLabelText('Bucket URL'), { target: { value: 's3://my-bucket' } });
    fireEvent.change(screen.getByLabelText(/Prefix/i), { target: { value: 'rupu/drops/' } });

    // Submit.
    fireEvent.click(screen.getByRole('button', { name: /add bucket host/i }));

    await waitFor(() => expect(addBucketSpy).toHaveBeenCalledTimes(1));
    expect(addBucketSpy.mock.calls[0][0]).toMatchObject({
      name: 'drop-bucket',
      url: 's3://my-bucket',
      prefix: 'rupu/drops/',
    });

    // Form closes after success.
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: /add bucket host/i })).not.toBeInTheDocument(),
    );

    // List must refresh after a successful add.
    await waitFor(() => expect(vi.mocked(api.getHosts)).toHaveBeenCalledTimes(2));
  });

  it('bucket mode: omits prefix when blank', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    const addBucketSpy = vi.spyOn(api, 'addBucketHost').mockResolvedValue(BUCKET_HOST);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'Add host' }));
    fireEvent.click(screen.getByRole('radio', { name: /bucket/i }));
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'drop-bucket' } });
    fireEvent.change(screen.getByLabelText('Bucket URL'), { target: { value: 'gs://my-gcs-bucket' } });
    // leave Prefix blank
    fireEvent.click(screen.getByRole('button', { name: /add bucket host/i }));

    await waitFor(() => expect(addBucketSpy).toHaveBeenCalledTimes(1));
    const call = addBucketSpy.mock.calls[0][0];
    expect(call.prefix).toBeUndefined();
  });

  it('tunnel mode: form closes after enroll and host list refreshes', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'enrollNode').mockResolvedValue({
      host: TUNNEL_HOST,
      command: 'rupu node --cp-url wss://<your-cp-host>:7878 --token t',
      token: 't',
    });

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // Open, switch to tunnel, fill, submit.
    fireEvent.click(await screen.findByRole('button', { name: 'Add host' }));
    fireEvent.click(screen.getByRole('radio', { name: /tunnel node/i }));
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'my-box' } });
    fireEvent.click(screen.getByRole('button', { name: /enroll node/i }));

    // After submit the form should close (the "Enroll node" submit button gone).
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: /enroll node/i })).not.toBeInTheDocument(),
    );

    // The host list should refresh (getHosts called a second time).
    await waitFor(() => expect(vi.mocked(api.getHosts)).toHaveBeenCalledTimes(2));
  });
});
