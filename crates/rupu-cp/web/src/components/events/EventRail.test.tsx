// @vitest-environment jsdom
// EventRail: the Needs-you deck pins what needs the operator (and lets go of
// what they saw or answered), rows of one run are threaded, five-minute
// separators head the groups, and scrolled down, new rows wait behind the
// "N new" pill instead of moving what is being read.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import EventRail, { type EventRailProps } from './EventRail';
import { IDLE_GATE, type EventGate } from './GateButtons';
import { resetSeen } from '../../lib/events/seen';
import type { StreamCard } from '../../lib/situationRoom/cards';

afterEach(cleanup);
beforeEach(() => resetSeen());

const NOW = Date.now();
let n = 0;
function card(p: Partial<StreamCard>): StreamCard {
  return { key: `k${n++}`, ts: NOW, form: 'activity', group: 'activity', accent: 'brand', badge: 'Working', title: 'scan', ...p };
}

function rail(cards: StreamCard[], p: Partial<EventRailProps> = {}) {
  const props: EventRailProps = { cards, scope: 'live', live: true, nowSub: '1 run live', emptyText: 'Waiting for events…', ...p };
  const out = render(
    <MemoryRouter>
      <EventRail {...props} />
    </MemoryRouter>,
  );
  return {
    ...out,
    rerender: (next: StreamCard[], more: Partial<EventRailProps> = {}) =>
      out.rerender(
        <MemoryRouter>
          <EventRail {...props} {...more} cards={next} />
        </MemoryRouter>,
      ),
  };
}

const gate = card({ form: 'await', group: 'await', accent: 'await', badge: 'Awaiting you', runId: 'r1', stepId: 'ship', title: 'Approval needed · ship', detail: 'ship it?', ts: NOW - 1000 });
const fail = card({ form: 'error', group: 'error', accent: 'error', badge: 'Error', runId: 'r2', stepId: 'build', title: 'build failed', detail: 'exit 2', ts: NOW - 2000 });
const high = card({ key: 'finding:f1', form: 'finding', group: 'finding', accent: 'high', severity: 'high', badge: 'High', title: 'Session cookie without Secure', ts: NOW - 3000 });
const low = card({ form: 'finding', group: 'finding', accent: 'low', severity: 'low', badge: 'Low', title: 'Debug endpoint', ts: NOW - 4000 });
const work = card({ runId: 'r1', stepId: 'scan', detail: 'Reading a.rs', crew: 'cobalt-harbor', ts: NOW - 5000 });
const work2 = card({ runId: 'r1', stepId: 'scan', detail: 'Reading b.rs', crew: 'cobalt-harbor', ts: NOW - 6000 });

describe('EventRail — the Needs-you deck', () => {
  it('pins the gate first, then the failure, then the severe finding; never a low one', () => {
    rail([gate, fail, high, low, work, work2]);
    const deck = screen.getByTestId('needs-you-deck');
    expect(deck).toHaveTextContent('1 gate · 1 failure · 1 finding');
    const cards = deck.querySelectorAll('[data-dkey]');
    expect([...cards].map((c) => c.getAttribute('data-dkey'))).toEqual([gate.key, fail.key, high.key]);
    expect(cards[0]).toHaveAttribute('data-front');
  });

  it('is absent when nothing needs you', () => {
    rail([low, work]);
    expect(screen.queryByTestId('needs-you-deck')).toBeNull();
  });

  it('Seen lets a card go and leaves a quiet tag on its row', () => {
    rail([fail, work]);
    const deck = screen.getByTestId('needs-you-deck');
    fireEvent.click(within(deck).getByRole('button', { name: 'Mark build failed seen' }));
    expect(screen.queryByTestId('needs-you-deck')).toBeNull();
    const row = document.querySelector(`[data-evkey="${fail.key}"]`)!;
    expect(within(row as HTMLElement).getByText('seen')).toBeInTheDocument();
  });

  it('a gate answered here leaves the deck and its row says so', () => {
    let decided: 'approved' | null = null;
    const g: EventGate = {
      state: () => (decided ? { pending: false, decision: decided, error: null } : IDLE_GATE),
      approve: () => {
        decided = 'approved';
      },
      reject: () => {},
    };
    const r = rail([gate, work], { gate: g });
    const deck = screen.getByTestId('needs-you-deck');
    fireEvent.click(within(deck).getByRole('button', { name: 'Approve' }));
    r.rerender([gate, work], { gate: { ...g } }); // the page re-renders with its new gate state
    expect(screen.queryByTestId('needs-you-deck')).toBeNull();
    expect(screen.getByTestId('gate-resolved')).toHaveTextContent('approved · resuming…');
  });

  it('a gate its run moved past is no longer pinned', () => {
    const done = card({ form: 'complete', badge: 'Step done', runId: 'r1', stepId: 'ship', ts: NOW });
    rail([done, gate]);
    expect(screen.queryByTestId('needs-you-deck')).toBeNull();
    expect(document.querySelector(`[data-evkey="${gate.key}"]`)).toHaveAttribute('data-mark', 'done');
  });
});

describe('EventRail — the rail', () => {
  it('threads consecutive rows of one run', () => {
    rail([work, work2]);
    expect(document.querySelector(`[data-evkey="${work.key}"]`)).toHaveAttribute('data-th-dn');
    expect(document.querySelector(`[data-evkey="${work2.key}"]`)).toHaveAttribute('data-th-up');
  });

  it("spins the newest row of a live run, and only that one", () => {
    rail([work, work2], { isRunLive: () => true });
    expect(document.querySelector(`[data-evkey="${work.key}"]`)).toHaveAttribute('data-mark', 'running');
    expect(document.querySelector(`[data-evkey="${work2.key}"]`)).toHaveAttribute('data-mark', 'activity');
  });

  it('heads five-minute groups with a separator that says how long ago', () => {
    const old = card({ runId: 'r9', ts: NOW - 20 * 60_000 });
    rail([work, old]);
    const seps = document.querySelectorAll('.evx-sep');
    expect(seps).toHaveLength(2);
    expect(seps[0]).toHaveTextContent('just now');
    expect(seps[1]).toHaveTextContent('20 min ago');
  });

  it('says what it is waiting for when empty', () => {
    rail([]);
    expect(screen.getByText('Waiting for events…')).toBeInTheDocument();
  });

  it('holds new rows behind the pill while scrolled down, and lets them in at the top', () => {
    const r = rail([work, work2]);
    const scroll = document.querySelector('.evx-scroll') as HTMLElement;
    act(() => {
      scroll.scrollTop = 400;
      scroll.dispatchEvent(new Event('scroll'));
    });
    const fresh = card({ runId: 'r1', detail: 'Reading c.rs', ts: NOW + 1000 });
    r.rerender([fresh, work, work2]);
    expect(document.querySelector(`[data-evkey="${fresh.key}"]`)).toBeNull();
    expect(screen.getByRole('button', { name: /1 new/ })).toBeInTheDocument();
    act(() => {
      scroll.scrollTop = 0;
      scroll.dispatchEvent(new Event('scroll'));
    });
    const row = document.querySelector(`[data-evkey="${fresh.key}"]`);
    expect(row).not.toBeNull();
    expect(row).toHaveAttribute('data-arrived');
    expect(screen.queryByRole('button', { name: /new/ })).toBeNull();
  });
});
