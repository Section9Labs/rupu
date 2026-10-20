// @vitest-environment jsdom
// Continue on a standalone agent run's transcript page (operator MCP Plan 3b,
// UI-1, mock §2): every ended run — completed, failed, cancelled, or crashed
// before recording an end — gets Continue, which opens a composer under the
// transcript with an optional prompt and a "Model & provider" disclosure
// showing the original values. The continuation restores every flag of the
// original launch. A completed run needs a prompt (without one a continue
// only reprints its answer); a failed one needs a prompt or another model or
// provider.

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
    status: 'cancelled',
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
    { provider: 'openai', fetched_at: null, stale: false, models: [{ id: 'gpt-6', input_tokens: null, output_tokens: null, source: 'live' }, { id: 'gpt-6-mini', input_tokens: null, output_tokens: null, source: 'live' }] },
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
    await waitFor(() =>
      expect(cont).toHaveBeenCalledWith('run_01JD2K', { prompt: undefined, model: undefined, provider: undefined }),
    );
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

  it('a provider chosen alone brings its first model, never the old one', async () => {
    stub(agentRow());
    const cont = vi
      .spyOn(runsApi, 'continueAgentRun')
      .mockResolvedValue({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    fireEvent.click(within(composer).getByRole('button', { name: /Model & provider/ }));
    await within(composer).findByRole('option', { name: 'openai' });
    fireEvent.change(within(composer).getByLabelText('Provider'), { target: { value: 'openai' } });
    expect((within(composer).getByLabelText('Model') as HTMLInputElement).value).toBe('gpt-6');
    fireEvent.click(within(composer).getByRole('button', { name: 'Continue run' }));
    await waitFor(() =>
      expect(cont).toHaveBeenCalledWith('run_01JD2K', { prompt: undefined, model: 'gpt-6', provider: 'openai' }),
    );
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
      expect(cont).toHaveBeenCalledWith('run_01JD2K', {
        prompt: undefined,
        model: 'claude-sonnet-5-5',
        provider: undefined,
      }),
    );
  });

  it('a failed run continues on a different model or provider', async () => {
    stub(agentRow({ status: 'failed' }));
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    expect(
      within(composer).getByText(/A failed run continues with a prompt, or on a different model or provider/),
    ).toBeInTheDocument();
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeDisabled();
    await within(composer).findByRole('option', { name: 'openai' });
    fireEvent.change(within(composer).getByLabelText('Provider'), { target: { value: 'openai' } });
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeEnabled();
  });

  it('a failed run continues on the same model when given a prompt', async () => {
    stub(agentRow({ status: 'failed' }));
    const cont = vi
      .spyOn(runsApi, 'continueAgentRun')
      .mockResolvedValue({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    const prompt = within(composer).getByLabelText(/Prompt/);
    // Without a model change the prompt is needed, so it isn't "optional".
    const label = composer.querySelector(`label[for="${prompt.id}"]`);
    expect(label).toHaveTextContent('Prompt — or change the model below');
    expect(label).not.toHaveTextContent(/optional/i);
    expect(prompt).not.toHaveAttribute('placeholder', expect.stringMatching(/optional/i));
    fireEvent.change(prompt, { target: { value: '   ' } });
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeDisabled();
    fireEvent.change(prompt, { target: { value: 'It is authorised: go ahead.' } });
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeEnabled();
    fireEvent.click(within(composer).getByRole('button', { name: 'Continue run' }));
    await waitFor(() =>
      expect(cont).toHaveBeenCalledWith('run_01JD2K', {
        prompt: 'It is authorised: go ahead.',
        model: undefined,
        provider: undefined,
      }),
    );
  });

  it('an interrupted run takes an optional prompt, sent as typed', async () => {
    stub(agentRow({ status: 'cancelled' }));
    const cont = vi
      .spyOn(runsApi, 'continueAgentRun')
      .mockResolvedValue({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    const prompt = within(composer).getByLabelText(/Prompt/);
    expect(prompt).toHaveAttribute('placeholder', expect.stringMatching(/optional/i));
    expect(composer.querySelector(`label[for="${prompt.id}"]`)).toHaveTextContent('Prompt (optional)');
    fireEvent.change(prompt, { target: { value: '/pdf then skip the appendix' } });
    fireEvent.keyDown(prompt, { key: 'Enter', metaKey: true });
    await waitFor(() =>
      expect(cont).toHaveBeenCalledWith('run_01JD2K', {
        prompt: '/pdf then skip the appendix',
        model: undefined,
        provider: undefined,
      }),
    );
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

  it('a completed run continues only with a prompt (without one it would only reprint its answer)', async () => {
    stub(agentRow({ status: 'completed' }));
    const cont = vi
      .spyOn(runsApi, 'continueAgentRun')
      .mockResolvedValue({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    expect(within(composer).getByText(/A finished run needs a prompt/)).toBeInTheDocument();
    const prompt = within(composer).getByLabelText(/Prompt/);
    expect(prompt).toBeRequired();
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeDisabled();
    // ⌘↵ with no prompt sends nothing.
    fireEvent.keyDown(prompt, { key: 'Enter', metaKey: true });
    // A different model alone is not enough: the run finished.
    fireEvent.click(within(composer).getByRole('button', { name: /Model & provider/ }));
    await within(composer).findByRole('option', { name: 'openai' });
    fireEvent.change(within(composer).getByLabelText('Provider'), { target: { value: 'openai' } });
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeDisabled();
    expect(cont).not.toHaveBeenCalled();

    fireEvent.change(prompt, { target: { value: 'Now write the fix.' } });
    expect(within(composer).getByRole('button', { name: 'Continue run' })).toBeEnabled();
    fireEvent.keyDown(prompt, { key: 'Enter', ctrlKey: true });
    await waitFor(() =>
      expect(cont).toHaveBeenCalledWith('run_01JD2K', {
        prompt: 'Now write the fix.',
        model: 'gpt-6',
        provider: 'openai',
      }),
    );
    expect(await screen.findByRole('link', { name: /Continued as run_01JD4Q/ })).toBeInTheDocument();
  });

  it('shows the pending state while the continue is sent', async () => {
    stub(agentRow({ status: 'completed' }));
    let resolve: (v: { run_id: string; continues: string; host_id: string }) => void = () => {};
    vi.spyOn(runsApi, 'continueAgentRun').mockReturnValue(
      new Promise((r) => {
        resolve = r;
      }),
    );
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    const composer = screen.getByRole('region', { name: 'Continue this run' });
    fireEvent.change(within(composer).getByLabelText(/Prompt/), { target: { value: 'go on' } });
    fireEvent.click(within(composer).getByRole('button', { name: 'Continue run' }));
    expect(await within(composer).findByRole('button', { name: 'Continuing…' })).toBeDisabled();
    resolve({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    expect(await screen.findByRole('link', { name: /Continued as run_01JD4Q/ })).toBeInTheDocument();
  });

  it('offers Continue on a run that crashed before recording how it ended (no status)', async () => {
    stub(agentRow({ status: null }));
    const cont = vi
      .spyOn(runsApi, 'continueAgentRun')
      .mockResolvedValue({ run_id: 'run_01JD4Q', continues: 'run_01JD2K', host_id: 'local' });
    renderAt();
    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    fireEvent.click(screen.getByRole('button', { name: 'Continue run' }));
    await waitFor(() =>
      expect(cont).toHaveBeenCalledWith('run_01JD2K', { prompt: undefined, model: undefined, provider: undefined }),
    );
  });

  it('offers no Continue on a session turn', async () => {
    stub(agentRow({ source: 'session', status: null, session_id: 'ses_1' }));
    renderAt();
    await screen.findByRole('heading', { name: /code-reviewer/ });
    expect(screen.queryByRole('button', { name: 'Continue' })).not.toBeInTheDocument();
  });

  it('offers no Continue while the run is live', async () => {
    for (const status of ['running', 'pending', 'paused', 'awaiting_approval']) {
      stub(agentRow({ status }));
      const { unmount } = renderAt();
      await screen.findByRole('heading', { name: /code-reviewer/ });
      expect(screen.queryByRole('button', { name: 'Continue' }), status).not.toBeInTheDocument();
      unmount();
      vi.restoreAllMocks();
    }
  });

  it('offers no Continue for a remote run', async () => {
    stub(agentRow());
    renderAt(`?path=${encodeURIComponent(PATH)}&live=0&host=h1&run=run_01JD2K`);
    await screen.findByTestId('transcript-panel');
    expect(screen.queryByRole('button', { name: 'Continue' })).not.toBeInTheDocument();
  });
});
