// @vitest-environment jsdom
// EventCard — the new rendering paths added in the Situation Room polish pass:
// findings render a line-numbered/highlighted code excerpt + deep link; error
// details with embedded JSON get a Parsed/Raw toggle; awaiting cards show
// Approve/Reject only when handlers are wired (per-run feeds omit them).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import EventCard from './EventCard';
import { cardFromEvent, cardFromFinding } from '../../lib/situationRoom/cards';
import type { FindingOut, StepAwaitingApprovalEvent, StepFailedEvent } from '../../lib/api';

afterEach(cleanup);

function renderCard(ui: React.ReactNode) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

const finding: FindingOut = {
  id: 'f1', ws_id: 'ws1', project: 'billing-api', target_id: 't1',
  file_path: 'src/routes/billing.ts', line_range: [16, 18], scope: null,
  summary: 'Broken org-scoping on GET /invoice/:id', severity: 'high', concern_id: null,
  evidence: { rationale: 'orgId checked for truthiness', code_excerpt: 'if (invoice.orgId) {\n  return res.json(invoice)\n}' },
  declared_by: null, declared_at: '2026-07-21T10:00:00Z', permalink: 'https://example.com/blob/x#L16',
};

describe('EventCard — finding', () => {
  it('renders a line-numbered code excerpt, file:line deep link, and permalink', () => {
    renderCard(<EventCard card={cardFromFinding(finding)} projectLabel="billing-api" />);
    expect(screen.getByText('Broken org-scoping on GET /invoice/:id')).toBeInTheDocument();
    expect(screen.getByLabelText('code excerpt')).toBeInTheDocument();
    // gutter starts at line_range[0]
    expect(screen.getByText('16')).toBeInTheDocument();
    // file:line deep-links to the project Code viewer
    const link = screen.getByText('src/routes/billing.ts:16-18').closest('a');
    expect(link).toHaveAttribute('href', expect.stringContaining('/projects/ws1/code?path='));
    expect(screen.getByText('View on repository').closest('a')).toHaveAttribute('href', finding.permalink);
  });
});

describe('EventCard — error', () => {
  it('offers a Parsed/Raw toggle for a JSON error and shows raw on toggle', () => {
    const ev: StepFailedEvent = { type: 'step_failed', run_id: 'r1', step_id: 'panel', error: 'provider error: {"code":429,"retry":true}' };
    renderCard(<EventCard card={cardFromEvent(ev, 1000, 'k1')!} />);
    expect(screen.getByRole('button', { name: /parsed/i })).toBeInTheDocument();
    const raw = screen.getByRole('button', { name: /raw/i });
    fireEvent.click(raw);
    expect(screen.getByText(/provider error: \{"code":429/)).toBeInTheDocument();
  });
});

describe('EventCard — awaiting', () => {
  const ev: StepAwaitingApprovalEvent = { type: 'step_awaiting_approval', run_id: 'r1', step_id: 'deploy', reason: 'ship it?' };

  it('shows Approve/Reject and invokes the handler when wired', async () => {
    const onApprove = vi.fn().mockResolvedValue(undefined);
    renderCard(<EventCard card={cardFromEvent(ev, 1000, 'k1')!} onApprove={onApprove} onReject={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }));
    expect(onApprove).toHaveBeenCalledWith('r1');
  });

  it('shows no buttons when handlers are omitted (per-run feed)', () => {
    renderCard(<EventCard card={cardFromEvent(ev, 1000, 'k1')!} />);
    expect(screen.queryByRole('button', { name: 'Approve' })).not.toBeInTheDocument();
    expect(screen.getByText('Awaiting approval')).toBeInTheDocument();
  });
});
