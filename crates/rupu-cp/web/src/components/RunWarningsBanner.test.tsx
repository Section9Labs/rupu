// @vitest-environment jsdom
// RunWarningsBanner — the run page's own "this run has warnings" notice. It
// lists what `collectWarnings` found, caps a flood (one warning per remote unit
// of a wide fan-out) behind a disclosure, and renders nothing for a clean run.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, cleanup, within } from '@testing-library/react';
import RunWarningsBanner from './RunWarningsBanner';
import type { RunWarning } from '../lib/runGraphModel';

afterEach(cleanup);

describe('RunWarningsBanner', () => {
  it('renders nothing when the run has no warnings', () => {
    const { container } = render(<RunWarningsBanner warnings={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('says how many warnings, in the warning tone, and lists step · unit · message', () => {
    const warnings: RunWarning[] = [
      { stepId: 'sweep', index: 3, message: 'host gpu-9 did not stream coverage' },
      { stepId: 'triage', message: 'coverage merge skipped' },
    ];
    render(<RunWarningsBanner warnings={warnings} />);
    const banner = screen.getByTestId('run-warnings');
    expect(banner).toHaveAttribute('role', 'status');
    expect(banner).toHaveClass('text-warn');
    expect(within(banner).getByText('2 warnings')).toBeInTheDocument();
    expect(within(banner).getByText('sweep · unit 3')).toBeInTheDocument();
    expect(within(banner).getByText('host gpu-9 did not stream coverage')).toBeInTheDocument();
    expect(within(banner).getByText('triage')).toBeInTheDocument();
    expect(within(banner).getByText('coverage merge skipped')).toBeInTheDocument();
  });

  it('uses the singular for one warning', () => {
    render(<RunWarningsBanner warnings={[{ stepId: 'a', message: 'm' }]} />);
    expect(screen.getByText('1 warning')).toBeInTheDocument();
  });

  it('caps a flood: the first few show, the rest sit behind a disclosure', () => {
    const warnings: RunWarning[] = Array.from({ length: 40 }, (_, i) => ({
      stepId: 'sweep', index: i, message: `unit ${i} lost coverage`,
    }));
    const { container } = render(<RunWarningsBanner warnings={warnings} />);
    expect(screen.getByText('40 warnings')).toBeInTheDocument();
    expect(screen.getByText('unit 0 lost coverage')).toBeVisible();
    const details = container.querySelector('details') as HTMLDetailsElement;
    expect(details).not.toBeNull();
    expect(details.open).toBe(false);
    expect(within(details).getByText('Show 35 more')).toBeInTheDocument();
    expect(within(details).getByText('unit 39 lost coverage')).toBeInTheDocument();
  });
});
