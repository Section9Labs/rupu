// @vitest-environment jsdom
// EventRow: a finding's excerpt, file deep link and permalink; a JSON error's
// Parsed/Raw toggle; a warning as a quiet amber line; a gate's Approve /
// Reject through the page's gate (none without one); codenames at body size.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import EventRow, { type EventRowProps } from './EventRow';
import { IDLE_GATE, type EventGate } from './GateButtons';
import { cardFromEvent, cardFromFinding } from '../../lib/situationRoom/cards';
import type {
  AgentStartedEvent,
  FindingOut,
  StepAwaitingApprovalEvent,
  StepFailedEvent,
  StepStartedEvent,
  StepWarningEvent,
} from '../../lib/api';
import type { StreamCard } from '../../lib/situationRoom/cards';

afterEach(cleanup);

function row(card: StreamCard, p: Partial<EventRowProps> = {}) {
  return render(
    <MemoryRouter>
      <EventRow
        card={card}
        scope="live"
        resolved={{}}
        seen={false}
        threadUp={false}
        threadDn={false}
        open={false}
        arrived={false}
        timed
        onToggle={() => {}}
        {...p}
      />
    </MemoryRouter>,
  );
}

const finding: FindingOut = {
  codename: 'cobalt-harbor/heron#1',
  codename_derived: false,
  id: 'f1',
  ws_id: 'ws1',
  project: 'billing-api',
  target_id: 't1',
  file_path: 'src/routes/billing.ts',
  line_range: [17, 17],
  scope: null,
  summary: 'Broken org-scoping on GET /invoice/:id',
  severity: 'high',
  concern_id: null,
  evidence: { rationale: 'orgId checked for truthiness', code_excerpt: 'if (invoice.orgId) {\n  return res.json(invoice)\n}' },
  declared_by: null,
  declared_at: '2026-07-21T10:00:00Z',
  permalink: 'https://example.com/blob/x#L16',
};

describe('EventRow — finding', () => {
  it('is a raised card with its excerpt, the finding line washed, a code deep link and the permalink', () => {
    const { container } = row(cardFromFinding(finding));
    expect(container.querySelector('[data-evkey]')).toHaveAttribute('data-raised');
    expect(screen.getByText('Broken org-scoping on GET /invoice/:id')).toBeInTheDocument();
    expect(screen.getByLabelText('code excerpt')).toBeInTheDocument();
    expect(screen.getByText('17').closest('.evx-code-l')).toHaveAttribute('data-hit');
    expect(screen.getByText('18').closest('.evx-code-l')).not.toHaveAttribute('data-hit');
    const link = screen.getByText('src/routes/billing.ts:17-17').closest('a');
    expect(link).toHaveAttribute('href', expect.stringContaining('/projects/ws1/code?path='));
    expect(screen.getByText('View on repository').closest('a')).toHaveAttribute('href', finding.permalink);
    expect(container.querySelector('.evx-cap')).toHaveAttribute('data-mark', 'high');
  });

  it('says seen once it was', () => {
    row(cardFromFinding(finding), { seen: true });
    expect(screen.getByText('seen')).toBeInTheDocument();
  });
});

