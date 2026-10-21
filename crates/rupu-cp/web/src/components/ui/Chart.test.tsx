// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { Chart, placeRefLabels, plotFor, spacedTicks, tooltipModel, type ChartSeries } from './Chart';
import { ThemeProvider } from '../theme/ThemeProvider';

afterEach(cleanup);

const dates = ['Sep 20', 'Sep 26', 'Oct 4', 'Oct 18'];
const series: ChartSeries[] = [
  { key: 'completed', label: 'Completed', values: [6, 22, 198, 9], color: 'status.completed' },
  { key: 'failed', label: 'Failed', values: [1, 3, 16, 2], color: 'status.failed' },
];

const wrap = (ui: React.ReactNode) => render(<ThemeProvider>{ui}</ThemeProvider>);

describe('Chart', () => {
  it('renders a stacked area + line per series', () => {
    const { container } = wrap(<Chart dates={dates} series={series} title="Outcomes over time" />);
    // one polyline per series (the stacked line tops)
    expect(container.querySelectorAll('polyline').length).toBe(2);
    // one filled area path per series, inside the reveal-clipped group
    expect(container.querySelectorAll('[clip-path] path').length).toBe(2);
  });

  it('labels the title, legend and peak total', () => {
    wrap(
      <Chart
        dates={dates}
        series={series}
        title="Outcomes over time"
        peakSubLabel={(_, t) => `${t} runs`}
      />,
    );
    expect(screen.getByText('Outcomes over time')).toBeInTheDocument();
    expect(screen.getByText('Completed')).toBeInTheDocument();
    expect(screen.getByText('Failed')).toBeInTheDocument();
    // peak total = 198 + 16 = 214 at Oct 4
    expect(screen.getByText('214')).toBeInTheDocument();
    expect(screen.getByText('214 runs')).toBeInTheDocument();
  });

  it('draws the now-pulse ring (reduced-motion hides it via CSS, not the DOM)', () => {
    const { container } = wrap(<Chart dates={dates} series={series} />);
    expect(container.querySelector('.cp-chart-now')).not.toBeNull();
  });

  it('shows y-tick labels and an exposed role=img plot', () => {
    wrap(<Chart dates={dates} series={series} ariaLabel="outcomes" />);
    expect(screen.getByRole('img', { name: 'outcomes' })).toBeInTheDocument();
    // the baseline 0 tick is always present
    expect(screen.getAllByText('0').length).toBeGreaterThan(0);
  });

  it('renders an empty (no-data) chart without crashing', () => {
    const { container } = wrap(<Chart dates={[]} series={[]} title="Empty" />);
    expect(screen.getByText('Empty')).toBeInTheDocument();
    expect(container.querySelectorAll('polyline').length).toBe(0);
  });

  it('tooltipModel stacks per-series values and the total for a bucket', () => {
    const m = tooltipModel(series, dates, 2);
    expect(m.date).toBe('Oct 4');
    expect(m.rows.map((r) => r.value)).toEqual([198, 16]);
    expect(m.total).toBe(214);
  });

  it('kind="line" draws one overlaid line per series and no area fills', () => {
    const { container } = wrap(<Chart dates={dates} series={series} kind="line" />);
    expect(container.querySelectorAll('polyline').length).toBe(2);
    // overlaid lines aren't clipped area paths
    expect(container.querySelectorAll('[clip-path] path').length).toBe(0);
    // the now-pulse still marks the latest point
    expect(container.querySelector('.cp-chart-now')).not.toBeNull();
  });

  it('kind="column" draws stacked columns — no lines, no now-pulse', () => {
    const { container } = wrap(<Chart dates={dates} series={series} kind="column" />);
    expect(container.querySelectorAll('polyline').length).toBe(0);
    // each column's cap is a rounded-top path inside the reveal clip
    expect(container.querySelectorAll('[clip-path] path').length).toBeGreaterThan(0);
    // columns don't carry the breathing now-dot (the rightmost bar is "now")
    expect(container.querySelector('.cp-chart-now')).toBeNull();
  });

  it('yFormat formats the y-tick labels', () => {
    wrap(<Chart dates={dates} series={series} yFormat={(v) => `$${v}`} />);
    // the baseline 0 tick is always present, now dollar-formatted
    expect(screen.getByText('$0')).toBeInTheDocument();
  });

  it('rawColor paints the legend swatch with the raw CSS color (dynamic palette)', () => {
    const dyn: ChartSeries[] = [
      { key: 'alpha', label: 'alpha', values: [1, 2, 3, 4], rawColor: 'rgb(10, 20, 30)' },
      { key: 'beta', label: 'beta', values: [2, 3, 4, 5], rawColor: 'rgb(40, 50, 60)' },
    ];
    const { container } = wrap(<Chart dates={dates} series={dyn} />);
    const swatches = Array.from(container.querySelectorAll('.cp-chart-legend-swatch, [aria-hidden]')).filter(
      (el) => (el as HTMLElement).style.background,
    );
    const backgrounds = swatches.map((el) => (el as HTMLElement).style.background);
    expect(backgrounds).toContain('rgb(10, 20, 30)');
    expect(backgrounds).toContain('rgb(40, 50, 60)');
    // still a stacked area (one filled path per series inside the reveal clip)
    expect(container.querySelectorAll('[clip-path] path').length).toBe(2);
  });

  it('showLegend={false} / showCard={false} drop the chrome but keep the plot', () => {
    const { container } = wrap(
      <Chart dates={dates} series={series} showLegend={false} showCard={false} ariaLabel="bare" />,
    );
    // no legend swatches in the header area; the plot is still there
    expect(screen.getByRole('img', { name: 'bare' })).toBeInTheDocument();
    expect(container.querySelector('.rounded-xl.border')).toBeNull();
  });

  it('kind="composed" overlays an area + two lines on two axes, unstacked (no Total, no peak)', () => {
    const composed: ChartSeries[] = [
      { key: 'in', label: 'In', values: [800, 600, 400, 900], color: 'status.running', mark: 'area', axis: 'left' },
      { key: 'out', label: 'Out', values: [120, 90, 200, 110], color: 'status.done', mark: 'line', axis: 'right' },
      { key: 'cached', label: 'Cached', values: [40, 50, 30, 20], color: 'status.awaiting', mark: 'line', axis: 'right' },
    ];
    const { container } = wrap(
      <Chart dates={dates} series={composed} kind="composed" yFormat={(v) => `${v}`} yFormatRight={(v) => `${v}`} />,
    );
    // in → area (1 clipped path) + its top line; out/cached → 2 lines ⇒ 3 polylines
    expect(container.querySelectorAll('[clip-path] path').length).toBe(1);
    expect(container.querySelectorAll('polyline').length).toBe(3);
    // overlaid, not stacked: the now-pulse still marks the latest point
    expect(container.querySelector('.cp-chart-now')).not.toBeNull();
  });

  it('refLines draw a dashed vertical with its label', () => {
    const composed: ChartSeries[] = [
      { key: 'in', label: 'In', values: [800, 600, 400, 900], color: 'status.running', mark: 'area', axis: 'left' },
    ];
    wrap(<Chart dates={dates} series={composed} kind="composed" refLines={[{ index: 2, label: 'step-b' }]} />);
    expect(screen.getByText('step-b')).toBeInTheDocument();
  });

  it('onSelectRange renders no band at rest and marks the svg selectable', () => {
    const { container } = wrap(<Chart dates={dates} series={series} onSelectRange={() => {}} />);
    expect(container.querySelector('[data-chart-band]')).toBeNull();
    const svg = container.querySelector('svg');
    expect(svg?.getAttribute('style') ?? '').toContain('crosshair');
  });
});

