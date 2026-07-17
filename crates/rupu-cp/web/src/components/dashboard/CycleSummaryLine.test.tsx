// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { CycleSummaryLine } from './CycleSummaryLine';
import type { CycleCounts } from '../../lib/api';

afterEach(() => {
  cleanup();
});

const wrap = (ui: React.ReactNode) => render(<MemoryRouter>{ui}</MemoryRouter>);

describe('CycleSummaryLine', () => {
  it('renders total / clean / with-failures counts', () => {
    const cycles: CycleCounts = { total: 8, clean: 6, with_failures: 2 };
    wrap(<CycleSummaryLine cycles={cycles} cyclesPartial={false} />);
    expect(screen.getByText(/8/)).toBeInTheDocument();
    expect(screen.getByText(/cycles/)).toBeInTheDocument();
    expect(screen.getByText(/6/)).toBeInTheDocument();
    expect(screen.getByText(/clean/)).toBeInTheDocument();
    expect(screen.getByText(/2/)).toBeInTheDocument();
    expect(screen.getByText(/with failures/)).toBeInTheDocument();
  });

  it('renders null clean/with_failures as an em-dash, never 0', () => {
    const cycles: CycleCounts = { total: 3, clean: null, with_failures: null };
    const { container } = wrap(<CycleSummaryLine cycles={cycles} cyclesPartial={false} />);
    expect(container.textContent).toContain('—');
    expect(container.textContent).not.toMatch(/\b0\b/);
  });

  it('marks a partial split as "(partial)"', () => {
    const cycles: CycleCounts = { total: 5, clean: 4, with_failures: 1 };
    wrap(<CycleSummaryLine cycles={cycles} cyclesPartial={true} />);
    expect(screen.getByText(/\(partial\)/)).toBeInTheDocument();
  });

  it('links "see all" to /runs', () => {
    const cycles: CycleCounts = { total: 1, clean: 1, with_failures: 0 };
    wrap(<CycleSummaryLine cycles={cycles} cyclesPartial={false} />);
    const link = screen.getByRole('link', { name: /see all/i });
    expect(link).toHaveAttribute('href', '/runs');
  });
});
