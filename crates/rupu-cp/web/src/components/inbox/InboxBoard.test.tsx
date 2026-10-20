// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { InboxBoard } from './InboxBoard';
import type { InboxItem, UseInbox } from '../../lib/dashboard/useInbox';

afterEach(cleanup);

const NOW = Date.UTC(2026, 0, 2, 12, 0, 0);

function item(over: Partial<InboxItem>): InboxItem {
  return {
    id: 'run_1',
    host: 'local',
    codename: 'cobalt-harbor/heron',
    codenameDerived: false,
    workflowName: 'api-migration',
    status: 'awaiting_approval',
    kind: 'awaiting',
    since: new Date(NOW - 2 * 3_600_000).toISOString(), // 2h ago
    ...over,
  };
}

function base(over: Partial<UseInbox>): UseInbox {
  return {
    actionable: [],
    failures: [],
    snoozed: [],
    dismissed: [],
    counts: { gates: 0, paused: 0, failed: 0 },
    stalledMs: 0,
    oldestActionable: null,
    clearedThisSession: 0,
    loading: false,
    error: null,
    pending: {},
    now: NOW,
    approve: vi.fn(),
    reject: vi.fn(),
    resume: vi.fn(),
    snooze: vi.fn(),
    dismiss: vi.fn(),
    unsnooze: vi.fn(),
    undismiss: vi.fn(),
    refresh: vi.fn(),
    ...over,
  };
}

function wrap(el: React.ReactElement) {
  return render(<MemoryRouter>{el}</MemoryRouter>);
}

describe('InboxBoard', () => {
  it('shows the inbox-zero fact sheet when nothing is waiting', () => {
    wrap(<InboxBoard data={base({ clearedThisSession: 3 })} />);
    expect(screen.getByText('Nothing waiting on you.')).toBeInTheDocument();
    expect(screen.getByText(/3/)).toBeInTheDocument(); // cleared this session
  });

  it('renders a gate row with Approve/Reject and always-visible Snooze/Dismiss, and acts', () => {
    const approve = vi.fn();
    const it0 = item({ id: 'g1', kind: 'awaiting', status: 'awaiting_approval' });
    wrap(
      <InboxBoard
        data={base({ actionable: [it0], counts: { gates: 1, paused: 0, failed: 0 }, approve })}
      />,
    );
    expect(screen.getByText('Waiting on you')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reject' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Snooze' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Dismiss' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }));
    expect(approve).toHaveBeenCalledWith(it0);
  });

  it('a paused row offers Resume', () => {
    const resume = vi.fn();
    const it0 = item({ id: 'p1', kind: 'paused', status: 'paused' });
    wrap(<InboxBoard data={base({ actionable: [it0], resume })} />);
    fireEvent.click(screen.getByRole('button', { name: 'Resume' }));
    expect(resume).toHaveBeenCalledWith(it0);
  });

  it('a failed row links out and can be dismissed, under a separate section', () => {
    const dismiss = vi.fn();
    const it0 = item({ id: 'f1', kind: 'failed', status: 'failed', workflowName: 'lint-sweep' });
    wrap(<InboxBoard data={base({ failures: [it0], counts: { gates: 0, paused: 0, failed: 1 }, dismiss })} />);
    expect(screen.getByText('Recent failures')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'View run' })).toHaveAttribute('href', '/runs/f1');
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(dismiss).toHaveBeenCalledWith(it0);
  });

  it('the filter narrows which rows show', () => {
    const gate = item({ id: 'g1', kind: 'awaiting' });
    const failed = item({ id: 'f1', kind: 'failed', status: 'failed', workflowName: 'lint-sweep' });
    wrap(
      <InboxBoard
        filter="gates"
        data={base({ actionable: [gate], failures: [failed], counts: { gates: 1, paused: 0, failed: 1 } })}
      />,
    );
    expect(screen.getByText('Waiting on you')).toBeInTheDocument();
    expect(screen.queryByText('Recent failures')).not.toBeInTheDocument();
  });

  it('surfaces a snoozed drawer with un-snooze', () => {
    const unsnooze = vi.fn();
    const sn = item({ id: 's1', kind: 'awaiting' });
    wrap(
      <InboxBoard
        data={base({ actionable: [item({ id: 'g1' })], snoozed: [{ item: sn, until: NOW + 3_600_000 }], unsnooze })}
      />,
    );
    // drawer collapsed: shows the count; expand to reach un-snooze
    fireEvent.click(screen.getByRole('button', { name: /Snoozed/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Un-snooze' }));
    expect(unsnooze).toHaveBeenCalledWith('s1');
  });
});
