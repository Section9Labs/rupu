// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup, fireEvent, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import SortableTable, { type Column } from './SortableTable';

interface Row {
  id: string;
  name: string;
  cost: number | null;
}

const COLUMNS: Column<Row>[] = [
  {
    key: 'name',
    header: 'Name',
    sortable: true,
    sortValue: (r) => r.name,
    render: (r) => <span>{r.name}</span>,
  },
  {
    key: 'cost',
    header: 'Cost',
    align: 'right',
    sortable: true,
    sortValue: (r) => r.cost,
    render: (r) => <span>{r.cost === null ? '—' : `$${r.cost}`}</span>,
  },
];

const ROWS: Row[] = [
  { id: 'b', name: 'Beta', cost: 30 },
  { id: 'a', name: 'Alpha', cost: 10 },
  { id: 'c', name: 'Charlie', cost: null },
  { id: 'd', name: 'Delta', cost: 20 },
];

function renderTable(props?: Partial<React.ComponentProps<typeof SortableTable<Row>>>) {
  return render(
    <MemoryRouter>
      <SortableTable<Row> columns={COLUMNS} rows={ROWS} rowKey={(r) => r.id} {...props} />
    </MemoryRouter>,
  );
}

/** The visible names in body-row order. */
function bodyNames(): string[] {
  const rows = within(screen.getByRole('table')).getAllByRole('row').slice(1); // drop header
  return rows.map((r) => within(r).getAllByRole('cell')[0].textContent ?? '');
}

afterEach(cleanup);

describe('SortableTable', () => {
  it('keeps source order until a header is clicked, then sorts asc/desc on toggle', () => {
    renderTable();
    expect(bodyNames()).toEqual(['Beta', 'Alpha', 'Charlie', 'Delta']);

    // First click → ascending by name.
    fireEvent.click(screen.getByRole('button', { name: 'Sort by Name' }));
    expect(bodyNames()).toEqual(['Alpha', 'Beta', 'Charlie', 'Delta']);

    // Second click on the active column → descending.
    fireEvent.click(screen.getByRole('button', { name: 'Sort by Name' }));
    expect(bodyNames()).toEqual(['Delta', 'Charlie', 'Beta', 'Alpha']);
  });

  it('sorts numeric columns by raw value and keeps nulls last in both directions', () => {
    renderTable();
    const costHeader = screen.getByRole('button', { name: 'Sort by Cost' });

    // Ascending: 10, 20, 30, then null (Charlie) last.
    fireEvent.click(costHeader);
    expect(bodyNames()).toEqual(['Alpha', 'Delta', 'Beta', 'Charlie']);

    // Descending: 30, 20, 10, null STILL last.
    fireEvent.click(costHeader);
    expect(bodyNames()).toEqual(['Beta', 'Delta', 'Alpha', 'Charlie']);
  });

  it('honours initialSort', () => {
    renderTable({ initialSort: { key: 'name', dir: 'desc' } });
    expect(bodyNames()).toEqual(['Delta', 'Charlie', 'Beta', 'Alpha']);
  });

  it('reflects sort state via aria-sort on the column header', () => {
    renderTable();
    const headers = screen.getAllByRole('columnheader');
    const nameTh = headers[0];
    expect(nameTh).toHaveAttribute('aria-sort', 'none');

    fireEvent.click(screen.getByRole('button', { name: 'Sort by Name' }));
    expect(nameTh).toHaveAttribute('aria-sort', 'ascending');

    fireEvent.click(screen.getByRole('button', { name: 'Sort by Name' }));
    expect(nameTh).toHaveAttribute('aria-sort', 'descending');
  });

  it('renders rows as links when rowHref is provided', () => {
    renderTable({ rowHref: (r) => `/things/${r.id}` });
    const link = screen.getAllByRole('link')[0] as HTMLAnchorElement;
    expect(link).toHaveAttribute('href', '/things/b');
  });
});
