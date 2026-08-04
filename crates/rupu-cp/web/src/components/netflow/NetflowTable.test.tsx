// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup } from '@testing-library/react';
import NetflowTable from './NetflowTable';
import type { FlowView } from '../../lib/netflow';

afterEach(() => {
  cleanup();
});

function flow(over: Partial<FlowView> = {}): FlowView {
  return {
    id: '01J',
    ts: '2026-08-03T00:00:00Z',
    ctx: { origin: { kind: 'provider', name: 'anthropic' }, run_id: 'r1' },
    fidelity: 'http',
    method: 'POST',
    scheme: 'https',
    host: 'api.anthropic.com',
    port: 443,
    path: '/v1/messages',
    outcome: 'ok',
    body_complete: true,
    bytes_in: 2048,
    bytes_out: 512,
    duration_ms: 30,
    asn: { asn: 13335, org: 'CLOUDFLARENET' },
    ...over,
  };
}

describe('NetflowTable', () => {
  it('renders host, path and ASN org', () => {
    render(<NetflowTable flows={[flow()]} dropped={0} asnLoaded />);
    expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();
    expect(screen.getByText('/v1/messages')).toBeInTheDocument();
    expect(screen.getByText(/CLOUDFLARENET/)).toBeInTheDocument();
    expect(screen.getByText(/AS13335/)).toBeInTheDocument();
  });

  it('always shows the fidelity badge', () => {
    render(<NetflowTable flows={[flow({ fidelity: 'coarse' })]} dropped={0} asnLoaded />);
    expect(screen.getByText('coarse')).toBeInTheDocument();
  });

  it('renders an unknown byte count as a dash, not zero', () => {
    render(
      <NetflowTable
        flows={[flow({ fidelity: 'coarse', bytes_in: null, bytes_out: null })]}
        dropped={0}
        asnLoaded
      />,
    );
    expect(screen.queryByText('0 B')).not.toBeInTheDocument();
    expect(screen.getAllByText('—').length).toBeGreaterThan(0);
  });

  it('surfaces dropped flows rather than under-reporting silently', () => {
    render(<NetflowTable flows={[flow()]} dropped={17} asnLoaded />);
    expect(screen.getByText(/17 flows dropped/i)).toBeInTheDocument();
  });

  it('explains a missing ASN table instead of leaving a blank column', () => {
    // `FlowView.asn` is `AsnInfo | undefined` — the Rust side
    // (`#[serde(skip_serializing_if = "Option::is_none")]`) OMITS the key
    // rather than serializing `null`, so `undefined` is the correct "no
    // ASN" value here, not `null`.
    render(<NetflowTable flows={[flow({ asn: undefined })]} dropped={0} asnLoaded={false} />);
    expect(screen.getByText(/ASN data not loaded/i)).toBeInTheDocument();
  });

  it('states the phase-1 scope limit on the empty state', () => {
    render(<NetflowTable flows={[]} dropped={0} asnLoaded />);
    expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument();
    expect(screen.getByText(/does not cover.*subprocess/i)).toBeInTheDocument();
  });
});
