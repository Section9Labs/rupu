// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import OrgCards from './OrgCards';
import { lane } from './explorerFixtures';

afterEach(() => {
  cleanup();
});

const LANES = [
  lane(),
  lane({ host: 'cdn.anthropic.com', calls: 2, p95_ms: undefined }),
  lane({ host: 'api.github.com', org: 'GitHub', org_id: 'as36459', asn: 36459, calls: 1 }),
];

describe('OrgCards', () => {
  it('renders one card per org with its endpoints and summed calls', () => {
    render(<OrgCards lanes={LANES} selectedHosts={[]} onToggleHost={() => {}} />);
    expect(screen.getByText('Cloudflare')).toBeInTheDocument();
    expect(screen.getByText('GitHub')).toBeInTheDocument();
    expect(screen.getByText(/AS13335 · 7 calls/)).toBeInTheDocument();
    expect(screen.getByText('api.anthropic.com:443')).toBeInTheDocument();
    expect(screen.getByText('cdn.anthropic.com:443')).toBeInTheDocument();
  });

  it('renders an unobservable p95 as an em dash, never 0 ms', () => {
    render(<OrgCards lanes={LANES} selectedHosts={[]} onToggleHost={() => {}} />);
    expect(screen.getAllByText('—').length).toBeGreaterThan(0);
    expect(screen.queryByText('0 ms')).not.toBeInTheDocument();
  });

  it('a row click toggles that endpoint filter', () => {
    const onToggleHost = vi.fn();
    render(<OrgCards lanes={LANES} selectedHosts={[]} onToggleHost={onToggleHost} />);
    fireEvent.click(screen.getByRole('button', { name: /cdn\.anthropic\.com:443/ }));
    expect(onToggleHost).toHaveBeenCalledWith('cdn.anthropic.com:443');
  });

  it('marks a selected endpoint row', () => {
    render(
      <OrgCards
        lanes={LANES}
        selectedHosts={['api.github.com:443']}
        onToggleHost={() => {}}
      />,
    );
    expect(screen.getByRole('button', { name: /api\.github\.com:443/ })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  });

  it('renders nothing at all with no lanes', () => {
    const { container } = render(<OrgCards lanes={[]} selectedHosts={[]} onToggleHost={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });
});
