// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { FleetStrip } from './FleetStrip';
import type { DashboardHostState } from '../../lib/dashboard/useDashboardData';
import type { DashboardSummary } from '../../lib/api';

afterEach(cleanup);

function summary(running: number): DashboardSummary {
  return {
    active: { running, awaiting_approval: 0, paused: 0, pending: 0 },
    active_longest: null,
    terminal_buckets: [],
    throughput_buckets: [],
    cycles: { total: 0, clean: 0, with_failures: 0 },
    findings_open: 0,
    captured_at: new Date().toISOString(),
    fleet: {} as DashboardSummary['fleet'],
  };
}

function host(over: Partial<DashboardHostState>): DashboardHostState {
  return { hostId: 'h', name: 'h', transportKind: 'local', state: 'ok', ...over };
}

const wrap = (hosts: DashboardHostState[]) =>
  render(
    <MemoryRouter>
      <FleetStrip hosts={hosts} />
    </MemoryRouter>,
  );

describe('FleetStrip', () => {
  it('summarises live vs down and renders a chip per host', () => {
    const { container } = wrap([
      host({ hostId: 'local', name: 'local', state: 'ok', summary: summary(0) }),
      host({ hostId: 'sutajio', name: 'sutajio', state: 'offline', reason: 'ssh timeout' }),
      host({ hostId: 'kali', name: 'kali', state: 'ok', summary: summary(2) }),
    ]);
    // 2 live (local idle + kali running), 1 down
    expect(container).toHaveTextContent('2 of 3 live');
    expect(container).toHaveTextContent('1 down');
    expect(screen.getByTestId('host-local')).toBeInTheDocument();
    expect(screen.getByTestId('host-sutajio')).toBeInTheDocument();
    expect(screen.getByTestId('host-kali')).toBeInTheDocument();
  });

  it('shows a run count on a running host and links each chip to /hosts', () => {
    wrap([host({ hostId: 'kali', name: 'kali', state: 'ok', summary: summary(3) })]);
    const chip = screen.getByTestId('host-kali');
    expect(chip).toHaveAttribute('href', '/hosts');
    expect(within(chip).getByText('3')).toBeInTheDocument();
  });

  it('orders active hosts before down ones', () => {
    wrap([
      host({ hostId: 'down1', name: 'down1', state: 'offline', reason: 'x' }),
      host({ hostId: 'busy', name: 'busy', state: 'ok', summary: summary(5) }),
    ]);
    const chips = screen.getAllByTestId(/^host-/);
    expect(chips[0]).toHaveAttribute('data-testid', 'host-busy');
    expect(chips[1]).toHaveAttribute('data-testid', 'host-down1');
  });

  it('renders nothing when there are no hosts', () => {
    const { container } = wrap([]);
    expect(container).toBeEmptyDOMElement();
  });
});