describe('EventRow — error', () => {
  it('offers a Parsed/Raw toggle for a JSON error', () => {
    const ev: StepFailedEvent = { type: 'step_failed', run_id: 'r1', step_id: 'panel', error: 'provider error: {"code":429,"retry":true}' };
    row(cardFromEvent(ev, 1000, 'k1')!);
    expect(screen.getByRole('button', { name: /parsed/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /raw/i }));
    expect(screen.getByText(/provider error: \{"code":429/)).toBeInTheDocument();
  });

  it('shows a plain error as its own block', () => {
    const ev: StepFailedEvent = { type: 'step_failed', run_id: 'r1', step_id: 'review', error: 'agent run ended in status Error (max turns (10) reached)' };
    const { container } = row(cardFromEvent(ev, 1000, 'k1')!);
    expect(screen.getByText('review failed')).toBeInTheDocument();
    expect(container.querySelector('.evx-detail')).toHaveTextContent('max turns (10) reached');
  });
});

describe('EventRow — warning', () => {
  const ev: StepWarningEvent = {
    type: 'step_warning',
    run_id: 'r1',
    step_id: 'sweep',
    index: 4,
    message: 'host gpu-9 did not stream coverage (older rupu?)',
  };

  it('is a quiet amber line with the message as its headline', () => {
    const { container } = row(cardFromEvent(ev, 1000, 'k1')!);
    const el = container.querySelector('[data-evkey]')!;
    expect(el).not.toHaveAttribute('data-raised');
    expect(el).toHaveAttribute('data-accent', 'warn');
    expect(el).toHaveAttribute('data-mark', 'warning');
    expect(screen.getByText('warning')).toHaveClass('evx-kw');
    expect(screen.getByText('host gpu-9 did not stream coverage (older rupu?)')).toHaveClass('evx-hl');
    expect(screen.queryByRole('button', { name: /parsed/i })).not.toBeInTheDocument();
  });
});

describe('EventRow — gate', () => {
  const ev: StepAwaitingApprovalEvent = { type: 'step_awaiting_approval', run_id: 'r1', step_id: 'deploy', reason: 'ship it?' };
  const gate = (over: Partial<EventGate> = {}): EventGate => ({ state: () => IDLE_GATE, approve: vi.fn(), reject: vi.fn(), ...over });

  it('approves through the page gate', () => {
    const g = gate();
    const card = cardFromEvent(ev, 1000, 'k1')!;
    const { container } = row(card, { gate: g, gateOutcome: 'open' });
    expect(container.querySelector('[data-evkey]')).toHaveAttribute('data-waiting');
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }));
    expect(g.approve).toHaveBeenCalledWith(card);
  });

  it('asks once more before it rejects', () => {
    const g = gate();
    row(cardFromEvent(ev, 1000, 'k1')!, { gate: g, gateOutcome: 'open' });
    fireEvent.click(screen.getByRole('button', { name: 'Reject' }));
    expect(g.reject).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Confirm rejection for deploy' }));
    expect(g.reject).toHaveBeenCalled();
  });

  it('says "resuming" once approved here, until the run moves the gate on', () => {
    const g = gate({ state: () => ({ pending: false, decision: 'approved', error: null }) });
    const { container } = row(cardFromEvent(ev, 1000, 'k1')!, { gate: g, gateOutcome: 'open' });
    expect(screen.getByTestId('gate-resolved')).toHaveTextContent('approved · resuming…');
    expect(container.querySelector('[data-evkey]')).toHaveAttribute('data-mark', 'running');
  });

  it('shows no buttons without a gate, and the outcome once the run moved on', () => {
    row(cardFromEvent(ev, 1000, 'k1')!, { gateOutcome: 'open' });
    expect(screen.queryByRole('button', { name: 'Approve' })).not.toBeInTheDocument();
    expect(screen.getByText('Awaiting approval')).toBeInTheDocument();
    cleanup();
    const { container } = row(cardFromEvent(ev, 1000, 'k1')!, { gateOutcome: 'rejected', gate: gate() });
    expect(screen.queryByRole('button', { name: 'Approve' })).not.toBeInTheDocument();
    expect(container.querySelector('[data-evkey]')).toHaveAttribute('data-mark', 'failed');
  });
});

describe('EventRow — codenames', () => {
  const agentStarted: AgentStartedEvent = {
    type: 'agent_started',
    run_id: 'run_01KS19A4MQXP',
    step_id: 'review',
    unit_index: 4,
    codename: 'jade-reef/heron#4',
    agent: 'sec-reviewer',
    provider: 'anthropic',
    model: 'claude-sonnet-4-6',
    agent_run_id: 'ar1',
    transcript_path: 't/ar1.jsonl',
  };

  it('on Live Events: crew/leaf in mono linking the run, then provider/model', () => {
    const { container } = row(cardFromEvent(agentStarted, 1000, 'k')!);
    const cn = container.querySelector('.evx-cn')!;
    expect(cn).toHaveTextContent('jade-reef/heron#4');
    expect(cn.closest('a')).toHaveAttribute('href', '/runs/run_01KS19A4MQXP');
    expect(screen.getByText('anthropic/claude-sonnet-4-6')).toBeInTheDocument();
    expect(screen.getByText('agent started')).toBeInTheDocument();
  });

  it('on a run tab: the leaf alone, no run link', () => {
    const { container } = row(cardFromEvent(agentStarted, 1000, 'k')!, { scope: 'run' });
    const cn = container.querySelector('.evx-cn')!;
    expect(cn).toHaveTextContent(/^heron#4$/);
    expect(cn.closest('a')).toBeNull();
  });

  it('a derived name renders muted, with why', () => {
    const ev: StepStartedEvent = {
      type: 'step_started',
      run_id: 'run_1',
      step_id: 'scan',
      kind: 'linear',
      agent: 'oracle',
      codename: 'jade-reef/heron',
      codename_derived: true,
    };
    const { container } = row(cardFromEvent(ev, 1000, 'k')!);
    const cn = container.querySelector('.evx-cn')!;
    expect(cn).toHaveAttribute('data-derived');
    expect(cn.getAttribute('title')).toMatch(/derived for a run recorded before codenames/);
  });

  it('a legacy run with no codename links the run by its id', () => {
    const ev: StepStartedEvent = { type: 'step_started', run_id: 'run_01KS19A4MQXP', step_id: 'scan', kind: 'linear', agent: 'oracle' };
    row(cardFromEvent(ev, 1000, 'k')!);
    expect(screen.getByRole('link', { name: 'run_01KS' })).toHaveAttribute('href', '/runs/run_01KS19A4MQXP');
  });

  it('opens the transcript of a row that carries one', () => {
    const onToggle = vi.fn();
    row(cardFromEvent(agentStarted, 1000, 'k')!, { onToggle });
    fireEvent.click(screen.getByRole('button', { name: /agent started: show what happened/ }));
    expect(onToggle).toHaveBeenCalledWith('k');
  });
});
