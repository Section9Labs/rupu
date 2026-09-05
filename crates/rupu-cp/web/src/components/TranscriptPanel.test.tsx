// @vitest-environment jsdom
/**
 * Tests for TranscriptPanel's `embedded` prop:
 *   - default (embedded omitted): the run-level header chrome renders (agent
 *     name + token footer) alongside the turn body.
 *   - embedded: the header/footer chrome is hidden, but the turn/tool
 *     conversation body still renders.
 *
 * The pure event→view mapping is covered by transcriptView.test.ts; here we only
 * mock `api.getTranscript` (no live SSE) and assert the chrome gating.
 */

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';
import type { TranscriptResponse } from '../lib/transcript';
import TranscriptPanel from './TranscriptPanel';

// `useNavigate` is what `openTranscript` reaches for; spy on it so the
// sub-run URL it builds can be asserted directly.
const { navigateSpy } = vi.hoisted(() => ({ navigateSpy: vi.fn() }));
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateSpy };
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateSpy.mockClear();
});

// A run_start (→ header) + an assistant_message (→ turn body) + a run_complete
// (→ footer). The header surfaces the agent name; the body the assistant text.
const TRANSCRIPT: TranscriptResponse = {
  events: [
    {
      type: 'run_start',
      data: {
        run_id: 'run-1',
        agent: 'reviewer-agent',
        provider: 'anthropic',
        model: 'opus',
        started_at: '2026-06-01T00:00:00Z',
        mode: 'ask',
      },
    },
    { type: 'assistant_message', data: { content: 'Hello from the assistant body.' } },
    {
      type: 'run_complete',
      data: { run_id: 'run-1', status: 'completed', total_tokens: 42, duration_ms: 1000 },
    },
  ],
  summary: null,
};

function renderPanel(embedded: boolean) {
  return render(
    <MemoryRouter>
      <TranscriptPanel path="/t/run-1.jsonl" live={false} embedded={embedded} />
    </MemoryRouter>,
  );
}

describe('TranscriptPanel embedded mode', () => {
  it('renders the header chrome by default (embedded=false)', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue(TRANSCRIPT);
    renderPanel(false);

    // Turn body present…
    expect((await screen.findAllByText('Hello from the assistant body.')).length).toBeGreaterThan(0);
    // …and so is the run-level header (agent name) + footer status chrome.
    expect(screen.getByText('reviewer-agent')).toBeInTheDocument();
    expect(screen.getByText(/completed/)).toBeInTheDocument();
  });

  it('hides the header/footer chrome when embedded, but keeps the turn body', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue(TRANSCRIPT);
    renderPanel(true);

    // Turn body still renders…
    expect((await screen.findAllByText('Hello from the assistant body.')).length).toBeGreaterThan(0);
    // …but the run-level header (agent name) + footer status are gone.
    expect(screen.queryByText('reviewer-agent')).not.toBeInTheDocument();
    expect(screen.queryByText(/completed/)).not.toBeInTheDocument();
  });

  it('shows an unparsed-lines badge when the response reports unparsed > 0', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue({ ...TRANSCRIPT, unparsed: 3 });
    renderPanel(false);

    expect((await screen.findAllByText('reviewer-agent'))[0]).toBeInTheDocument();
    expect(screen.getByText(/3 unparsed lines/)).toBeInTheDocument();
  });

  it('omits the unparsed badge when unparsed is absent or zero', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue(TRANSCRIPT);
    renderPanel(false);

    expect((await screen.findAllByText('reviewer-agent'))[0]).toBeInTheDocument();
    expect(screen.queryByText(/unparsed/)).not.toBeInTheDocument();
  });
});

describe('TranscriptPanel remote reads', () => {
  it('forwards host and run id to the fetch and the stream', async () => {
    const getTranscript = vi.spyOn(api, 'getTranscript').mockResolvedValue({ events: [], summary: null });
    const subscribeTranscript = vi.spyOn(api, 'subscribeTranscript').mockReturnValue(() => {});
    render(
      <MemoryRouter>
        <TranscriptPanel path="/remote/.rupu/transcripts/run_01A.jsonl" live host="host_abc" runId="run_01PARENT" />
      </MemoryRouter>,
    );
    await waitFor(() => expect(getTranscript).toHaveBeenCalled());
    expect(getTranscript).toHaveBeenCalledWith('/remote/.rupu/transcripts/run_01A.jsonl', { host: 'host_abc', run: 'run_01PARENT' });
    expect(subscribeTranscript).toHaveBeenCalledWith(
      '/remote/.rupu/transcripts/run_01A.jsonl',
      expect.any(Function),
      expect.any(Function),
      { host: 'host_abc', run: 'run_01PARENT' },
    );
  });

  it('shows the partial badge when the server could not collect the whole transcript', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue({ events: [], summary: null, partial: true });
    render(
      <MemoryRouter>
        <TranscriptPanel path="/t/run-1.jsonl" live={false} host="host_abc" runId="run_01P" />
      </MemoryRouter>,
    );
    expect(await screen.findByText(/incomplete/i)).toBeInTheDocument();
  });
});

describe('TranscriptPanel sub-run links', () => {
  // A `seed` event with a `source_transcript` renders the "view source
  // transcript" button, which is the only route into `openTranscript`.
  const SEEDED: TranscriptResponse = {
    events: [
      {
        type: 'seed',
        data: {
          message_count: 3,
          source_transcript: '/home/ci/.rupu/runs/run_01P/sub/sub_01X/transcript.jsonl',
        },
      },
    ],
    summary: null,
  };

  it('carries host and run into the sub-run URL', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue(SEEDED);
    render(
      <MemoryRouter>
        <TranscriptPanel path="/t/run-1.jsonl" live={false} host="host_abc" runId="run_01P" />
      </MemoryRouter>,
    );
    fireEvent.click(await screen.findByText('view source transcript'));
    expect(navigateSpy).toHaveBeenCalledWith(
      '/transcript?path=' +
        encodeURIComponent('/home/ci/.rupu/runs/run_01P/sub/sub_01X/transcript.jsonl') +
        '&live=0&host=host_abc&run=run_01P',
    );
  });

  it('omits both when the panel has neither', async () => {
    vi.spyOn(api, 'getTranscript').mockResolvedValue(SEEDED);
    render(
      <MemoryRouter>
        <TranscriptPanel path="/t/run-1.jsonl" live={false} />
      </MemoryRouter>,
    );
    fireEvent.click(await screen.findByText('view source transcript'));
    expect(navigateSpy).toHaveBeenCalledWith(
      '/transcript?path=' +
        encodeURIComponent('/home/ci/.rupu/runs/run_01P/sub/sub_01X/transcript.jsonl') +
        '&live=0',
    );
  });
});
