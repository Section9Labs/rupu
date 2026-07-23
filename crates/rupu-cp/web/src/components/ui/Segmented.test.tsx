// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Segmented } from './Segmented';

const OPTIONS = [
  { value: 'a', label: 'Alpha' },
  { value: 'b', label: 'Beta' },
];

afterEach(cleanup);

describe('Segmented', () => {
  it('marks the active option via aria-pressed and applies the active style', () => {
    render(<Segmented options={OPTIONS} value="a" onChange={vi.fn()} />);
    const alpha = screen.getByRole('button', { name: 'Alpha' });
    const beta = screen.getByRole('button', { name: 'Beta' });
    expect(alpha).toHaveAttribute('aria-pressed', 'true');
    expect(beta).toHaveAttribute('aria-pressed', 'false');
    expect(alpha.className).toMatch(/bg-surface/);
    expect(alpha.className).toMatch(/text-ink\b/);
    expect(beta.className).toMatch(/text-ink-dim/);
  });

  it('calls onChange with the clicked option value', () => {
    const onChange = vi.fn();
    render(<Segmented options={OPTIONS} value="a" onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: 'Beta' }));
    expect(onChange).toHaveBeenCalledWith('b');
  });

  it('applies the boxed/joined container chrome', () => {
    render(<Segmented options={OPTIONS} value="a" onChange={vi.fn()} ariaLabel="View" />);
    const group = screen.getByRole('group', { name: 'View' });
    expect(group.className).toMatch(/rounded-md/);
    expect(group.className).toMatch(/border-border/);
    expect(group.className).toMatch(/bg-panel/);
    expect(group.className).toMatch(/p-0\.5/);
  });

  it('switches size classes between sm and md', () => {
    const { rerender } = render(
      <Segmented options={OPTIONS} value="a" onChange={vi.fn()} size="sm" />,
    );
    expect(screen.getByRole('button', { name: 'Alpha' }).className).toMatch(/text-note/);

    rerender(<Segmented options={OPTIONS} value="a" onChange={vi.fn()} size="md" />);
    expect(screen.getByRole('button', { name: 'Alpha' }).className).toMatch(/text-ui/);
  });

  it('never lets an option label wrap', () => {
    render(<Segmented options={OPTIONS} value="a" onChange={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'Alpha' }).className).toMatch(/whitespace-nowrap/);
  });
});
