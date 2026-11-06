// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import TokenFlow from './TokenFlow';
import { ThemeProvider } from '../theme/ThemeProvider';
import type { UsageTimelinePoint } from '../../lib/api';

afterEach(cleanup);

const pts: UsageTimelinePoint[] = [
  ...Array.from({ length: 12 }, (_, i) => ({ turn: i + 1, label: 'assess', tokens_in: 1000 + i * 100, tokens_out: 50, tokens_cached: 400 })),
  ...Array.from({ length: 8 }, (_, i) => ({ turn: i + 1, label: 'verify', tokens_in: 2000, tokens_out: 80, tokens_cached: 0 })),
];
const wrap = (ui: React.ReactNode) => render(<ThemeProvider>{ui}</ThemeProvider>);

describe('TokenFlow', () => {
  it('names the flow in its header and on the plot', () => {
    wrap(<TokenFlow points={pts} />);
    expect(screen.getByText('Token flow')).toBeInTheDocument();
    expect(screen.getByText(/20 turns across 2 stages/)).toBeInTheDocument();
    expect(screen.getByRole('img', { name: /20 turns, 34\.6k in, 1\.2k out/ })).toBeInTheDocument();
    // cache reads are part of the context: 12 × 400 = 4.8k of 34.6k
    expect(screen.getByText(/14% from cache/)).toBeInTheDocument();
  });

  it('lists the stages with their share of the context', () => {
    wrap(<TokenFlow points={pts} />);
    // assess: 12 turns, 1000..2100 → 18.6k of 34.6k = 54%
    expect(screen.getByRole('button', { name: /assess.*18\.6k.*54%/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /verify.*16k.*46%/ })).toBeInTheDocument();
  });

  it('zooms to a stage when it is clicked, and resets', () => {
    wrap(<TokenFlow points={pts} />);
    fireEvent.click(screen.getByRole('button', { name: /verify.*16k/ }));
    expect(screen.getByText(/Turns 13–20/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Reset/ }));
    expect(screen.queryByText(/Turns 13–20/)).toBeNull();
  });

  it('marks a live flow and breathes its end; a finished one does not', () => {
    const { container } = wrap(<TokenFlow points={pts} live />);
    expect(screen.getByText('live')).toBeInTheDocument();
    expect(container.querySelectorAll('.cp-chart-ring').length).toBe(2);
    cleanup();
    const done = wrap(<TokenFlow points={pts} />);
    expect(done.container.querySelector('.cp-chart-ring')).toBeNull();
  });

  it('shows the empty state with no turns', () => {
    wrap(<TokenFlow points={[]} />);
    expect(screen.getByText(/No per-turn usage yet/)).toBeInTheDocument();
  });

  it('measures its width even when it mounts before any turns arrive', () => {
    // the run page renders the chart empty, then hands it the turns
    const rect = vi
      .spyOn(HTMLElement.prototype, 'getBoundingClientRect')
      .mockReturnValue({ width: 1000, height: 300, top: 0, left: 0, right: 1000, bottom: 300, x: 0, y: 0, toJSON: () => ({}) } as DOMRect);
    try {
      const { container, rerender } = wrap(<TokenFlow points={[]} />);
      rerender(
        <ThemeProvider>
          <TokenFlow points={pts} />
        </ThemeProvider>,
      );
      const svg = container.querySelector('svg[role="img"]') as SVGSVGElement;
      expect(svg.getAttribute('viewBox')?.startsWith('0 0 1000 ')).toBe(true);
    } finally {
      rect.mockRestore();
    }
  });

  it('drops a zoom the data no longer covers', () => {
    const { rerender } = wrap(<TokenFlow points={pts} />);
    fireEvent.click(screen.getByRole('button', { name: /verify.*16k/ }));
    expect(screen.getByText(/Turns 13–20/)).toBeInTheDocument();
    rerender(
      <ThemeProvider>
        <TokenFlow points={pts.slice(0, 6)} />
      </ThemeProvider>,
    );
    expect(screen.queryByText(/Turns 13–20/)).toBeNull();
    expect(screen.queryByRole('button', { name: /Reset/ })).toBeNull();
  });
});
