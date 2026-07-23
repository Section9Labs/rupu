// @vitest-environment jsdom
// NOTE: no Button test previously existed in the repo (the Task B plan says
// "extend its test", but there was nothing to extend) — this file is new,
// covering the pre-existing variants alongside the new ring/ring-danger/link
// ones so a future change to any variant's shape is caught.
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Button } from './Button';

afterEach(cleanup);

describe('Button', () => {
  it('defaults to the primary variant and md size', () => {
    render(<Button>Go</Button>);
    const btn = screen.getByRole('button', { name: 'Go' });
    expect(btn).toHaveAttribute('type', 'button');
    expect(btn.className).toMatch(/bg-brand-600/);
    expect(btn.className).toMatch(/rounded-md/);
    expect(btn.className).toMatch(/px-3/);
  });

  it('fires onClick', () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Go</Button>);
    fireEvent.click(screen.getByRole('button', { name: 'Go' }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['secondary', /border-border/],
    ['ghost', /text-ink-dim/],
    ['danger', /bg-err\b/],
    ['danger-outline', /border-err\/30/],
  ] as const)('renders the pre-existing %s variant unchanged', (variant, expected) => {
    render(<Button variant={variant}>X</Button>);
    expect(screen.getByRole('button', { name: 'X' }).className).toMatch(expected);
  });

  it('ring variant: compact ring-bordered pill, no rounded-md/size padding', () => {
    render(<Button variant="ring">Archive</Button>);
    const btn = screen.getByRole('button', { name: 'Archive' });
    expect(btn.className).toMatch(/\bring-1\b/);
    expect(btn.className).toMatch(/ring-border/);
    expect(btn.className).toMatch(/\brounded\b/);
    expect(btn.className).not.toMatch(/rounded-md/);
    expect(btn.className).toMatch(/px-2\b/);
    expect(btn.className).not.toMatch(/px-3/);
  });

  it('ring-danger variant: err tones on the same compact shape', () => {
    render(<Button variant="ring-danger">Delete</Button>);
    const btn = screen.getByRole('button', { name: 'Delete' });
    expect(btn.className).toMatch(/ring-err\/30/);
    expect(btn.className).toMatch(/bg-err-bg/);
    expect(btn.className).toMatch(/text-err/);
    expect(btn.className).toMatch(/\brounded\b/);
    expect(btn.className).not.toMatch(/rounded-md/);
  });

  it('link variant: no chrome, brand-colored text', () => {
    render(<Button variant="link">See more</Button>);
    const btn = screen.getByRole('button', { name: 'See more' });
    expect(btn.className).toMatch(/text-brand-600/);
    expect(btn.className).not.toMatch(/rounded-md/);
    expect(btn.className).not.toMatch(/\bbg-brand-600\b/);
  });

  it('ignores the size prop for compact variants (ring/ring-danger/link)', () => {
    const { rerender } = render(
      <Button variant="ring" size="sm">
        A
      </Button>,
    );
    const smCls = screen.getByRole('button', { name: 'A' }).className;
    rerender(
      <Button variant="ring" size="md">
        A
      </Button>,
    );
    const mdCls = screen.getByRole('button', { name: 'A' }).className;
    expect(smCls).toBe(mdCls);
  });

  it('merges an extra className via twMerge without dropping the variant color', () => {
    render(
      <Button variant="primary" className="gap-1.5">
        Go
      </Button>,
    );
    const btn = screen.getByRole('button', { name: 'Go' });
    expect(btn.className).toMatch(/gap-1\.5/);
    expect(btn.className).toMatch(/bg-brand-600/);
  });

  it('disables via the native disabled attribute', () => {
    render(<Button disabled>Go</Button>);
    expect(screen.getByRole('button', { name: 'Go' })).toBeDisabled();
  });
});
