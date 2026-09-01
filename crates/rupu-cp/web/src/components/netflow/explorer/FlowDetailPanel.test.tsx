// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import FlowDetailPanel from './FlowDetailPanel';
import { flowView } from './explorerFixtures';

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
});
