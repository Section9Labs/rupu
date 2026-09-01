// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import TopologyView from './TopologyView';
import { populatedExplorerResponse } from './explorerFixtures';

afterEach(() => {
  cleanup();
});

const NO_SELECTION = { wf: [], or: [], org: [] };

// `useThemeColors` reads CSS custom properties straight off
// `document.documentElement` and tolerates the missing ThemeProvider in
// jsdom (same as RunGraph.edges.test.tsx) — no wrapper needed.
function renderView(props: Partial<React.ComponentProps<typeof TopologyView>> = {}) {
  return render(
    <TopologyView
      sankey={populatedExplorerResponse().sankey}
      selected={NO_SELECTION}
      onToggle={() => {}}
      scope="global"
      {...props}
    />,
  );
}

describe('TopologyView', () => {
  it('renders the three columns with their nodes and counts', () => {
    renderView();
    expect(screen.getByText('Workflows')).toBeInTheDocument();
    expect(screen.getByText('Origins')).toBeInTheDocument();
    expect(screen.getByText('Networks')).toBeInTheDocument();
    expect(screen.getByText('review-wf')).toBeInTheDocument();
    expect(screen.getByText('provider:anthropic')).toBeInTheDocument();
    expect(screen.getByText('Cloudflare')).toBeInTheDocument();
  });

  it('clicking a node toggles its dimension filter', () => {
    const onToggle = vi.fn();
    renderView({ onToggle });
    fireEvent.click(screen.getByRole('button', { name: /review-wf/ }));
    expect(onToggle).toHaveBeenCalledWith('wf', 'review-wf');

    fireEvent.click(screen.getByRole('button', { name: /Cloudflare/ }));
    expect(onToggle).toHaveBeenCalledWith('org', 'as13335');
  });

  it('renders a zero-call node dimmed — present, never removed', () => {
    // The `unknown` workflow node has calls: 0 in the fixture (filtered /
    // windowed out but still in scope).
    renderView();
    const dimmedNode = screen.getByRole('button', { name: /unknown/ });
    expect(dimmedNode).toBeInTheDocument();
    expect(dimmedNode.className).toContain('opacity-35');
    // Zero renders as a middot placeholder, not a numeric claim of "0".
    expect(dimmedNode).toHaveTextContent('·');
  });

  it('marks a selected node with aria-pressed', () => {
    renderView({ selected: { wf: ['review-wf'], or: [], org: [] } });
    expect(screen.getByRole('button', { name: /review-wf/ })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  });

  it('words the empty state by the server window echo', () => {
    const empty = { workflows: [], origins: [], orgs: [], wf_origin: [], origin_org: [] };
    const { unmount } = renderView({
      sankey: empty,
      appliedWindow: { from: '2026-08-01T00:00:00Z', to: null },
    });
    expect(screen.getByText(/no flows to graph in this range/i)).toBeInTheDocument();
    unmount();

    renderView({ sankey: empty, appliedWindow: { from: null, to: null } });
    expect(screen.getByText(/no flows to graph for this scope/i)).toBeInTheDocument();
  });
});
