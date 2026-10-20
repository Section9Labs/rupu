// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { FleetHostCards } from './FleetHostCards';
import type { DashboardHostState } from '../../lib/dashboard/useDashboardData';

afterEach(cleanup);

const host = (over: Partial<DashboardHostState>): DashboardHostState => ({
  hostId: 'local',
  name: 'local',
  transportKind: 'local',
  state: 'ok',
  ...over,
});

const wrap = (ui: React.ReactNode) => render(<MemoryRouter>{ui}</MemoryRouter>);

describe('FleetHostCards', () => {
  it('renders nothing for an empty fleet', () => {
    const { container } = wrap(<FleetHostCards hosts={[]} />);
    expect(container.firstChild).toBeNull();
  });

  it('shows a live state with the running count when a host has runs', () => {
    wrap(
      <FleetHostCards
        hosts={[
          host({
            hostId: 'mini',
            name: 'mini',
            state: 'ok',
            // minimal summary shape — only the fields the card reads
            summary: { active: { running: 1 } } as DashboardHostState['summary'],
          }),
        ]}
      />,
    );
    expect(screen.getByText('mini')).toBeInTheDocument();
    expect(screen.getByText('live · 1 run')).toBeInTheDocument();
  });

  it('shows idle for an ok host with no runs', () => {
    wrap(<FleetHostCards hosts={[host({ summary: { active: { running: 0 } } as DashboardHostState['summary'] })]} />);
    expect(screen.getByText('idle')).toBeInTheDocument();
  });

  it('shows offline with the reason as a title', () => {
    wrap(<FleetHostCards hosts={[host({ hostId: 'kali6', name: 'kali6', state: 'offline', reason: 'ssh refused' })]} />);
    const state = screen.getByText('offline');
    expect(state).toHaveAttribute('title', 'ssh refused');
  });
});
