// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it } from 'vitest';
import { FleetStrip } from './FleetStrip';
import type { FleetCounts } from '../../lib/api';

function fleet(over: Partial<FleetCounts> = {}): FleetCounts {
  return {
    repos: null,
    providers_configured: null,
    providers_unhealthy: null,
    autoflows_enabled: null,
    autoflows_disabled: null,
    workers: null,
    claims_active: null,
    issues_pending: null,
    issues_open: null,
    issues_capped: false,
    inventory_captured_at: null,
    ...over,
  };
}

function renderStrip(f: FleetCounts, partial = false) {
  return render(
    <MemoryRouter>
      <FleetStrip fleet={f} fleetPartial={partial} />
    </MemoryRouter>,
  );
}

afterEach(cleanup);

describe('FleetStrip', () => {
  it('renders an em-dash for a null count, never a zero', () => {
    renderStrip(fleet({ workers: null }));
    expect(screen.getByTestId('fleet-workers')).toHaveTextContent('—');
    expect(screen.getByTestId('fleet-workers')).not.toHaveTextContent('0');
  });

  it('renders a genuine zero as 0', () => {
    renderStrip(fleet({ workers: 0 }));
    expect(screen.getByTestId('fleet-workers')).toHaveTextContent('0 workers');
  });

  it('shows the disabled autoflow count only when some are disabled', () => {
    const { rerender } = renderStrip(fleet({ autoflows_enabled: 6, autoflows_disabled: 2 }));
    expect(screen.getByTestId('fleet-autoflows')).toHaveTextContent('2 off');

    rerender(
      <MemoryRouter>
        <FleetStrip
          fleet={fleet({ autoflows_enabled: 6, autoflows_disabled: 0 })}
          fleetPartial={false}
        />
      </MemoryRouter>,
    );
    expect(screen.getByTestId('fleet-autoflows')).not.toHaveTextContent('off');
  });

  it('suffixes the open issue count with + when capped', () => {
    renderStrip(fleet({ issues_pending: 14, issues_open: 312, issues_capped: true }));
    expect(screen.getByTestId('fleet-issues')).toHaveTextContent('312+');
  });

  it('does not suffix the open issue count when not capped', () => {
    renderStrip(fleet({ issues_pending: 14, issues_open: 312, issues_capped: false }));
    expect(screen.getByTestId('fleet-issues')).toHaveTextContent('312 open');
    expect(screen.getByTestId('fleet-issues')).not.toHaveTextContent('312+');
  });

  it('weights the providers segment only when some are unhealthy', () => {
    const { rerender } = renderStrip(fleet({ providers_configured: 4, providers_unhealthy: 1 }));
    expect(screen.getByTestId('fleet-providers')).toHaveAttribute('data-fault', 'true');

    rerender(
      <MemoryRouter>
        <FleetStrip
          fleet={fleet({ providers_configured: 4, providers_unhealthy: 0 })}
          fleetPartial={false}
        />
      </MemoryRouter>,
    );
    expect(screen.getByTestId('fleet-providers')).toHaveAttribute('data-fault', 'false');
  });

  it('claims nothing about health when providers_unhealthy is null', () => {
    renderStrip(fleet({ providers_configured: 4, providers_unhealthy: null }));
    const seg = screen.getByTestId('fleet-providers');
    expect(seg).toHaveTextContent('4 providers');
    expect(seg).toHaveAttribute('data-fault', 'false');
    expect(seg).not.toHaveTextContent('unhealthy');
  });

  it('marks itself partial when a reporting host omitted a count', () => {
    renderStrip(fleet({ workers: 3 }), true);
    expect(screen.getByTestId('fleet-strip')).toHaveTextContent('partial');
  });

  it('does not mark itself partial when every host reported', () => {
    renderStrip(fleet({ workers: 3 }), false);
    expect(screen.getByTestId('fleet-strip')).not.toHaveTextContent('partial');
  });
});
