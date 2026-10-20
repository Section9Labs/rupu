// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { SummaryStrip } from './SummaryStrip';

afterEach(cleanup);

describe('SummaryStrip', () => {
  it('renders key/value cells with tabular numbers', () => {
    render(<SummaryStrip items={[{ label: 'Repos', value: 620 }]} />);
    expect(screen.getByText('Repos')).toBeInTheDocument();
    const v = screen.getByText('620');
    expect(v.className).toMatch(/tabular-nums/);
  });

  it('renders a quiet sub value beside the primary', () => {
    render(<SummaryStrip items={[{ label: 'Autoflows', value: 0, sub: '· 7 off' }]} />);
    expect(screen.getByText('· 7 off')).toBeInTheDocument();
  });

  it('shows an em-dash value verbatim (not-reported)', () => {
    render(<SummaryStrip items={[{ label: 'Workers', value: '—' }]} />);
    expect(screen.getByText('—')).toBeInTheDocument();
  });
});
