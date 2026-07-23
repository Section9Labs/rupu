// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { SearchInput } from './SearchInput';

afterEach(cleanup);

describe('SearchInput', () => {
  it('renders an icon-left magnifier alongside the input', () => {
    const { container } = render(<SearchInput aria-label="Filter files" value="" onChange={vi.fn()} />);
    expect(container.querySelector('svg')).toBeInTheDocument();
    expect(screen.getByLabelText('Filter files')).toBeInTheDocument();
  });

  it('forwards input props (value/onChange/placeholder)', () => {
    const onChange = vi.fn();
    render(
      <SearchInput aria-label="Filter files" value="foo" onChange={onChange} placeholder="Filter…" />,
    );
    const el = screen.getByLabelText('Filter files') as HTMLInputElement;
    expect(el.value).toBe('foo');
    expect(el).toHaveAttribute('placeholder', 'Filter…');
    fireEvent.change(el, { target: { value: 'foobar' } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('defaults to a generic placeholder when none is given', () => {
    render(<SearchInput aria-label="Search" value="" onChange={vi.fn()} />);
    expect(screen.getByLabelText('Search')).toHaveAttribute('placeholder', 'Search…');
  });

  it('gives the input left padding to clear the icon', () => {
    render(<SearchInput aria-label="Search" value="" onChange={vi.fn()} />);
    expect(screen.getByLabelText('Search').className).toMatch(/pl-6/);
  });
});
