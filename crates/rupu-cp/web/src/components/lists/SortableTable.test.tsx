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

  it('toggles an expandable detail row via the chevron (and ignores rowHref)', () => {
    renderTable({
      rowHref: (r) => `/things/${r.id}`,
      renderDetail: (r) => <div>detail for {r.name}</div>,
    });
    // Expandable tables are not link-wrapped.
    expect(screen.queryByRole('link')).toBeNull();
    // Detail hidden until expanded.
    expect(screen.queryByText('detail for Beta')).toBeNull();

    const toggles = screen.getAllByRole('button', { name: 'Expand row' });
    fireEvent.click(toggles[0]);
    expect(screen.getByText('detail for Beta')).toBeInTheDocument();

    // Clicking again collapses it.
    fireEvent.click(screen.getByRole('button', { name: 'Collapse row' }));
    expect(screen.queryByText('detail for Beta')).toBeNull();
  });

  it('is expandable per-row: a row whose renderDetail returns null gets NO chevron and IS link-wrapped by rowHref, while a row with detail content stays expandable', () => {
    renderTable({
      rowHref: (r) => `/things/${r.id}`,
      // Beta (row 0) has detail; every other row (Alpha, Charlie, Delta)
      // does not.
      renderDetail: (r) => (r.name === 'Beta' ? <div>detail for {r.name}</div> : null),
    });

    // Beta: expandable — no link, has a chevron, expands to show its detail.
    const rows = screen.getAllByRole('row').slice(1); // drop header row
    const betaRow = rows[0];
    expect(within(betaRow).queryByRole('link')).toBeNull();
    expect(within(betaRow).getByRole('button', { name: 'Expand row' })).toBeInTheDocument();
    fireEvent.click(within(betaRow).getByRole('button', { name: 'Expand row' }));
    expect(screen.getByText('detail for Beta')).toBeInTheDocument();

    // Alpha (row 1): no detail — link-wrapped by rowHref, no chevron button.
    const alphaRow = rows[1];
    expect(within(alphaRow).queryByRole('button', { name: 'Expand row' })).toBeNull();
    const alphaLink = within(alphaRow).getAllByRole('link')[0] as HTMLAnchorElement;
    expect(alphaLink).toHaveAttribute('href', '/things/a');
  });

  it('shrinks a fit column to its content on both th and td (w-[1%] + nowrap)', () => {
    const columns: Column<Row>[] = [
      { key: 'name', header: 'Name', render: (r) => <span>{r.name}</span> },
      { key: 'cost', header: 'Cost', fit: true, align: 'right', render: (r) => <span>{r.cost}</span> },
    ];
    renderTable({ columns });
    const th = screen.getAllByRole('columnheader')[1];
    expect(th.className).toMatch(/w-\[1%\]/);
    expect(th.className).toMatch(/whitespace-nowrap/);
    expect(th.className).toMatch(/text-right/);

    const costCell = within(screen.getAllByRole('row')[1]).getAllByRole('cell')[1];
    expect(costCell.className).toMatch(/w-\[1%\]/);
    expect(costCell.className).toMatch(/whitespace-nowrap/);
    expect(costCell.className).toMatch(/tabular-nums/);
  });

  it('truncates a subject column via max-w-0 + inner truncate + a title tooltip', () => {
    const columns: Column<Row>[] = [
      {
        key: 'name',
        header: 'Name',
        subject: true,
        titleValue: (r) => r.name,
        render: (r) => <span>{r.name}</span>,
      },
    ];
    renderTable({ columns });
    const nameCell = within(screen.getAllByRole('row')[1]).getAllByRole('cell')[0];
    expect(nameCell.className).toMatch(/max-w-0/);
    // The truncation wrapper is the cell's direct child (the caller's own
    // rendered markup nests inside it).
    const wrapper = nameCell.firstElementChild as HTMLElement;
    expect(wrapper.className).toMatch(/truncate/);
    expect(wrapper).toHaveAttribute('title', 'Beta');
    expect(wrapper).toHaveTextContent('Beta');
  });

  it('falls back to the rendered string as the subject title when titleValue is omitted', () => {
    const columns: Column<Row>[] = [
      { key: 'name', header: 'Name', subject: true, render: (r) => r.name },
    ];
    renderTable({ columns });
    const nameCell = within(screen.getAllByRole('row')[1]).getAllByRole('cell')[0];
    const wrapper = nameCell.firstElementChild as HTMLElement;
    expect(wrapper).toHaveAttribute('title', 'Beta');
  });
});
