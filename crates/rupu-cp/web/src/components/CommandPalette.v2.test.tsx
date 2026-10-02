// @vitest-environment jsdom
// CommandPalette v2 additions: the external `openCommandPalette()` open
// event, and the `shell="v2"` nav-page swap + moved-list-route rewrite.
// Mocks `../lib/api` and `useNavigate` the same way the rest of the CP web
// test suite does (see LauncherSheet.test.tsx) — no existing CommandPalette
// test file exists yet to imitate directly.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api } from '../lib/api';
import type { FindingOut, RunListRow } from '../lib/api';

// Stub useNavigate — keep the rest of react-router-dom intact.
const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

import CommandPalette, { openCommandPalette } from './CommandPalette';

function mockEmptyApi() {
  vi.spyOn(api, 'getRuns').mockResolvedValue([]);
  vi.spyOn(api, 'getAgents').mockResolvedValue([]);
  vi.spyOn(api, 'getWorkflows').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([]);
  vi.spyOn(api, 'getSessions').mockResolvedValue([]);
  vi.spyOn(api, 'getProjects').mockResolvedValue([]);
  vi.spyOn(api, 'getCoverage').mockResolvedValue([]);
  vi.spyOn(api, 'getFindings').mockResolvedValue({
    findings: [],
    summary: { total: 0, by_severity: {} } as never,
  });
  vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);
  vi.spyOn(api, 'getWorkers').mockResolvedValue([]);
}

// A minimal run row for the per-host palette tests. `usage` is never read by
// the palette mappers, so a cast keeps the fixture small.
function runRow(over: Partial<RunListRow> & { id: string; workflow_name: string }): RunListRow {
  return {
    codename: 'cobalt-harbor/heron',
    codename_derived: false,
    status: 'completed',
    started_at: '2026-10-01T00:00:00Z',
    trigger: 'manual',
    turns: 1,
    usage: {} as never,
    ...over,
  };
}

function stubHosts() {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([
    { id: 'local', name: 'Local', transport_kind: 'local' },
    { id: 'host_prod', name: 'prod', transport_kind: 'ssh' },
  ]);
}

