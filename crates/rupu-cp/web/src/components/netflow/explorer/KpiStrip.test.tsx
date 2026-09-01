// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import KpiStrip from './KpiStrip';
import { emptyKpis } from './explorerFixtures';

afterEach(() => {
  cleanup();
});

describe('KpiStrip', () => {
  it('renders the six server-computed KPIs', () => {
    render(
      <KpiStrip
        kpis={{
          flows: 42,
          endpoints: 3,
          orgs: 2,
          errors: 0,
          bytes_in: 4096,
          bytes_out: 512,
          bytes_partial: false,
          p95_ms: 120,
        }}
      />,
    );
    expect(screen.getByText('Flows')).toBeInTheDocument();
    expect(screen.getByText('42')).toBeInTheDocument();
    expect(screen.getByText('4.0 KB')).toBeInTheDocument();
    expect(screen.getByText('120')).toBeInTheDocument();
  });

  it('marks byte sums with † and the observed-only tooltip when partial', () => {
    render(
      <KpiStrip
        kpis={{ ...emptyKpis(), flows: 2, bytes_in: 100, bytes_partial: true }}
      />,
    );
    expect(screen.getByText('†')).toBeInTheDocument();
    expect(screen.getByTitle(/observed \(HTTP-fidelity\) flows only/i)).toBeInTheDocument();
  });

  it('renders null byte sums as an em dash, never 0 B', () => {
    // `bytes_in: null` = NO flow contributed an observed value (e.g. an
    // all-coarse window) — `0 B` would claim we saw zero bytes.
    render(<KpiStrip kpis={{ ...emptyKpis(), flows: 2, bytes_partial: true }} />);
    expect(screen.queryByText('0 B')).not.toBeInTheDocument();
    expect(screen.getAllByText('—').length).toBeGreaterThan(0);
  });

  it('renders an absent p95 as an em dash, never 0 ms', () => {
    render(<KpiStrip kpis={{ ...emptyKpis(), flows: 1 }} />);
    expect(screen.queryByText(/0 ms/)).not.toBeInTheDocument();
  });

  it('shows an em-dash error rate on an empty set instead of a fake 0%', () => {
    render(<KpiStrip kpis={emptyKpis()} />);
    expect(screen.getByText('Error rate')).toBeInTheDocument();
    expect(screen.queryByText('0.0')).not.toBeInTheDocument();
  });
});
