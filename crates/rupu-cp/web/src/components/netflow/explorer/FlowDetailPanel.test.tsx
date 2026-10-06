// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import FlowDetailPanel from './FlowDetailPanel';
import { flowView, socketFlowView } from './explorerFixtures';

afterEach(() => {
  cleanup();
});

describe('FlowDetailPanel', () => {
  it('renders nothing when no flow is selected', () => {
    const { container } = render(<FlowDetailPanel flow={null} scope="global" onClose={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the full record with the fidelity badge and its explanation line', () => {
    render(<FlowDetailPanel flow={flowView()} scope="global" onClose={() => {}} />);
    expect(screen.getByRole('dialog', { name: /flow detail/i })).toBeInTheDocument();
    expect(screen.getByText('api.anthropic.com:443')).toBeInTheDocument();
    expect(screen.getByText(/POST \/v1\/messages/)).toBeInTheDocument();
    // Fidelity moved here from the old table column: badge + FIDELITY_TITLE line.
    expect(screen.getByText('http')).toBeInTheDocument();
    expect(screen.getByText(/exact request and response metadata/i)).toBeInTheDocument();
    expect(screen.getByText('2.0 KB')).toBeInTheDocument();
    expect(screen.getByText(/observed directly by the instrumented/i)).toBeInTheDocument();
  });

  it('renders coarse unknowns as em dashes with the "not observable, never zero" note', () => {
    render(
      <FlowDetailPanel
        flow={flowView({
          fidelity: 'coarse',
          bytes_in: undefined,
          bytes_out: undefined,
          peer_ip: undefined,
          asn: undefined,
          ttfb_ms: undefined,
        })}
        scope="global"
        onClose={() => {}}
      />,
    );
    expect(screen.queryByText('0 B')).not.toBeInTheDocument();
    expect(screen.getAllByText('—').length).toBeGreaterThan(2);
    expect(screen.getByText(/"not observable", never zero/i)).toBeInTheDocument();
  });

  it('shows Run/Workflow attribution rows at non-run scopes only', () => {
    const attributed = flowView({ run_id: 'run-9', workflow: 'review-wf' });
    const { unmount } = render(
      <FlowDetailPanel flow={attributed} scope="global" onClose={() => {}} />,
    );
    expect(screen.getByText('run-9')).toBeInTheDocument();
    expect(screen.getByText('review-wf')).toBeInTheDocument();
    unmount();

    render(<FlowDetailPanel flow={attributed} scope="run" onClose={() => {}} />);
    expect(screen.queryByText('run-9')).not.toBeInTheDocument();
  });

  it('closes via the close button', () => {
    const onClose = vi.fn();
    render(<FlowDetailPanel flow={flowView()} scope="global" onClose={onClose} />);
    fireEvent.click(screen.getByRole('button', { name: /close flow detail/i }));
    expect(onClose).toHaveBeenCalled();
  });

  it('shows the socket field set and none of the http-only rows for a socket flow', () => {
    render(<FlowDetailPanel flow={socketFlowView()} scope="global" onClose={() => {}} />);
    expect(screen.getByText('140.82.116.3:443')).toBeInTheDocument();
    expect(screen.getByText('curl (pid 4412)')).toBeInTheDocument();
    expect(screen.getByText('10.0.0.2:51000')).toBeInTheDocument();
    expect(screen.getByText('outbound')).toBeInTheDocument();
    expect(screen.getByText('4.0 KB')).toBeInTheDocument();
    expect(screen.getByText('128 B')).toBeInTheDocument();
    expect(screen.queryByText('Status')).not.toBeInTheDocument();
    expect(screen.queryByText('TTFB')).not.toBeInTheDocument();
    expect(screen.queryByText('Peer IP')).not.toBeInTheDocument();
    expect(screen.queryByText(/GET|POST/)).not.toBeInTheDocument();
  });

  it('keeps the http rows for an http flow', () => {
    render(<FlowDetailPanel flow={flowView({ ttfb_ms: 12 })} scope="global" onClose={() => {}} />);
    expect(screen.getByText('Status')).toBeInTheDocument();
    expect(screen.getByText('TTFB')).toBeInTheDocument();
    expect(screen.queryByText('Local address')).not.toBeInTheDocument();
    expect(screen.queryByText('Process')).not.toBeInTheDocument();
  });
});
