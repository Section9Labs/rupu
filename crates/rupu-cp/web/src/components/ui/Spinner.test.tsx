// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import { Spinner } from './Spinner';
import { Skeleton } from './Skeleton';

afterEach(() => {
  cleanup();
});

describe('Spinner', () => {
  it('renders a spinning glyph with an accessible status role', () => {
    const { container, getByRole } = render(<Spinner />);
    const status = getByRole('status');
    expect(status).toBeInTheDocument();
    expect(status).toHaveAttribute('aria-label', 'Loading');
    // The lucide Loader2 glyph carries the spin animation class.
    const svg = container.querySelector('svg');
    expect(svg).toBeInTheDocument();
    expect(svg?.getAttribute('class')).toMatch(/animate-spin/);
  });

  it('renders the given label as visible text and as the accessible name', () => {
    const { getByRole, getByText } = render(<Spinner label="Updating" />);
    expect(getByRole('status')).toHaveAttribute('aria-label', 'Updating');
    expect(getByText('Updating')).toBeInTheDocument();
  });

  it('scales the glyph per the size prop (sm/md/lg or an explicit number)', () => {
    const { container: sm } = render(<Spinner size="sm" />);
    const { container: lg } = render(<Spinner size="lg" />);
    const { container: custom } = render(<Spinner size={40} />);

    expect(sm.querySelector('svg')).toHaveAttribute('width', '14');
    expect(lg.querySelector('svg')).toHaveAttribute('width', '28');
    expect(custom.querySelector('svg')).toHaveAttribute('width', '40');
  });

  it('never hardcodes a color literal — uses the themed ink-mute token class', () => {
    const { getByRole } = render(<Spinner />);
    expect(getByRole('status').className).toMatch(/\btext-ink-mute\b/);
  });
});

describe('Skeleton', () => {
  it('renders a pulsing themed block that composes with className', () => {
    const { container } = render(<Skeleton className="h-4 w-32" />);
    const el = container.firstElementChild as HTMLElement;
    expect(el.className).toMatch(/animate-pulse/);
    expect(el.className).toMatch(/\bbg-surface\b/);
    expect(el.className).toMatch(/h-4/);
    expect(el.className).toMatch(/w-32/);
  });
});
