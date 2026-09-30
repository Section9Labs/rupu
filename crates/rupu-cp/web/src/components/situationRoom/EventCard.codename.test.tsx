// @vitest-environment jsdom
// EventCard codename rendering: the agent line shows the codename member label
// (leaf · agent · provider/model), the run link reads the crew name instead of
// the 8-char id, and the card carries a crew tint stripe from the palette.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import EventCard from './EventCard';
import { cardFromEvent } from '../../lib/situationRoom/cards';
import { crewTint } from '../../lib/codename';
import type { AgentStartedEvent, StepStartedEvent } from '../../lib/api';

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
});
