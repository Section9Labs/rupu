// @vitest-environment jsdom
// Continue on a standalone agent run's transcript page (operator MCP Plan 3b,
// UI-1, mock §2): a run that isn't running gets Continue, which opens a
// composer under the transcript with a "Model & provider" disclosure showing
// the original values. The CP's continue takes no prompt (Plan 3a, package
// B): it restores every flag of the original launch, and only a different
// model or provider can be given.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError, type AgentRunRow } from '../lib/api';
import { runsApi } from '../lib/api3b/runs';

vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: ({ path }: { path: string }) => <div data-testid="transcript-panel">transcript:{path}</div>,
}));

import RunTranscript from './RunTranscript';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const PATH = '/home/u/.rupu/transcripts/run_01JD2K.jsonl';

function agentRow(over: Partial<AgentRunRow> = {}): AgentRunRow {
  return {
    codename: 'cobalt-harbor/lynx',
    codename_derived: false,
    run_id: 'run_01JD2K',
    source: 'standalone',
    agent: 'code-reviewer',
    provider: 'anthropic',
    model: 'claude-opus-5-5',
    status: 'completed',
    transcript_path: PATH,
    turns: 14,
    usage: undefined as never,
    ...over,
  };
}

function stub(row: AgentRunRow | null) {
  vi.spyOn(api, 'getAgentRuns').mockResolvedValue(row ? [row] : []);
  vi.spyOn(api, 'getRunChildren').mockResolvedValue([]);
  vi.spyOn(api, 'getModelCatalog').mockResolvedValue([
    { provider: 'anthropic', fetched_at: null, stale: false, models: [{ id: 'claude-opus-5-5', input_tokens: null, output_tokens: null, source: 'live' }, { id: 'claude-sonnet-5-5', input_tokens: null, output_tokens: null, source: 'live' }] },
    { provider: 'openai', fetched_at: null, stale: false, models: [] },
  ]);
}

function renderAt(search = `?path=${encodeURIComponent(PATH)}&live=0`) {
  return render(
    <MemoryRouter initialEntries={[`/transcript${search}`]}>
      <RunTranscript />
    </MemoryRouter>,
  );
}

describe('RunTranscript continue', () => {
  it('heads the page with the run and asks for it by id', async () => {
    stub(agentRow());
    renderAt();
    expect(await screen.findByRole('heading', { name: /code-reviewer/ })).toBeInTheDocument();
    expect(api.getAgentRuns).toHaveBeenCalledWith(expect.objectContaining({ q: 'id:run_01JD2K', limit: 5 }));
  });

  it('opens the composer, shows the original model and provider, and continues', async () => {
    stub(agentRow());
    const cont = vi
      .spyOn(runsApi, 'continueAgentRun')
      .mockResolvedValue({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    expect(within(composer).getByText(/same as the original: anthropic · claude-opus-5-5/)).toBeInTheDocument();

    fireEvent.click(within(composer).getByRole('button', { name: 'Continue run' }));
    await waitFor(() => expect(cont).toHaveBeenCalledWith('run_01JD2K', { model: undefined, provider: undefined }));
    const link = await screen.findByRole('link', { name: /Continued as run_01JD4Q/ });
    expect(link).toHaveAttribute(
      'href',
      `/transcript?path=${encodeURIComponent('/home/u/.rupu/transcripts/run_01JD4Q.jsonl')}&live=1`,
    );
  });

  it('⌘↵ continues from inside the composer', async () => {
    stub(agentRow());
    const cont = vi
      .spyOn(runsApi, 'continueAgentRun')
      .mockResolvedValue({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    fireEvent.keyDown(composer, { key: 'Enter', metaKey: true });
    await waitFor(() => expect(cont).toHaveBeenCalled());
  });

  it('sends a different model chosen under the disclosure', async () => {
    stub(agentRow());
    const cont = vi
      .spyOn(runsApi, 'continueAgentRun')
      .mockResolvedValue({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    fireEvent.click(within(composer).getByRole('button', { name: /Model & provider/ }));
    fireEvent.change(within(composer).getByLabelText('Model'), { target: { value: 'claude-sonnet-5-5' } });
    fireEvent.click(within(composer).getByRole('button', { name: 'Continue run' }));
    await waitFor(() =>
      expect(cont).toHaveBeenCalledWith('run_01JD2K', { model: 'claude-sonnet-5-5', provider: undefined }),
    );
  });

  it('a failed run continues only on a different model or provider', async () => {
    stub(agentRow({ status: 'failed' }));
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    expect(within(composer).getByText(/A failed run continues only on a different model or provider/)).toBeInTheDocument();
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeDisabled();
    await within(composer).findByRole('option', { name: 'openai' });
    fireEvent.change(within(composer).getByLabelText('Provider'), { target: { value: 'openai' } });
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeEnabled();
  });

  it('shows a 409 inline', async () => {
    stub(agentRow());
    vi.spyOn(runsApi, 'continueAgentRun').mockRejectedValue(
      new ApiError(409, 'conflict', JSON.stringify({ error: 'run run_01JD2K is still running' })),
    );
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    fireEvent.click(screen.getByRole('button', { name: 'Continue run' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('run run_01JD2K is still running');
  });

  it('offers no Continue while the run is running', async () => {
    stub(agentRow({ status: 'running' }));
    renderAt();
    await screen.findByRole('heading', { name: /code-reviewer/ });
    expect(screen.queryByRole('button', { name: 'Continue' })).not.toBeInTheDocument();
  });

  it('offers no Continue for a remote run', async () => {
    stub(agentRow());
    renderAt(`?path=${encodeURIComponent(PATH)}&live=0&host=h1&run=run_01JD2K`);
    await screen.findByTestId('transcript-panel');
    expect(screen.queryByRole('button', { name: 'Continue' })).not.toBeInTheDocument();
  });
});
