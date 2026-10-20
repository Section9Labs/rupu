// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { NeedsYou } from './NeedsYou';
import type { NeedsYouItem, UseNeedsYou } from '../../lib/dashboard/useNeedsYou';

afterEach(cleanup);

const item = (over: Partial<NeedsYouItem>): NeedsYouItem => ({
  id: 'r1',
  host: 'local',
  codename: 'cobalt-harbor/heron#4',
  codenameDerived: false,
  workflowName: 'network-assessment',
  status: 'awaiting_approval',
  kind: 'awaiting',
  since: new Date(Date.now() - 2 * 3600_000).toISOString(),
  ...over,
});

const base = (over: Partial<UseNeedsYou>): UseNeedsYou => ({
  items: [],
  loading: false,
  error: null,
  pending: {},
  approve: vi.fn(),
  reject: vi.fn(),
  resume: vi.fn(),
  refresh: vi.fn(),
  ...over,
});

const wrap = (ui: React.ReactNode) => render(<MemoryRouter>{ui}</MemoryRouter>);

describe('NeedsYou', () => {
  it('shows an empty state keeping the frame when nothing needs you', () => {
    wrap(<NeedsYou data={base({})} />);
    expect(screen.getByText('Nothing needs you')).toBeInTheDocument();
  });

  it('shows skeleton rows while loading with no items', () => {
    const { container } = wrap(<NeedsYou data={base({ loading: true })} />);
    expect(container.querySelector('.animate-pulse')).not.toBeNull();
  });

  it('awaiting row wires Approve and Reject', () => {
    const approve = vi.fn();
    const reject = vi.fn();
    const it0 = item({ id: 'gate-1', kind: 'awaiting' });
    wrap(<NeedsYou data={base({ items: [it0], approve, reject })} />);

    expect(screen.getByText('gate')).toBeInTheDocument();
    expect(screen.getByText(/waiting 2h/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Approve' }));
    fireEvent.click(screen.getByRole('button', { name: 'Reject' }));
    expect(approve).toHaveBeenCalledWith(it0);
    expect(reject).toHaveBeenCalledWith(it0);
  });

  it('paused row wires Resume; buttons disabled while pending', () => {
    const resume = vi.fn();
    const it0 = item({ id: 'p1', kind: 'paused', status: 'paused' });
    wrap(<NeedsYou data={base({ items: [it0], resume, pending: { p1: 'resume' } })} />);
    const btn = screen.getByRole('button', { name: 'Resuming…' });
    expect(btn).toBeDisabled();
  });

  it('failed row links to the run instead of a mutation', () => {
    const it0 = item({ id: 'f1', kind: 'failed', status: 'failed' });
    wrap(<NeedsYou data={base({ items: [it0] })} />);
    expect(screen.getByText(/failed .* ago/)).toBeInTheDocument();
    const view = screen.getByRole('link', { name: 'View run' });
    expect(view).toHaveAttribute('href', expect.stringContaining('f1'));
    expect(screen.queryByRole('button', { name: /Resume|Approve/ })).toBeNull();
  });
});
