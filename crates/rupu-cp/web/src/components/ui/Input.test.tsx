// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Input } from './Input';

afterEach(cleanup);

describe('Input', () => {
  it('forwards native input props', () => {
    const onChange = vi.fn();
    render(<Input aria-label="Name" value="hi" onChange={onChange} placeholder="type…" />);
    const el = screen.getByLabelText('Name') as HTMLInputElement;
    expect(el.value).toBe('hi');
    expect(el).toHaveAttribute('placeholder', 'type…');
    fireEvent.change(el, { target: { value: 'ho' } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('applies the shared field chrome and merges caller className', () => {
    render(<Input aria-label="Name" className="max-w-xs" />);
    const el = screen.getByLabelText('Name');
    expect(el.className).toMatch(/rounded-md/);
    expect(el.className).toMatch(/border-border/);
    expect(el.className).toMatch(/bg-panel/);
    expect(el.className).toMatch(/max-w-xs/);
  });

  it('supports disabled', () => {
    render(<Input aria-label="Name" disabled />);
    expect(screen.getByLabelText('Name')).toBeDisabled();
  });
});
