// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from '../../lib/api';
import type { TranscriptEvent } from '../../lib/transcript';
import TranscriptPanel from '../TranscriptPanel';
import { LYNX_TURN } from '../messages/fixtures';
import { MessageLinksContext } from './messageLinks';
import Turn from './Turn';
import { buildTranscriptView } from './transcriptView';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const EVENTS = LYNX_TURN as TranscriptEvent[];

describe('messages in the transcript view', () => {
  const view = buildTranscriptView(EVENTS);
  const blocks = view.turns[0].blocks;

  it('folds each messaging tool call into its sent bubble, in its place', () => {
    expect(blocks.map((b) => (b.kind === 'message' ? `${b.message.direction}:${b.message.id}` : b.kind === 'tool' ? `tool:${b.view.tool}` : b.kind))).toEqual([
      'injected',
      'received:msg_01JQ8MA003',
      'assistant',
      'tool:bash',
      'sent:msg_01JQ8MA004',
      'sent:msg_01JQ8MA006',
    ]);
    const sent = blocks[4];
    expect(sent.kind === 'message' && sent.message.raw?.tool).toBe('board.post');
    expect(view.turns[0].summary.toolCount).toBe(3);
    expect(view.self).toBe('lynx#1');
  });

  it('keeps context injections as chips and drops the ones its bubbles show', () => {
    const chips = blocks.filter((b) => b.kind === 'injected');
    expect(chips).toEqual([expect.objectContaining({ kind: 'injected', source: 'roster', cadence: 'every_turn' })]);
  });
});

describe('Turn renders messages', () => {
  const view = buildTranscriptView(EVENTS);

  it('shows received and sent bubbles, a raw toggle and a collapsed chip', () => {
    render(<Turn turn={view.turns[0]} turnNumber={1} showMarker self={view.self} />);
    const bubbles = screen.getAllByTestId('message-bubble');
    expect(bubbles.map((b) => b.dataset.direction)).toEqual(['received', 'sent', 'sent']);
    expect(within(bubbles[0]).getByText('→ you')).toBeInTheDocument();
    expect(within(bubbles[2]).getByTestId('undelivered')).toHaveTextContent('no live participant for role:crawler');
    expect(within(bubbles[1]).getByText('delivered to heron, otter')).toBeInTheDocument();

    expect(document.querySelector('[data-call-id="toolu_02"]')).toBeNull();
    fireEvent.click(within(bubbles[1]).getByRole('button', { name: 'raw' }));
    expect(document.querySelector('[data-call-id="toolu_02"]')).not.toBeNull();

    const chip = screen.getByTestId('injected-chip');
    expect(chip).toHaveTextContent('context injected · roster');
    expect(within(chip).queryByText(/Participants in this run/)).toBeNull();
    fireEvent.click(within(chip).getByRole('button'));
    expect(within(chip).getByText(/Participants in this run/)).toBeInTheDocument();
  });

  it('links a bubble to the Messages tab and to the sender', () => {
    const openInFeed = vi.fn();
    const openParticipant = vi.fn();
    render(
      <MessageLinksContext.Provider value={{ openInFeed, openParticipant }}>
        <Turn turn={view.turns[0]} turnNumber={1} showMarker self={view.self} />
      </MessageLinksContext.Provider>,
    );
    const received = screen.getAllByTestId('message-bubble')[0];
    fireEvent.click(within(received).getByRole('button', { name: /in Messages/ }));
    expect(openInFeed).toHaveBeenCalledWith('msg_01JQ8MA003');
    fireEvent.click(within(received).getByRole('button', { name: /sender's transcript/ }));
    expect(openParticipant).toHaveBeenCalledWith('heron', 'msg_01JQ8MA003');
  });
});

describe('#msg- anchors in a transcript', () => {
  it('open the turn holding the message, ring the bubble and scroll to it', async () => {
    const scrolled: Element[] = [];
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this);
    };
    // A later turn makes the message's turn a collapsed one.
    const events = [...EVENTS, { type: 'turn_start', data: {} }, { type: 'assistant_message', data: { content: 'done' } }] as TranscriptEvent[];
    vi.spyOn(api, 'getTranscript').mockResolvedValue({ events } as Awaited<ReturnType<typeof api.getTranscript>>);
    render(
      <MemoryRouter initialEntries={['/transcript?path=x#msg-msg_01JQ8MA004']}>
        <TranscriptPanel path="/t/lynx.jsonl" live={false} />
      </MemoryRouter>,
    );
    await waitFor(() => expect(document.getElementById('msg-msg_01JQ8MA004')).not.toBeNull());
    const bubble = document.getElementById('msg-msg_01JQ8MA004')!;
    expect(bubble.querySelector('.ring-2')).not.toBeNull();
    // The scroll is an effect: it runs after the commit that mounted it.
    await waitFor(() => expect(scrolled).toContain(bubble));
  });
});
