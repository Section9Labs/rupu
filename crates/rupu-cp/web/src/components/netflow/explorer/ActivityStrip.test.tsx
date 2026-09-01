// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ActivityStrip from './ActivityStrip';
import { emptyHistogram, populatedExplorerResponse } from './explorerFixtures';

afterEach(() => {
  cleanup();
});

const HISTOGRAM = populatedExplorerResponse().histogram;

function renderStrip(props: Partial<React.ComponentProps<typeof ActivityStrip>> = {}) {
  return render(
    <ActivityStrip
      histogram={HISTOGRAM}
      range={{ preset: 'all' }}
      onRangeChange={() => {}}
      onZoom={() => {}}
      appliedWindow={{ from: null, to: null }}
      {...props}
    />,
  );
}

/** jsdom gives every element a 0×0 rect; the drag math needs a real one. */
function mockChartRect() {
  const chart = screen.getByRole('img', { name: /recorded flows/i });
  vi.spyOn(chart, 'getBoundingClientRect').mockReturnValue({
    left: 0,
    width: 700,
    top: 0,
    height: 64,
    right: 700,
    bottom: 64,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect);
  chart.setPointerCapture = vi.fn();
  return chart;
}

describe('ActivityStrip', () => {
  it('renders the presets, the server-echo readout, and the histogram bounds', () => {
    renderStrip();
    expect(screen.getByRole('button', { name: 'All' })).toBeInTheDocument();
    expect(screen.getByText(/showing all recorded flows/i)).toBeInTheDocument();
    expect(screen.getByText(/drag on the chart to zoom/i)).toBeInTheDocument();
  });

  it('reads the readout from the SERVER echo, not the picker state', () => {
    // Picker says "all" but the echo carries a bound — the echo wins.
    renderStrip({ appliedWindow: { from: '2026-08-01T00:00:00Z', to: null } });
    expect(screen.getByText(/showing flows from/i)).toBeInTheDocument();
    expect(screen.queryByText(/showing all recorded flows/i)).not.toBeInTheDocument();
  });

  it('commits a drag wider than one bucket as a custom zoom window', () => {
    const onZoom = vi.fn();
    renderStrip({ onZoom });
    const chart = mockChartRect();

    fireEvent.pointerDown(chart, { clientX: 100, pointerId: 1 });
    fireEvent.pointerMove(chart, { clientX: 400, pointerId: 1 });
    fireEvent.pointerUp(chart, { pointerId: 1 });

    expect(onZoom).toHaveBeenCalledTimes(1);
    const [from, to] = onZoom.mock.calls[0];
    expect(Date.parse(from)).toBeLessThan(Date.parse(to));
  });

  it('discards a click / sub-bucket micro-drag instead of committing a sliver window', () => {
    const onZoom = vi.fn();
    renderStrip({ onZoom });
    const chart = mockChartRect();

    fireEvent.pointerDown(chart, { clientX: 100, pointerId: 1 });
    fireEvent.pointerUp(chart, { pointerId: 1 });

    expect(onZoom).not.toHaveBeenCalled();
  });

  it('disables dragging entirely when the scope has no flows (null histogram bounds)', () => {
    const onZoom = vi.fn();
    renderStrip({ histogram: emptyHistogram(), onZoom });
    const chart = screen.getByRole('img', { name: /recorded flows/i });
    chart.setPointerCapture = vi.fn();

    fireEvent.pointerDown(chart, { clientX: 100, pointerId: 1 });
    fireEvent.pointerUp(chart, { pointerId: 1 });

    expect(onZoom).not.toHaveBeenCalled();
    expect(screen.queryByText(/drag on the chart/i)).not.toBeInTheDocument();
  });
});
