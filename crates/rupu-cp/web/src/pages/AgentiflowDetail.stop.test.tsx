// @vitest-environment jsdom
// AgentiflowDetail's two stops (operator MCP Plan 3b, UI-2): Stop queues the
// graceful stop and shows a pending state until the run leaves `running`;
// Stop now asks first (the mock's warn card), then kills the run.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { api, ApiError, type AgentiflowDetail as Detail } from '../lib/api';
import { agentiflowsApi } from '../lib/api3b/agentiflows';

// The Flow tab's map has its own tests (components/flowmap); its canvas
// can't mount under jsdom.
vi.mock('../components/flowmap/FlowMap', () => ({
  __esModule: true,
  default: ({ running }: { running: boolean }) => <div data-testid="flow-map" data-running={String(running)} />,
}));

vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: () => <div data-testid="transcript-panel" />,
}));

import AgentiflowDetail from './AgentiflowDetail';

const RUNNING: Detail = {
  record: {
    id: 'af_01RUN',
    name: 'assess-lab',
    codename: 'amber-quay',
    codename_derived: false,
    status: 'running',
    stop_reason: null,
    rounds: 4,
    goals_met: 0,
    goals_total: 1,
    spent_usd: 3.2,
    spent_tokens: 50_000,
    started_at: '2026-10-20T10:00:00Z',
    ended_at: null,
    engagement_profiles: ['web'],
    runner_alive: true,
    trigger: 'agentiflow',
    runner_pid: 4100,
    goals: [{ id: 'g', met: false, current: 0, target: 1 }],
  },
  def: null,
  budget_state: 'ok',
  events: [],
  units: ['u1', 'u2', 'u3'].map((u, i) => ({
    unit_id: u,
    codename: `amber-quay/lynx#${i + 1}`,
    codename_derived: false,
    agent: 'recon',
    participant: `recon#${i + 1}`,
    kind: 'agent' as const,
    status: { state: i < 2 ? ('running' as const) : ('done' as const), success: true },
    pgid: null,
    started_at: '2026-10-20T10:01:00Z',
    transcript_path: null,
  })),
  lead_transcripts: [],
};

const STOPPED: Detail = {
  ...RUNNING,
  record: { ...RUNNING.record, status: 'failed', stop_reason: 'operator_stop:now', ended_at: '2026-10-20T10:05:00Z' },
};

let getAgentiflow: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  getAgentiflow = vi.spyOn(api, 'getAgentiflow').mockResolvedValue(RUNNING);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// A link-free way to move the page to another flow, as the sidebar or a
// Messages link would: the same route element stays mounted.
function GoTo({ to }: { to: string }) {
  const navigate = useNavigate();
  return (
    <button type="button" onClick={() => navigate(to)}>
      go to {to}
    </button>
  );
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/agentiflows/af_01RUN']}>
      <Routes>
        <Route path="/agentiflows/:id" element={<AgentiflowDetail />} />
      </Routes>
      <GoTo to="/agentiflows/af_01OTHER" />
    </MemoryRouter>,
  );
}

describe('AgentiflowDetail stop controls', () => {
  it('offers Stop and Stop now only while the flow runs', async () => {
    getAgentiflow.mockResolvedValue(STOPPED);
    renderPage();
    await screen.findByRole('heading', { name: 'assess-lab' });
    expect(screen.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Stop now' })).not.toBeInTheDocument();
  });

  it('Stop queues the graceful stop and shows the round-boundary pending state', async () => {
    const stop = vi.spyOn(agentiflowsApi, 'stop').mockResolvedValue({ queued: true });
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(stop).toHaveBeenCalledWith('af_01RUN', false));
    expect(await screen.findByText(/stops after this round/i)).toBeInTheDocument();
    // Pending, not done: the button stays disabled while the run still reads running.
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
    // Stop now is still there to escalate.
    expect(screen.getByRole('button', { name: 'Stop now' })).toBeEnabled();
  });

  it('a 409 shows its message inline', async () => {
    vi.spyOn(agentiflowsApi, 'stop').mockRejectedValue(
      new ApiError(409, 'conflict', JSON.stringify({ error: 'agentiflow af_01RUN is not running' })),
    );
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Stop' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('agentiflow af_01RUN is not running');
    expect(screen.queryByText(/stops after this round/i)).not.toBeInTheDocument();
  });

  it('a 501 disables both stops with the reason', async () => {
    vi.spyOn(agentiflowsApi, 'stop').mockRejectedValue(
      new ApiError(501, 'x', JSON.stringify({ error: 'stopping an agentiflow needs `rupu cp serve`' })),
    );
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Stop' }));
    expect(await screen.findByText(/needs `rupu cp serve`/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Stop now' })).toBeDisabled();
  });

  it('Stop now asks first, naming the running units; Keep running backs out', async () => {
    const stop = vi.spyOn(agentiflowsApi, 'stop');
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Stop now' }));
    const card = await screen.findByRole('alertdialog', { name: 'Stop now?' });
    expect(card).toHaveTextContent('2 running units');
    fireEvent.click(screen.getByRole('button', { name: 'Keep running' }));
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(stop).not.toHaveBeenCalled();
  });

  it('confirming Stop now kills the run, then reloads it', async () => {
    const stop = vi
      .spyOn(agentiflowsApi, 'stop')
      .mockResolvedValue({ stopped: true, status: 'failed', stop_reason: 'operator_stop:now' });
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Stop now' }));
    getAgentiflow.mockResolvedValue(STOPPED);
    const card = await screen.findByRole('alertdialog', { name: 'Stop now?' });
    fireEvent.click(card.querySelector('button[data-confirm]') as HTMLButtonElement);
    await waitFor(() => expect(stop).toHaveBeenCalledWith('af_01RUN', true));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Stop now' })).not.toBeInTheDocument());
    expect(screen.getByText('operator_stop:now')).toBeInTheDocument();
  });

  it("a stop requested on one flow does not follow the page to another flow", async () => {
    vi.spyOn(agentiflowsApi, 'stop').mockResolvedValue({ queued: true });
    const other: Detail = { ...RUNNING, record: { ...RUNNING.record, id: 'af_01OTHER', name: 'sweep-lab' } };
    getAgentiflow.mockImplementation(async (id: string) => (id === 'af_01OTHER' ? other : RUNNING));
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Stop' }));
    expect(await screen.findByText(/stops after this round/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'go to /agentiflows/af_01OTHER' }));
    expect(await screen.findByRole('heading', { name: 'sweep-lab' })).toBeInTheDocument();
    expect(screen.queryByText(/stops after this round/i)).not.toBeInTheDocument();
    expect(screen.queryByText('stop requested')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled();
  });

  it('a running record whose coordinator is gone offers only Stop now', async () => {
    getAgentiflow.mockResolvedValue({ ...RUNNING, record: { ...RUNNING.record, runner_alive: false } });
    renderPage();
    expect(await screen.findByRole('button', { name: 'Stop' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Stop now' })).toBeEnabled();
  });
});
