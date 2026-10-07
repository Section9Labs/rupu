// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { HostsWithoutCustomerBanner, hostsLeftOut } from './HostsWithoutCustomerBanner';

afterEach(cleanup);

const LIST_501 = "host host_mini can't report a customer for every run";
const AGG_501 = "host host_prod can't be filtered by customer: its totals are summed remotely";

describe('HostsWithoutCustomerBanner', () => {
  it('renders nothing when no host is left out', () => {
    const { container } = render(
      <HostsWithoutCustomerBanner
        hosts={[
          { id: 'local', name: 'Local', state: 'ok', reason: null },
          { id: 'host_prod', name: 'prod', state: 'offline', reason: 'connection refused' },
          // unavailable for an unrelated reason (an old rupu that can't list at all)
          { id: 'host_old', name: 'old', state: 'unavailable', reason: 'needs rupu >= 0.49' },
        ]}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('names a slice unavailable with the list 501 reason', () => {
    render(<HostsWithoutCustomerBanner hosts={[{ id: 'host_mini', name: 'mini', state: 'unavailable', reason: LIST_501 }]} />);
    const banner = screen.getByRole('status');
    expect(banner).toHaveTextContent(
      'mini runs an older rupu or holds runs from before customers, so it can’t tag every run with a customer — those runs are left out of this view, not counted as zero.',
    );
  });

  it('names header / array host ids by their slice name, else by id, deduplicated', () => {
    render(
      <HostsWithoutCustomerBanner
        hosts={[
          { id: 'host_mini', name: 'mini', state: 'unavailable', reason: LIST_501 },
          { id: 'host_kuki', name: 'kuki', state: 'ok', reason: null },
        ]}
        without={['host_kuki', 'host_mini', 'worker-7']}
      />,
    );
    expect(screen.getByRole('status')).toHaveTextContent(
      'kuki, mini and worker-7 run an older rupu or hold runs from before customers, so they can’t tag every run',
    );
  });

  it('says why an aggregate host is left out', () => {
    render(<HostsWithoutCustomerBanner hosts={[{ id: 'host_prod', name: 'prod', state: 'unavailable', reason: AGG_501 }]} />);
    expect(screen.getByRole('status')).toHaveTextContent(
      'prod can’t be filtered by customer (its totals are summed on the host) — left out of this view, not counted as zero.',
    );
  });

  it('matches the reason inside a raw JSON error body too', () => {
    expect(
      hostsLeftOut([{ name: 'mini', state: 'unavailable', reason: JSON.stringify({ error: LIST_501 }) }], []),
    ).toEqual({ unreportable: ['mini'], aggregate: [] });
  });
});
