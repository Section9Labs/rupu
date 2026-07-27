// @vitest-environment jsdom
// Agents list — row actions (Run / Session / Delete). The table uses
// whole-row navigation (rowHref → /agents/:name), so the action column must
// be `interactive: true` and every button must call BOTH preventDefault()
// and stopPropagation() on its click — stopPropagation alone does not stop
// the enclosing rowHref <a> from following its native navigation (see
// pages/runs/WorkflowRuns.preventDefault.test.tsx, whose technique this
// mirrors: `fireEvent.click` returns `false` iff `preventDefault()` fired
// somewhere in the dispatch path).
//
// AgentLauncherSheet is mocked to a stub — its own behavior is covered by
// AgentLauncherSheet.test.tsx; here we only assert Agents.tsx opens it with
// the right agent and doesn't navigate the row.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { api, type AgentSummary } from '../lib/api';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

vi.mock('../components/AgentLauncherSheet', () => ({
  __esModule: true,
  default: ({ agent }: { agent: string }) => <div data-testid="agent-launcher-sheet">{agent}</div>,
}));

import Agents from './Agents';

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

const USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: null,
  priced: true,
  runs: 0,
};

const ROWS: AgentSummary[] = [
  { name: 'reviewer', scope: 'global', scope_kind: 'global', usage: USAGE, run_count: 2 },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('Agents — row actions', () => {
  it('renders Run, Session, and Delete buttons per row, in a plain (non-link-wrapped) action cell', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    const runBtn = screen.getByRole('button', { name: 'Run reviewer' });
    const sessionBtn = screen.getByRole('button', { name: 'Start session with reviewer' });
    const deleteBtn = screen.getByRole('button', { name: 'Delete reviewer' });
    expect(runBtn).toBeInTheDocument();
    expect(sessionBtn).toBeInTheDocument();
    expect(deleteBtn).toBeInTheDocument();
    // The action column is `interactive` — SortableTable renders it as a
    // plain, unwrapped cell, never nested inside the row's own <a>.
    expect(runBtn.closest('a')).toBeNull();
  });

  it('clicking Run opens the AgentLauncherSheet for this agent, without navigating the row', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    const notCanceled = fireEvent.click(screen.getByRole('button', { name: 'Run reviewer' }));

    expect(notCanceled).toBe(false);
    expect(screen.getByTestId('agent-launcher-sheet')).toHaveTextContent('reviewer');
    expect(screen.getByTestId('loc')).toHaveTextContent('/agents');
    expect(navigateMock).not.toHaveBeenCalled();
  });

  it('clicking Session starts a session via the API and navigates to it, without navigating the row first', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    const sessionSpy = vi.spyOn(api, 'startSession').mockResolvedValue({ session_id: 'sess_1' });

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    const notCanceled = fireEvent.click(
      screen.getByRole('button', { name: 'Start session with reviewer' }),
    );
    expect(notCanceled).toBe(false);

    await waitFor(() => expect(sessionSpy).toHaveBeenCalledWith('reviewer'));
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/sessions/sess_1'));
  });

  it('clicking Delete without confirming does not call the API', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    const deleteSpy = vi.spyOn(api, 'deleteAgent').mockResolvedValue({ deleted: true, scope: 'global', scope_kind: 'global' });
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete reviewer' }));

    expect(window.confirm).toHaveBeenCalled();
    expect(deleteSpy).not.toHaveBeenCalled();
  });

  it('clicking Delete and confirming deletes the agent definition and refreshes the list, without navigating the row', async () => {
    const getAgentsSpy = vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    const deleteSpy = vi.spyOn(api, 'deleteAgent').mockResolvedValue({ deleted: true, scope: 'global', scope_kind: 'global' });
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
        <LocationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    const notCanceled = fireEvent.click(screen.getByRole('button', { name: 'Delete reviewer' }));
    expect(notCanceled).toBe(false);

    await waitFor(() =>
      expect(deleteSpy).toHaveBeenCalledWith('reviewer', { scope_kind: 'global' }),
    );
    await waitFor(() => expect(getAgentsSpy).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId('loc')).toHaveTextContent('/agents');
  });

  it('surfaces a delete failure via the page error mechanism', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue(ROWS);
    vi.spyOn(api, 'deleteAgent').mockRejectedValue(new Error('boom'));
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    render(
      <MemoryRouter initialEntries={['/agents']}>
        <Agents />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('reviewer')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Delete reviewer' }));

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('boom'));
  });
});
