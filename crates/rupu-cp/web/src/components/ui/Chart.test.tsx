// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { Chart, tooltipModel, type ChartSeries } from './Chart';
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
    // one filled area path per series
    expect(container.querySelectorAll('.cp-chart-riser path').length).toBe(2);
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
});
