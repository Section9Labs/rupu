// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ThemeContext, type ThemeContextValue } from '../theme/ThemeProvider';
import type { CustomerRef } from '../../lib/api';
import { CustomerChip } from './CustomerChip';
import { CustomerDot } from './CustomerDot';
import { PricingErrorMark } from './PricingErrorMark';

const acme: CustomerRef = {
  slug: 'acme',
  name: 'Acme Corp',
  tint: { light: '#aa0000', dark: '#00bb00' },
  archived: false,
};

afterEach(cleanup);

function inTheme(mode: 'light' | 'dark', ui: JSX.Element) {
  const value = { mode } as unknown as ThemeContextValue;
  return render(<ThemeContext.Provider value={value}>{ui}</ThemeContext.Provider>);
}

describe('CustomerDot', () => {
  it('uses the light tint in light mode and the dark tint in dark mode', () => {
    const l = inTheme('light', <CustomerDot tint={acme.tint} />);
    expect((l.container.firstElementChild as HTMLElement).style.backgroundColor).toBe('rgb(170, 0, 0)');
    l.unmount();
    const d = inTheme('dark', <CustomerDot tint={acme.tint} />);
    expect((d.container.firstElementChild as HTMLElement).style.backgroundColor).toBe('rgb(0, 187, 0)');
  });
  it('is aria-hidden unless it has a title', () => {
    const a = render(<CustomerDot tint={acme.tint} />);
    expect(a.container.firstElementChild!.getAttribute('aria-hidden')).toBe('true');
    a.unmount();
    const b = render(<CustomerDot tint={acme.tint} title="Acme" />);
    expect(b.container.firstElementChild!.getAttribute('aria-hidden')).toBeNull();
    expect(b.container.firstElementChild!.getAttribute('title')).toBe('Acme');
  });
});

describe('CustomerChip', () => {
  it('renders the name with a dot tinted for the theme', () => {
    const { container } = inTheme('dark', <CustomerChip customer={acme} />);
    expect(screen.getByText('Acme Corp')).toBeTruthy();
    const dot = container.querySelector('[data-customer-dot]') as HTMLElement;
    expect(dot.style.backgroundColor).toBe('rgb(0, 187, 0)');
  });

  it('null renders the dashed "No customer" pill', () => {
    const { container } = render(<CustomerChip customer={null} />);
    expect(screen.getByText('No customer')).toBeTruthy();
    expect((container.firstElementChild as HTMLElement).className).toContain('border-dashed');
    expect(container.querySelector('[data-customer-dot]')).toBeNull();
  });

  it('unknown renders a muted state that is not "No customer"', () => {
    const { container } = render(<CustomerChip customer={null} unknown />);
    expect(screen.queryByText('No customer')).toBeNull();
    expect(screen.getByText('Unknown customer')).toBeTruthy();
    const el = container.firstElementChild as HTMLElement;
    expect(el.getAttribute('title')).toBe("This host or record can't say whose this is");
    expect(el.className).toContain('text-ink-mute');
  });

  it('derived adds the attribution tooltip', () => {
    const { container } = render(<CustomerChip customer={acme} derived />);
    expect((container.firstElementChild as HTMLElement).getAttribute('title')).toBe(
      "Attributed from the project's current customer",
    );
  });

  it('shows archived customers muted', () => {
    const { container } = render(<CustomerChip customer={{ ...acme, archived: true }} />);
    expect((container.firstElementChild as HTMLElement).className).toContain('opacity-60');
  });

  it('× calls onRemove and is labelled', () => {
    const onRemove = vi.fn();
    render(<CustomerChip customer={acme} onRemove={onRemove} />);
    fireEvent.click(screen.getByRole('button', { name: 'Clear customer scope' }));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it('renders no × without onRemove', () => {
    render(<CustomerChip customer={acme} />);
    expect(screen.queryByRole('button')).toBeNull();
  });
});

describe('PricingErrorMark', () => {
  it('renders nothing without an error', () => {
    const a = render(<PricingErrorMark />);
    expect(a.container.firstChild).toBeNull();
    a.unmount();
    const b = render(<PricingErrorMark error={null} />);
    expect(b.container.firstChild).toBeNull();
    b.unmount();
    const c = render(<PricingErrorMark error="" />);
    expect(c.container.firstChild).toBeNull();
  });

  it('renders a warn marker with the error as tooltip and accessible label', () => {
    render(<PricingErrorMark error="pricing file unreadable" />);
    const el = screen.getByLabelText('Pricing unavailable: pricing file unreadable');
    expect(el.getAttribute('title')).toBe('pricing file unreadable');
    expect(el.className).toContain('text-warn');
    expect(el.getAttribute('role')).toBe('img');
  });
});