async function openPalette() {
  render(<CommandPalette />);
  openCommandPalette();
  return screen.findByPlaceholderText('Search runs, agents, workflows, sessions…');
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('CommandPalette v2', () => {
  it('openCommandPalette() opens the dialog', async () => {
    mockEmptyApi();
    render(<CommandPalette />);

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    openCommandPalette();

    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Search runs, agents, workflows, sessions…')).toBeInTheDocument();
  });

  it('shell="v2" surfaces Overview and not Dashboard in page results', async () => {
    mockEmptyApi();
    render(<CommandPalette shell="v2" />);

    openCommandPalette();
    const input = await screen.findByPlaceholderText('Search runs, agents, workflows, sessions…');

    // Query result titles are re-highlighted char-by-char (each matched
    // letter gets its own <mark>), so a plain getByText won't find the
    // concatenated string — match on the option's accessible name instead,
    // which aggregates the whole subtree's text.
    fireEvent.change(input, { target: { value: 'overview' } });
    await waitFor(() =>
      expect(screen.getByRole('option', { name: /Overview/i })).toBeInTheDocument(),
    );

    fireEvent.change(input, { target: { value: 'dashboard' } });
    await waitFor(() =>
      expect(screen.queryByRole('option', { name: /Dashboard/i })).not.toBeInTheDocument(),
    );
  });

  it('shell="v2" navigates a finding entity result to its report page', async () => {
    mockEmptyApi();
    const finding: FindingOut = {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
      id: 'f-1',
      ws_id: 'ws-1',
      project: 'demo',
      target_id: 't-1',
      file_path: 'src/main.rs',
      line_range: null,
      scope: null,
      summary: 'SQL injection in query builder',
      severity: 'high',
      concern_id: null,
      evidence: {} as never,
      declared_by: null,
      declared_at: '2026-08-01T00:00:00Z',
    };
    vi.spyOn(api, 'getFindings').mockResolvedValue({
      findings: [finding],
      summary: { total: 1, by_severity: {} } as never,
    });

    render(<CommandPalette shell="v2" />);
    openCommandPalette();
    const input = await screen.findByPlaceholderText('Search runs, agents, workflows, sessions…');

    fireEvent.change(input, { target: { value: 'SQL injection' } });
    // The title is highlighted char-by-char (each matched letter in its own
    // <mark>), so match on the row's full textContent rather than a plain
    // getByText, which only looks at each node's own direct text children.
    const titleNode = await screen.findByText(
      (_, node) => node?.textContent === 'SQL injection in query builder',
    );
    const result = titleNode.closest('[role="option"]');
    expect(result).toBeTruthy();
    fireEvent.click(result as HTMLElement);

    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/findings/f-1'));
  });

  it('lists local runs without waiting on a hung remote host', async () => {
    mockEmptyApi();
    stubHosts();
    vi.spyOn(api, 'getRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([runRow({ id: 'run_local_1', workflow_name: 'local-wf', host_id: 'local' })])
        : new Promise(() => {}),
    );
    await openPalette();
    await waitFor(() => expect(screen.getByText('local-wf')).toBeInTheDocument());
  });

  it('appends a remote host\'s runs when it answers and opens them with ?host=', async () => {
    mockEmptyApi();
    stubHosts();
    let answerRemote: (rows: RunListRow[]) => void = () => {};
    vi.spyOn(api, 'getRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([runRow({ id: 'run_local_1', workflow_name: 'local-wf', host_id: 'local' })])
        : new Promise<RunListRow[]>((resolve) => { answerRemote = resolve; }),
    );
    await openPalette();
    await waitFor(() => expect(screen.getByText('local-wf')).toBeInTheDocument());
    expect(screen.queryByText('remote-wf')).not.toBeInTheDocument();

    answerRemote([runRow({ id: 'run_remote_1', workflow_name: 'remote-wf', host_id: 'host_prod' })]);
    const remote = await screen.findByText('remote-wf');
    // Both sources survive: the remote append must not displace the local
    // batch that `Promise.all` committed first.
    expect(screen.getByText('local-wf')).toBeInTheDocument();

    fireEvent.click(remote.closest('[role="option"]') as HTMLElement);
    await waitFor(() =>
      expect(navigateMock).toHaveBeenCalledWith('/runs/run_remote_1?host=host_prod'),
    );
  });

  it('keeps a remote host\'s runs when they land before the local batch commits', async () => {
    mockEmptyApi();
    stubHosts();
    let answerLocal: (rows: RunListRow[]) => void = () => {};
    vi.spyOn(api, 'getRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? new Promise<RunListRow[]>((resolve) => { answerLocal = resolve; })
        : Promise.resolve([runRow({ id: 'run_remote_1', workflow_name: 'remote-wf', host_id: 'host_prod' })]),
    );
    await openPalette();
    await screen.findByText('remote-wf');

    answerLocal([runRow({ id: 'run_local_1', workflow_name: 'local-wf', host_id: 'local' })]);
    await screen.findByText('local-wf');
    // The Promise.all commit (setItems) must not have overwritten the remote.
    expect(screen.getByText('remote-wf')).toBeInTheDocument();
  });

  it('falls back to local-only runs when the host list cannot be read', async () => {
    mockEmptyApi();
    vi.spyOn(api, 'getRegisteredHosts').mockRejectedValue(new Error('boom'));
    const getRuns = vi.spyOn(api, 'getRuns').mockResolvedValue([
      runRow({ id: 'run_local_1', workflow_name: 'local-wf', host_id: 'local' }),
    ]);
    await openPalette();
    await waitFor(() => expect(screen.getByText('local-wf')).toBeInTheDocument());
    expect(getRuns).toHaveBeenCalledTimes(1);
    expect(getRuns).toHaveBeenCalledWith(expect.objectContaining({ host: 'local', limit: 200 }));
  });

  it('aborts every per-host run request when the palette closes', async () => {
    mockEmptyApi();
    stubHosts();
    const signals: Record<string, AbortSignal | undefined> = {};
    vi.spyOn(api, 'getRuns').mockImplementation((p) => {
      signals[p?.host ?? '?'] = p?.signal;
      // Remote hangs until aborted, then rejects like a real fetch would.
      return p?.host === 'local'
        ? Promise.resolve([runRow({ id: 'run_local_1', workflow_name: 'local-wf', host_id: 'local' })])
        : new Promise<RunListRow[]>((_, reject) => {
            p?.signal?.addEventListener('abort', () =>
              reject(new DOMException('aborted', 'AbortError')),
            );
          });
    });
    await openPalette();
    await waitFor(() => expect(screen.getByText('local-wf')).toBeInTheDocument());
    await waitFor(() => expect(signals.host_prod).toBeDefined());
    expect(signals.local).toBeDefined();
    expect(signals.host_prod?.aborted).toBe(false);
    expect(signals.local?.aborted).toBe(false);

    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());

    expect(signals.host_prod?.aborted).toBe(true);
    expect(signals.local?.aborted).toBe(true);
    // The aborted remote's rejection is swallowed: nothing is rendered for
    // it, and re-opening works from a clean slate with a fresh signal.
    openCommandPalette();
    await screen.findByRole('dialog');
    await waitFor(() => expect(screen.getByText('local-wf')).toBeInTheDocument());
    expect(screen.queryByText(/error|failed/i)).not.toBeInTheDocument();
    expect(signals.host_prod?.aborted).toBe(false);
  });
});
