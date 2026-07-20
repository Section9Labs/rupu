// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { OutlierPanel } from './OutlierPanel';
import type { OutlierRun } from '../../lib/api';

afterEach(() => {
  cleanup();
});

function outlier(overrides: Partial<OutlierRun> = {}): OutlierRun {
  return {
    run_id: 'run-42',
    workflow_name: 'nightly-review',
    cost_usd: 12,
    baseline_usd: 3,
    ratio: 4,
    started_at: new Date().toISOString(),
    ...overrides,
  };
}

function renderPanel(props: Partial<React.ComponentProps<typeof OutlierPanel>> = {}) {
  return render(
    <MemoryRouter>
      <OutlierPanel outliers={[outlier()]} {...props} />
    </MemoryRouter>,
  );
}

describe('OutlierPanel', () => {
  it('renders the empty state with no outliers', () => {
    render(
      <MemoryRouter>
        <OutlierPanel outliers={[]} />
      </MemoryRouter>,
    );
    expect(screen.getByText(/No cost outliers in this window/)).toBeInTheDocument();
  });

  it('does not render an exclude toggle when onToggleRun is absent', () => {
    renderPanel();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  });

  it('calls onToggleRun with the row run_id when its toggle is clicked', () => {
    const onToggleRun = vi.fn();
    renderPanel({ onToggleRun, excludedRunIds: new Set() });
    fireEvent.click(screen.getByRole('checkbox', { name: 'run-42' }));
    expect(onToggleRun).toHaveBeenCalledWith('run-42');
  });

  it('renders an excluded run unchecked and visibly muted', () => {
    renderPanel({ onToggleRun: () => {}, excludedRunIds: new Set(['run-42']) });
    const checkbox = screen.getByRole('checkbox', { name: 'run-42' }) as HTMLInputElement;
    expect(checkbox.checked).toBe(false);
    expect(screen.getByText('nightly-review')).toHaveClass('line-through');
  });
});
