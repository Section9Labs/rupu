// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { Badge } from './Badge';

afterEach(cleanup);

describe('Badge', () => {
  it('renders flat (no ring) by default', () => {
    render(<Badge>neutral</Badge>);
    const badge = screen.getByText('neutral');
    expect(badge.className).not.toMatch(/ring-1/);
    expect(badge.className).toMatch(/bg-surface/);
  });

  it('applies the tone classes', () => {
    render(<Badge tone="green">ok</Badge>);
    expect(screen.getByText('ok').className).toMatch(/bg-ok-bg/);
  });

  it('adds the bordered ring look when `ring` is set', () => {
    render(<Badge ring>bordered</Badge>);
    const badge = screen.getByText('bordered');
    expect(badge.className).toMatch(/ring-1/);
    expect(badge.className).toMatch(/ring-border/);
  });

  it('switches size classes between sm and md', () => {
    const { rerender } = render(<Badge size="sm">x</Badge>);
    expect(screen.getByText('x').className).toMatch(/text-meta/);

    rerender(<Badge size="md">x</Badge>);
    expect(screen.getByText('x').className).toMatch(/text-note/);
  });

  it('merges a caller className alongside the base classes', () => {
    render(<Badge className="uppercase tracking-wide">x</Badge>);
    const badge = screen.getByText('x');
    expect(badge.className).toMatch(/uppercase/);
    expect(badge.className).toMatch(/bg-surface/);
  });
});
