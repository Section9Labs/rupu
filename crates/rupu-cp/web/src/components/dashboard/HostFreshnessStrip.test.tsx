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
});
