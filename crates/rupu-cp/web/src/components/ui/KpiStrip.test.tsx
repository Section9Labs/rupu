// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { KpiStrip } from './KpiStrip';

afterEach(cleanup);

describe('KpiStrip', () => {
  it('renders each item as label + value + optional meta', () => {
    render(
      <KpiStrip
        items={[
          { label: 'Failed', value: 27, attention: true, meta: '+4 vs prior 30d', testId: 'k-failed' },
          { label: 'Success rate', value: '92%' },
        ]}
      />,
    );
    expect(screen.getByText('Failed')).toBeInTheDocument();
    expect(screen.getByText('27')).toBeInTheDocument();
    expect(screen.getByText('+4 vs prior 30d')).toBeInTheDocument();
    expect(screen.getByText('92%')).toBeInTheDocument();
  });

  it('paints the attention value in the failed tone, others in ink', () => {
    render(
      <KpiStrip
        items={[
          { label: 'Failed', value: 27, attention: true, testId: 'k-failed' },
          { label: 'Cycles', value: 100, testId: 'k-cycles' },
        ]}
      />,
    );
    expect(screen.getByText('27').className).toMatch(/text-status-failed/);
    expect(screen.getByText('100').className).toMatch(/text-ink\b/);
  });

  it('values use tabular numbers', () => {
    render(<KpiStrip items={[{ label: 'Cycles', value: 100 }]} />);
    expect(screen.getByText('100').className).toMatch(/tabular-nums/);
  });
});
