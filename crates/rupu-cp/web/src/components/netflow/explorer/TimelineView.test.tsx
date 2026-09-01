// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import TimelineView from './TimelineView';
import { emptyTimeline, lane, bucket } from './explorerFixtures';

afterEach(() => {
  cleanup();
});

function renderView(props: Partial<React.ComponentProps<typeof TimelineView>> = {}) {
  return render(
    <TimelineView
      timeline={{
        ...emptyTimeline(),
        lanes: [
          lane(),
          lane({ host: 'api.github.com', org: 'GitHub', org_id: 'as36459', asn: 36459, calls: 1, fidelity: 'coarse' }),
        ],
        runs: [1, 2, 0, 0],
        runs_in_window: 2,
      }}
      selectedHosts={[]}
      onToggleHost={() => {}}
      onZoomBucket={() => {}}
      canZoomOut={false}
      onZoomOut={() => {}}
      {...props}
    />,
  );
}

describe('TimelineView', () => {
  it('renders one lane per endpoint with per-lane stats and org group headers', () => {
    renderView();
    expect(screen.getByText('api.anthropic.com:443')).toBeInTheDocument();
    expect(screen.getByText('api.github.com:443')).toBeInTheDocument();
    expect(screen.getByText(/Cloudflare · AS13335/)).toBeInTheDocument();
    expect(screen.getByText(/GitHub · AS36459/)).toBeInTheDocument();
    expect(screen.getByText('5 calls')).toBeInTheDocument();
    expect(screen.getAllByText('120 ms').length).toBeGreaterThan(0);
    expect(screen.getByText('2 runs in window')).toBeInTheDocument();
  });

  it('tags a coarse lane with the fidelity badge', () => {
    renderView();
    expect(screen.getByText('coarse')).toBeInTheDocument();
  });

  it('clicking an endpoint label toggles its host filter (by host:port key)', () => {
    const onToggleHost = vi.fn();
    renderView({ onToggleHost });
    fireEvent.click(screen.getByRole('button', { name: /api\.github\.com:443/ }));
    expect(onToggleHost).toHaveBeenCalledWith('api.github.com:443');
  });

  it('clicking a populated cell zooms the window to that bucket; an empty cell does nothing', () => {
    const onZoomBucket = vi.fn();
    renderView({ onZoomBucket });
    // Cells carry a `N calls` tooltip. Fixture lane() has buckets
    // [2, 3(1 err), 0, 0] over 00:00–01:00.
    const populated = screen.getAllByTitle(/^2 calls/)[0];
    fireEvent.click(populated);
    expect(onZoomBucket).toHaveBeenCalledTimes(1);
    const [from, to] = onZoomBucket.mock.calls[0];
    expect(Date.parse(from)).toBeLessThan(Date.parse(to));

    const empty = screen.getAllByTitle(/^0 calls/)[0];
    fireEvent.click(empty);
    expect(onZoomBucket).toHaveBeenCalledTimes(1);
  });

  it('offers Zoom out only when the zoom stack is non-empty', () => {
    const onZoomOut = vi.fn();
    const { unmount } = renderView({ canZoomOut: true, onZoomOut });
    fireEvent.click(screen.getByRole('button', { name: /zoom out/i }));
    expect(onZoomOut).toHaveBeenCalled();
    unmount();

    renderView({ canZoomOut: false });
    expect(screen.queryByRole('button', { name: /zoom out/i })).not.toBeInTheDocument();
  });

  it('marks error buckets in the tooltip and lane stats', () => {
    renderView();
    expect(screen.getAllByTitle(/3 calls · 1 errors/).length).toBeGreaterThan(0);
  });

  it('words the empty state by the server window echo', () => {
    const { unmount } = renderView({
      timeline: { ...emptyTimeline(), lanes: [] },
      appliedWindow: { from: '2026-08-01T00:00:00Z', to: null },
    });
    expect(screen.getByText(/no endpoint activity in this range/i)).toBeInTheDocument();
    unmount();

    renderView({
      timeline: { ...emptyTimeline(), lanes: [] },
      appliedWindow: { from: null, to: null },
    });
    expect(screen.getByText(/no endpoint activity for this scope/i)).toBeInTheDocument();
  });

  it('renders dense buckets — zeros included — and never scales cells by bytes', () => {
    renderView({
      timeline: {
        ...emptyTimeline(),
        lanes: [lane({ buckets: [bucket(0), bucket(0), bucket(0), bucket(4)] })],
      },
    });
    // All four cells render (dense), three of them with the empty tooltip.
    expect(screen.getAllByTitle(/^0 calls/).length).toBe(3);
    expect(screen.getAllByTitle(/^4 calls/).length).toBe(1);
  });
});
