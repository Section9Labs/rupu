// @vitest-environment jsdom
// EventCard codename rendering: the agent line shows the codename member label
// (leaf · agent · provider/model), the run link reads the crew name instead of
// the 8-char id, and the card carries a crew tint stripe from the palette.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import EventCard from './EventCard';
import { agentUnitIndex, cardFromEvent, unitKeyIndex } from '../../lib/situationRoom/cards';
import { crewTint } from '../../lib/codename';
import type { AgentStartedEvent, StepStartedEvent, UnitStartedEvent } from '../../lib/api';

afterEach(cleanup);

const agentStarted: AgentStartedEvent = {
  type: 'agent_started', run_id: 'run_01KS19A4MQXP', step_id: 'review', unit_index: 4,
  codename: 'jade-reef/heron#4', agent: 'sec-reviewer', provider: 'anthropic',
  model: 'claude-sonnet-4-6', agent_run_id: 'ar1', transcript_path: 't/ar1.jsonl',
};

describe('EventCard — codenames', () => {
  it('shows the agent name with its provider and model, once', () => {
    render(<MemoryRouter><EventCard card={cardFromEvent(agentStarted, 1000, 'k')!} /></MemoryRouter>);
    const label = 'heron#4 · sec-reviewer · anthropic/claude-sonnet-4-6';
    expect(screen.getAllByText(label)).toHaveLength(1);
  });

  it('run link reads the crew name instead of the id slice', () => {
    render(<MemoryRouter><EventCard card={cardFromEvent(agentStarted, 1000, 'k')!} /></MemoryRouter>);
    const link = screen.getByRole('link', { name: 'jade-reef' });
    expect(link).toHaveAttribute('href', '/runs/run_01KS19A4MQXP');
    expect(screen.queryByText('run_01KS')).not.toBeInTheDocument();
  });

  it('paints a crew tint stripe', () => {
    const { container } = render(<MemoryRouter><EventCard card={cardFromEvent(agentStarted, 1000, 'k')!} /></MemoryRouter>);
    const stripe = container.querySelector('[data-testid="sr-crew-stripe"]') as HTMLElement;
    expect(stripe).not.toBeNull();
    const tint = crewTint('jade-reef', 'light');
    expect(tint).toBeDefined();
    expect(stripe.style.backgroundColor).not.toBe('');
  });

  it('legacy card (no codename) keeps the plain agent + id slice', () => {
    const ev: StepStartedEvent = { type: 'step_started', run_id: 'run_01KS19A4MQXP', step_id: 'scan', kind: 'linear', agent: 'oracle' };
    const { container } = render(<MemoryRouter><EventCard card={cardFromEvent(ev, 1000, 'k')!} /></MemoryRouter>);
    expect(screen.getByText('oracle')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'run_01KS' })).toBeInTheDocument();
    expect(container.querySelector('[data-testid="sr-crew-stripe"]')).toBeNull();
  });

  it('legacy unit_started with a DERIVED name still gets its card, rendered muted', () => {
    const ev: UnitStartedEvent = {
      type: 'unit_started', run_id: 'run_01KS19A4MQXP', step_id: 'fan', index: 0,
      unit_key: 'a.rs', agent: 'triage', transcript_path: '/t/u0.jsonl',
      codename: 'jade-reef/numbat#1', codename_derived: true,
    };
    const card = cardFromEvent(ev, 1000, 'k');
    expect(card).not.toBeNull();
    expect(card!.codenameDerived).toBe(true);
    render(<MemoryRouter><EventCard card={card!} /></MemoryRouter>);
    const name = screen.getByText('numbat#1 · triage').closest('[title]') as HTMLElement;
    expect(name.className).toMatch(/opacity-60/);
    expect(name.getAttribute('title')).toMatch(/derived for a run recorded before codenames/);
  });

  it('derived step_started card is muted; a stored one is not', () => {
    const derived: StepStartedEvent = {
      type: 'step_started', run_id: 'run_01KS19A4MQXP', step_id: 'scan', kind: 'linear',
      agent: 'oracle', codename: 'jade-reef/heron', codename_derived: true,
    };
    render(<MemoryRouter><EventCard card={cardFromEvent(derived, 1000, 'k')!} /></MemoryRouter>);
    expect((screen.getByText('heron · oracle').closest('[title]') as HTMLElement).className).toMatch(/opacity-60/);
    cleanup();
    const stored: StepStartedEvent = { ...derived, codename_derived: undefined };
    render(<MemoryRouter><EventCard card={cardFromEvent(stored, 1000, 'k')!} /></MemoryRouter>);
    expect((screen.getByText('heron · oracle').closest('[title]') as HTMLElement).className).not.toMatch(/opacity-60/);
  });

  it('pre-codename run with BOTH a derived unit_started and agent_started renders one card per unit', () => {
    const unit: UnitStartedEvent = {
      type: 'unit_started', run_id: 'run_L', step_id: 'fan', index: 0,
      unit_key: 'a.rs', agent: 'triage', transcript_path: '/t/u0.jsonl',
      codename: 'jade-reef/numbat#1', codename_derived: true,
    };
    const agent: AgentStartedEvent = {
      type: 'agent_started', run_id: 'run_L', step_id: 'fan', unit_index: 0,
      codename: 'jade-reef/numbat#1', codename_derived: true, agent: 'triage',
      agent_run_id: 'ar', transcript_path: '/t/u0.jsonl',
    };
    const other: UnitStartedEvent = { ...unit, index: 1, codename: 'jade-reef/numbat#2' };
    const evs = [unit, agent, other];
    const ctx = { unitKeys: unitKeyIndex(evs), agentUnits: agentUnitIndex(evs) };
    const cards = evs.map((e, i) => cardFromEvent(e, 1000, `k${i}`, ctx)).filter(Boolean);
    // unit 0: only the agent_started card (muted, carrying the unit key);
    // unit 1 has no agent_started, so its unit_started card stays.
    expect(cards).toHaveLength(2);
    expect(cards[0]!.badge).toBe('Agent');
    expect(cards[0]!.codenameDerived).toBe(true);
    expect(cards[0]!.unitKey).toBe('a.rs');
    expect(cards[1]!.codename).toBe('jade-reef/numbat#2');
  });

  it('a stored-name unit_started stays suppressed (agent_started carries it)', () => {
    const ev: UnitStartedEvent = {
      type: 'unit_started', run_id: 'run_01KS19A4MQXP', step_id: 'fan', index: 0,
      unit_key: 'a.rs', agent: 'triage', transcript_path: '/t/u0.jsonl', codename: 'jade-reef/numbat#1',
    };
    expect(cardFromEvent(ev, 1000, 'k')).toBeNull();
  });
});
