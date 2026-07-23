// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { FilterBar } from './FilterBar';

afterEach(cleanup);

describe('FilterBar', () => {
  it('renders slots in the fixed order: view, filters, search, scope', () => {
    const { container } = render(
      <FilterBar
        view={<span data-testid="view">V</span>}
        filters={<span data-testid="filters">F</span>}
        search={<span data-testid="search">S</span>}
        scope={<span data-testid="scope">H</span>}
      />,
    );
    const ids = Array.from(container.querySelectorAll('[data-testid]')).map((el) =>
      el.getAttribute('data-testid'),
    );
    expect(ids).toEqual(['view', 'filters', 'search', 'scope']);
  });

  it('omits empty slots entirely rather than rendering a placeholder', () => {
    render(<FilterBar view={<span data-testid="view">V</span>} />);
    expect(screen.getByTestId('view')).toBeInTheDocument();
    expect(screen.queryByTestId('filters')).toBeNull();
    expect(screen.queryByTestId('search')).toBeNull();
    expect(screen.queryByTestId('scope')).toBeNull();
  });

  it('renders nothing but the container when every slot is omitted', () => {
    const { container } = render(<FilterBar />);
    expect(container.firstElementChild?.children.length).toBe(0);
  });

  it('lays out as a single wrapping flex row', () => {
    const { container } = render(<FilterBar view={<span>V</span>} search={<span>S</span>} />);
    const row = container.firstElementChild as HTMLElement;
    expect(row.className).toMatch(/flex/);
    expect(row.className).toMatch(/flex-wrap/);
    expect(row.className).toMatch(/items-center/);
    expect(row.className).toMatch(/gap-2\.5/);
  });
});