describe('chart layout at true pixel size', () => {
  const plot = plotFor(1000, 220, false);
  const xs = (n: number) => Array.from({ length: n }, (_, i) => plot.x0 + ((plot.x1 - plot.x0) * i) / (n - 1));

  it('draws the svg 1:1 with its box, so labels keep their designed size', () => {
    const { container } = wrap(<Chart dates={dates} series={series} height={220} />);
    const svg = container.querySelector('svg') as SVGSVGElement;
    // jsdom has no layout: the chart falls back to its 480px design width.
    expect(svg.getAttribute('viewBox')).toBe('0 0 480 220');
    expect(svg.getAttribute('height')).toBe('220');
  });

  it('spaces x ticks at least the minimum gap apart, keeping first and last', () => {
    const x = xs(2400);
    const ticks = spacedTicks(x, 2395, 56);
    expect(ticks[0]).toBe(0);
    expect(ticks[ticks.length - 1]).toBe(2399);
    for (let k = 1; k < ticks.length; k++) expect(x[ticks[k]] - x[ticks[k - 1]]).toBeGreaterThanOrEqual(56);
    // the peak right next to the last tick is dropped rather than overlapping it
    expect(ticks).not.toContain(2395);
  });

  it('keeps the label of the wider segment when two step labels collide', () => {
    const x = xs(2400);
    const placed = placeRefLabels(
      [
        { index: 0, label: 'enumerate' },
        { index: 2, label: 'assess' },
        { index: 1500, label: 'verify' },
        { index: 2390, label: 'report' },
      ],
      x,
      plot,
    );
    expect(placed.map((p) => p.text)).toEqual(['assess', 'verify', 'report']);
    // a label at the right edge flips to end-anchored so it stays inside the plot
    expect(placed[2].anchor).toBe('end');
  });
});
