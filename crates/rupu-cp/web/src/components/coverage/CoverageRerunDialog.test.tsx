// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CoverageRerunButton } from './CoverageRerunDialog';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const jsonResponse = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

const RUNS = [
  { run_id: 'run_01JB2AGENT', started_at: '2026-10-18T09:00:00Z', model: 'claude-sonnet-5-5', surface: 'agent', cells_asserted: 12, findings: 2, files_touched: 9 },
  { run_id: 'run_01JB1FLOW', started_at: '2026-10-17T09:00:00Z', model: 'claude-sonnet-5-5', surface: 'workflow', cells_asserted: 4, findings: 0, files_touched: 3 },
];

function server(rerun: () => Response) {
  return vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) =>
    String(input).includes('/rerun') ? rerun() : jsonResponse(RUNS),
  );
}

function mount() {
  render(
    <MemoryRouter>
      <CoverageRerunButton target="repo/shop-web" wsId="ws_1" />
    </MemoryRouter>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Rerun…' }));
}

describe('CoverageRerunButton', () => {
  it('lists the recorded runs, only agent runs pickable', async () => {
    const fetchMock = server(() => jsonResponse({}));
    mount();

    const agent = await screen.findByRole('radio', { name: /run_01JB2AGENT/ });
    expect(fetchMock.mock.calls[0][0]).toBe('/api/coverage/repo%2Fshop-web/runs?ws_id=ws_1');
    expect(agent).toBeEnabled();
    expect(screen.getByRole('radio', { name: /run_01JB1FLOW/ })).toBeDisabled();
    expect(screen.getByText(/only agent runs can be rerun/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Rerun…' , hidden: false })).toBeInTheDocument();
  });

  it('confirms before it starts, then links to the new run', async () => {
    const fetchMock = server(() =>
      jsonResponse({ run_id: 'run_01JB3NEW', host_id: 'local', agent: 'auth-reviewer', target_id: 'repo/shop-web', ws_id: 'ws_1', replays: 'run_01JB2AGENT' }, 202),
    );
    mount();
    fireEvent.click(await screen.findByRole('radio', { name: /run_01JB2AGENT/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Rerun this run' }));

    expect(screen.getByText(/starts the run's agent again/i)).toBeInTheDocument();
    expect(fetchMock.mock.calls.some(([u]) => String(u).includes('/rerun'))).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: 'Start rerun' }));

    const link = await screen.findByRole('link', { name: /run_01JB3NEW/ });
    expect(link).toHaveAttribute('href', '/activity?tab=agents&q=id%3Arun_01JB3NEW');
    expect(screen.getByText(/rerun requested/i)).toBeInTheDocument();
    const call = fetchMock.mock.calls.find(([u]) => String(u).includes('/rerun'))!;
    expect(call[0]).toBe('/api/coverage/repo%2Fshop-web/rerun?ws_id=ws_1');
    expect(JSON.parse((call[1] as RequestInit).body as string)).toEqual({ run: 'run_01JB2AGENT' });
  });

  it('shows a refusal under the action', async () => {
    server(() => jsonResponse({ error: 'run_01JB2AGENT continued another run; rerun that one' }, 409));
    mount();
    fireEvent.click(await screen.findByRole('radio', { name: /run_01JB2AGENT/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Rerun this run' }));
    fireEvent.click(screen.getByRole('button', { name: 'Start rerun' }));

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('continued another run'));
    expect(screen.getByRole('button', { name: 'Start rerun' })).toBeEnabled();
  });
});
