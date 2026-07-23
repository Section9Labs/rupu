// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Select } from './Select';

afterEach(cleanup);

describe('Select', () => {
  it('forwards native select props and renders option children', () => {
    const onChange = vi.fn();
    render(
      <Select aria-label="Pick" value="b" onChange={onChange}>
        <option value="a">A</option>
        <option value="b">B</option>
      </Select>,
    );
    const el = screen.getByLabelText('Pick') as HTMLSelectElement;
    expect(el.value).toBe('b');
    fireEvent.change(el, { target: { value: 'a' } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('applies the shared select chrome and merges caller className', () => {
    render(
      <Select aria-label="Pick" className="w-40">
        <option value="a">A</option>
      </Select>,
    );
    const el = screen.getByLabelText('Pick');
    expect(el.className).toMatch(/rounded-md/);
    expect(el.className).toMatch(/border-border/);
    expect(el.className).toMatch(/bg-panel/);
    expect(el.className).toMatch(/w-40/);
  });

  it('supports disabled', () => {
    render(
      <Select aria-label="Pick" disabled>
        <option value="a">A</option>
      </Select>,
    );
    expect(screen.getByLabelText('Pick')).toBeDisabled();
  });
});
