// @vitest-environment jsdom
import { afterEach, describe, it, expect, vi } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { FindingMetrics } from './FindingMetrics';
import type { FindingsSummary } from '../../lib/api';

afterEach(cleanup);

const summary: FindingsSummary = {
  total: 21,
  critical: 1,
  high: 2,
  medium: 5,
  low: 6,
  info: 7,
};

describe('FindingMetrics', () => {
  it('renders the six tiles with the right numbers', () => {
    render(<FindingMetrics summary={summary} />);
    expect(screen.getByText('Total')).toBeInTheDocument();
    expect(screen.getByText('21')).toBeInTheDocument();
    expect(screen.getByText('1')).toBeInTheDocument();
    expect(screen.getByText('2')).toBeInTheDocument();
    expect(screen.getByText('5')).toBeInTheDocument();
    expect(screen.getByText('6')).toBeInTheDocument();
    expect(screen.getByText('7')).toBeInTheDocument();
  });

  it('renders static (non-button) tiles when onSelect is absent', () => {
    render(<FindingMetrics summary={summary} />);
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('fires onSelect with the severity / null when tiles are clicked', () => {
    const onSelect = vi.fn();
    render(<FindingMetrics summary={summary} onSelect={onSelect} />);

    fireEvent.click(screen.getByRole('button', { name: /Filter by high/i }));
    expect(onSelect).toHaveBeenCalledWith('high');

    fireEvent.click(screen.getByRole('button', { name: /Filter by Total/i }));
    expect(onSelect).toHaveBeenCalledWith(null);
  });

  it('marks the active tile as pressed', () => {
    const onSelect = vi.fn();
    render(<FindingMetrics summary={summary} active="critical" onSelect={onSelect} />);
    expect(screen.getByRole('button', { name: /Filter by critical/i })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect(screen.getByRole('button', { name: /Filter by Total/i })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
  });
});
