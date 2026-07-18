// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { HostFreshnessStrip } from './HostFreshnessStrip';
import type { HostFreshness } from '../../lib/api';

afterEach(() => {
  cleanup();
});

const host = (over: Partial<HostFreshness> = {}): HostFreshness => ({
  host_id: 'local',
  name: 'local',
  transport_kind: 'local',
  state: 'ok',
  captured_at: new Date().toISOString(),
  reason: null,
  ...over,
});

describe('HostFreshnessStrip', () => {
  it('renders a fresh host as live', () => {
    render(<HostFreshnessStrip hosts={[host()]} />);
    expect(screen.getByText(/live/i)).toBeInTheDocument();
  });

  it('renders an unavailable host with its reason, NOT as zero', () => {
    render(
      <HostFreshnessStrip
        hosts={[
          host({
            host_id: 'builder-01',
            name: 'builder-01',
            state: 'unavailable',
            captured_at: null,
            reason: 'needs rupu >= 0.49',
          }),
        ]}
      />,
    );
    expect(screen.getByText(/unavailable/i)).toBeInTheDocument();
    expect(screen.getByTitle(/needs rupu/i)).toBeInTheDocument();
    expect(screen.queryByText('0')).not.toBeInTheDocument();
  });

  it('renders a stale host with its age rather than claiming live', () => {
    const thirtySecondsAgo = new Date(Date.now() - 30_000).toISOString();
    render(
      <HostFreshnessStrip hosts={[host({ host_id: 'b', name: 'b', captured_at: thirtySecondsAgo })]} />,
    );
    expect(screen.queryByText(/live/i)).not.toBeInTheDocument();
    expect(screen.getByText(/30s/)).toBeInTheDocument();
  });

  // useDashboardData's per-host state is FOUR-valued (loading/ok/unavailable)
  // because it seeds every registered host as 'loading' the instant the host
  // list is known, before that host's own `getDashboard` call resolves. The
  // wire `HostFreshness.state` is only three-valued (ok/offline/unavailable)
  // because the SERVER never reports a host until it has already resolved.
  // This strip must render that fourth state distinctly, not fold it into
  // 'unavailable' (which would read as a dead host) or 'ok' (which would lie
  // about freshness).
  it('renders a loading host distinctly from ok, offline, and unavailable', () => {
    render(
      <HostFreshnessStrip
        hosts={[
          {
            host_id: 'builder-01',
            name: 'builder-01',
            transport_kind: 'ssh',
            state: 'loading',
            captured_at: null,
            reason: null,
          },
        ]}
      />,
    );
    expect(screen.getByText(/loading/i)).toBeInTheDocument();
    expect(screen.queryByText(/live/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/unavailable/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/offline/i)).not.toBeInTheDocument();
  });

  it('a loading host does not fabricate a captured_at age', () => {
    render(
      <HostFreshnessStrip
        hosts={[
          {
            host_id: 'builder-01',
            name: 'builder-01',
            transport_kind: 'ssh',
            state: 'loading',
            captured_at: null,
            reason: null,
          },
        ]}
      />,
    );
    // No age string (e.g. "30s", "5m") should appear for a host that has
    // never actually reported.
    expect(screen.queryByText(/^\d+[sm]$/)).not.toBeInTheDocument();
  });
});
