// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import SortableTable, { type Column } from './SortableTable';

afterEach(cleanup);
type Row = { id: string; locked?: boolean };
const cols: Column<Row>[] = [{ key: 'id', header: 'Id', subject: true, render: (r) => r.id }];
const rows: Row[] = [{ id: 'a' }, { id: 'b' }, { id: 'c', locked: true }];

function setup(selected: Set<string>) {
  const onToggle = vi.fn();
  const onToggleAll = vi.fn();
  render(
    <MemoryRouter>
      <SortableTable
        columns={cols}
        rows={rows}
        rowKey={(r) => r.id}
        selection={{
          isSelected: (r) => selected.has(r.id),
          blockedReason: (r) => (r.locked ? 'locked' : null),
          label: (r) => `Select ${r.id}`,
          onToggle,
          onToggleAll,
        }}
      />
    </MemoryRouter>,
  );
  return { onToggle, onToggleAll };
}

describe('SortableTable selection', () => {
  it('renders a checkbox per row, disabled with a reason when blocked', () => {
    const { onToggle } = setup(new Set(['a']));
    expect(screen.getByRole('checkbox', { name: 'Select a' })).toBeChecked();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select b' }));
    expect(onToggle).toHaveBeenCalledWith(rows[1]);
    const locked = screen.getByRole('checkbox', { name: 'Select c' });
    expect(locked).toBeDisabled();
    expect(locked).toHaveAttribute('title', 'locked');
  });
  it('header checkbox selects every selectable row and is indeterminate when some are selected', () => {
    const { onToggleAll } = setup(new Set(['a']));
    const all = screen.getByRole('checkbox', { name: 'Select all' }) as HTMLInputElement;
    expect(all.indeterminate).toBe(true);
    fireEvent.click(all);
    expect(onToggleAll).toHaveBeenCalledWith([rows[0], rows[1]], true);
  });
  it('header checkbox clears when every selectable row is selected', () => {
    const { onToggleAll } = setup(new Set(['a', 'b']));
    const all = screen.getByRole('checkbox', { name: 'Select all' }) as HTMLInputElement;
    expect(all).toBeChecked();
    expect(all.indeterminate).toBe(false);
    fireEvent.click(all);
    expect(onToggleAll).toHaveBeenCalledWith([rows[0], rows[1]], false);
  });
  it("a key on a row's checkbox never reaches the row's click", () => {
    const onRowClick = vi.fn();
    render(
      <MemoryRouter>
        <SortableTable
          columns={cols}
          rows={[{ id: 'a' }]}
          rowKey={(r) => r.id}
          onRowClick={onRowClick}
          selection={{ isSelected: () => false, label: (r) => `Select ${r.id}`, onToggle: vi.fn(), onToggleAll: vi.fn() }}
        />
      </MemoryRouter>,
    );
    const box = screen.getByRole('checkbox', { name: 'Select a' });
    const space = fireEvent.keyDown(box, { key: ' ' });
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(onRowClick).not.toHaveBeenCalled();
    // Not default-prevented, so the browser still toggles the box.
    expect(space).toBe(true);
  });
  it('no selection prop renders no checkboxes', () => {
    render(
      <MemoryRouter>
        <SortableTable columns={cols} rows={rows} rowKey={(r) => r.id} />
      </MemoryRouter>,
    );
    expect(screen.queryByRole('checkbox')).toBeNull();
  });
  it('the detail row spans the checkbox column too', () => {
    render(
      <MemoryRouter>
        <SortableTable
          columns={cols}
          rows={[{ id: 'a' }]}
          rowKey={(r) => r.id}
          renderDetail={() => <p>detail</p>}
          selection={{
            isSelected: () => false,
            label: (r) => `Select ${r.id}`,
            onToggle: vi.fn(),
            onToggleAll: vi.fn(),
          }}
        />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Expand row' }));
    // checkbox + chevron + one data column
    expect(screen.getByText('detail').closest('td')).toHaveAttribute('colspan', '3');
  });
});
