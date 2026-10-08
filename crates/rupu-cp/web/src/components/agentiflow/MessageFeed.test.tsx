// @vitest-environment jsdom
// MessageFeed renders the agentiflow board as a group chat: a participants bar,
// a pinned directives panel that folds retractions, and one compact row per
// post with @mention-style treatment — a directed message (`addressed_to`) is
// highlighted and shows `@recipient`, a broadcast is plain, and participant
// tokens the fleet names in a body are highlighted inline.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { api, type AgentiflowMessages } from '../../lib/api';
import MessageFeed from './MessageFeed';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const MSGS: AgentiflowMessages = {
  posts: [
    { author: 'recon#1', kind: 'question', ts: '2026-10-07T22:17:00Z', body: '[elk / fawn-butte] round 1 blocked, no scope located' },
    { author: 'service-analyst#3', addressed_to: 'lead', kind: 'note', ts: '2026-10-07T22:19:00Z', body: 'capability blocker from ibis: no shell' },
  ],
  directives: [
    { author: 'lead', id: 'D1', ts: '2026-10-07T22:16:00Z', body: 'scope discipline: stay strictly in scope' } as AgentiflowMessages['directives'][number],
    { retract: 'D1' } as unknown as AgentiflowMessages['directives'][number],
  ],
};

async function renderFeed() {
  vi.spyOn(api, 'getAgentiflowMessages').mockResolvedValue(MSGS);
  render(<MessageFeed id="af_X" />);
  // wait for the async load
  await screen.findByText('2 messages');
}

describe('MessageFeed', () => {
  it('renders a directed message as an @mention and highlights it', async () => {
    await renderFeed();
    // The directed post shows the recipient as an @mention pill.
    expect(screen.getByText('@lead')).toBeInTheDocument();
    // …and the message row carries the directed highlight (a left accent).
    const anchor = screen.getByText('@lead').closest('div[style]');
    expect(anchor).toHaveStyle({ borderLeftStyle: 'solid' });
    // Both kinds render as inline pills.
    expect(screen.getByText('question')).toBeInTheDocument();
    expect(screen.getByText('note')).toBeInTheDocument();
  });

  it('highlights participant tokens learned from the thread, inside bodies', async () => {
    await renderFeed();
    // `elk` is learned from recon#1's `[elk / fawn-butte]` self-tag and then
    // highlighted wherever it appears — rendered as its own styled span.
    const spans = [...document.querySelectorAll('p span')];
    expect(spans.some((s) => s.textContent === 'elk')).toBe(true);
    expect(spans.some((s) => s.textContent === 'fawn-butte')).toBe(true);
    // The body text survives intact across the highlight spans.
    expect(document.body.textContent).toContain('round 1 blocked');
  });

  it('pins lead directives and folds retractions (struck, 0 standing)', async () => {
    await renderFeed();
    expect(screen.getByText(/0 standing/)).toBeInTheDocument();
    // The one directive was retracted → shown struck with a `retracted` badge,
    // not as a blank row.
    expect(screen.getByText('retracted')).toBeInTheDocument();
    expect(screen.getByText(/scope discipline: stay strictly in scope/)).toBeInTheDocument();
  });

  it('lists the channel participants', async () => {
    await renderFeed();
    // Each author appears in the participants bar (and again in its message
    // header), so there are at least two matches.
    expect(screen.getAllByText('recon#1').length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText('service-analyst#3').length).toBeGreaterThanOrEqual(2);
  });
});
