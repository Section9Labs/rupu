// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
// fireEvent, not user-event: @testing-library/user-event is NOT a dependency
// (see KeyPointTiles.test.tsx).
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import NetflowSummary from './NetflowSummary';
import type { HostRollup } from '../../lib/netflow';

afterEach(() => {
  cleanup();
});

/** Deliberately omits `p50_ms`/`p95_ms` — they must be genuinely ABSENT keys
 *  (serde's `skip_serializing_if`, never `null`), not present-with-value
 *  `undefined`, so a fixture built via object-literal (no spread of a
 *  helper that always sets them) is the honest stand-in for the real wire
 *  shape. */
function hostNoPercentiles(over: Partial<HostRollup> = {}): HostRollup {
  return {
    host: 'api.anthropic.com',
    port: 443,
    calls: 10,
    bytes_in: 2048,
    bytes_out: 512,
    errors: 0,
    ...over,
  };
}

describe('NetflowSummary', () => {
  it('renders an em dash for genuinely absent percentiles, without throwing', () => {
    const host = hostNoPercentiles();
    expect('p50_ms' in host).toBe(false);
    expect('p95_ms' in host).toBe(false);

    expect(() => render(<NetflowSummary hosts={[host]} />)).not.toThrow();
    // Both p50 and p95 columns render a dash for this row.
    expect(screen.getAllByText('—').length).toBeGreaterThanOrEqual(2);
    expect(screen.queryByText(/undefined/)).not.toBeInTheDocument();
    expect(screen.queryByText('0 ms')).not.toBeInTheDocument();
  });

  it('renders real percentile values', () => {
    const host = hostNoPercentiles({ p50_ms: 120, p95_ms: 480 });
    render(<NetflowSummary hosts={[host]} />);
    expect(screen.getByText('120 ms')).toBeInTheDocument();
    expect(screen.getByText('480 ms')).toBeInTheDocument();
  });

  it('renders an unknown byte count as a dash, not zero (Coarse host)', () => {
    const host = hostNoPercentiles({ bytes_in: null, p50_ms: 50, p95_ms: 90 });
    render(<NetflowSummary hosts={[host]} />);
    expect(screen.queryByText('0 B')).not.toBeInTheDocument();
    expect(screen.getAllByText('—').length).toBeGreaterThanOrEqual(1);
    // The known fields on the same row still render normally — the dash is
    // specific to the unobservable field, not a table-wide fallback.
    expect(screen.getByText('512 B')).toBeInTheDocument();
    expect(screen.getByText('50 ms')).toBeInTheDocument();
  });

  it('renders nothing for an empty host list', () => {
    const { container } = render(<NetflowSummary hosts={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('does not treat an unobserved p50 as zero when sorting', () => {
    const hosts: HostRollup[] = [
      // Genuinely no timing data at all.
      hostNoPercentiles({ host: 'unknown.example.com' }),
      // A real, fast (zero-millisecond) response — a true zero, distinct
      // from "we never observed a duration".
      hostNoPercentiles({ host: 'zero.example.com', p50_ms: 0, p95_ms: 0 }),
    ];
    render(<NetflowSummary hosts={hosts} />);

    // First click sorts p50 ascending. If "unknown" were coerced to 0 it
    // would tie with the genuine zero and sort order would be unstable /
    // wrong; SortableTable always sorts null/undefined last regardless of
    // direction, so the genuine zero must lead.
    fireEvent.click(screen.getByRole('button', { name: /sort by p50/i }));

    const rows = screen
      .getAllByRole('row')
      .filter((r) => /example\.com/.test(r.textContent ?? ''));
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent('zero.example.com');
    expect(rows[1]).toHaveTextContent('unknown.example.com');

    // Sanity: the genuine zero renders as an honest "0 ms" (both p50 and
    // p95 are 0 for that row), not a dash.
    expect(screen.getAllByText('0 ms').length).toBeGreaterThanOrEqual(1);
  });
});
