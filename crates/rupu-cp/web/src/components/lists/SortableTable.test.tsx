// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, within, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import SortableTable, { type Column } from './SortableTable';
import { ESTIMATED_ROW_PX, OVERSCAN_ROWS } from './useWindowVirtualRows';

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

  // I7 (whole-branch-review, a11y): rowHref used to wrap EVERY cell's
  // content in its own <Link>, so a multi-column row was one tab stop per
  // column and screen readers announced the same destination once per cell.
  // Only the subject column's link stays a real, focusable, announced link;
  // the rest are mouse-only (tabIndex=-1 + aria-hidden), so a row is exactly
  // one Tab stop, while the whole row stays clickable.
  it('link-wraps only the subject cell for keyboard/AT — other cells are mouse-only', () => {
    const columns: Column<Row>[] = [
      { key: 'name', header: 'Name', subject: true, render: (r) => <span>{r.name}</span> },
      { key: 'cost', header: 'Cost', align: 'right', render: (r) => <span>{r.cost}</span> },
    ];
    renderTable({ columns, rowHref: (r) => `/things/${r.id}` });

    const betaRow = screen.getAllByRole('row')[1];
    // Exactly one accessible (non-hidden) link per row.
    expect(within(betaRow).getAllByRole('link')).toHaveLength(1);

    const cells = within(betaRow).getAllByRole('cell');
    const nameLink = cells[0].querySelector('a')!;
    const costLink = cells[1].querySelector('a')!;
    expect(nameLink).not.toHaveAttribute('aria-hidden');
    expect(nameLink).not.toHaveAttribute('tabindex');
    expect(costLink).toHaveAttribute('aria-hidden', 'true');
    expect(costLink).toHaveAttribute('tabindex', '-1');
    // Still present (and pointing at the row href) for mouse clicks.
    expect(costLink).toHaveAttribute('href', '/things/b');
  });

  // Final re-review: an `interactive` column must render as a plain,
  // unwrapped cell (not link-wrapped at all) so its own controls stay
  // queryable/focusable by role, and so a column that sometimes renders
  // `null` (e.g. a conditional action button) never produces an empty
  // anchor with no accessible name. The rest of the row must still
  // navigate via rowHref.
  it('renders an interactive column as a plain cell — its controls stay queryable/focusable, and the rest of the row still navigates', () => {
    const columns: Column<Row>[] = [
      { key: 'name', header: 'Name', subject: true, render: (r) => <span>{r.name}</span> },
      {
        key: 'action',
        header: '',
        interactive: true,
        render: (r) => (r.name === 'Beta' ? <button type="button">Open</button> : null),
      },
    ];
    renderTable({ columns, rowHref: (r) => `/things/${r.id}` });

    const betaRow = screen.getAllByRole('row')[1]; // Beta
    const alphaRow = screen.getAllByRole('row')[2]; // Alpha

    // Exactly one link per row (the subject cell) — the interactive column
    // is never link-wrapped, so it never adds a second anchor.
    expect(within(betaRow).getAllByRole('link')).toHaveLength(1);
    expect(within(alphaRow).getAllByRole('link')).toHaveLength(1);

    // The button in the interactive column stays independently queryable
    // and focusable by role — not swallowed inside a mouse-only anchor.
    const openButton = within(betaRow).getByRole('button', { name: 'Open' });
    expect(openButton).toBeInTheDocument();
    expect(openButton.closest('a')).toBeNull();

    // Alpha's interactive cell renders null — no empty, unnamed anchor.
    const alphaCells = within(alphaRow).getAllByRole('cell');
    const alphaActionCell = alphaCells[1];
    expect(alphaActionCell.querySelector('a')).toBeNull();

    // The subject cell still carries the row's real navigation link.
    const nameLink = within(betaRow).getByRole('link');
    expect(nameLink).toHaveAttribute('href', '/things/b');
  });
});

describe('SortableTable virtualization', () => {
  type VRow = { id: string; n: number };
  const rows = (count: number): VRow[] =>
    Array.from({ length: count }, (_, i) => ({ id: `r${i}`, n: count - i }));
  const columns = [
    { key: 'id', header: 'Id', render: (r: VRow) => r.id },
    { key: 'n', header: 'N', sortable: true, sortValue: (r: VRow) => r.n, render: (r: VRow) => String(r.n) },
  ];
  // The first tbody is the real (virtual) body; a second, aria-hidden one is
  // the width probe.
  const mounted = (c: HTMLElement) =>
    Array.from(c.querySelectorAll('tbody:first-of-type tr:not([aria-hidden])')) as HTMLTableRowElement[];
  const spacers = (c: HTMLElement) =>
    Array.from(c.querySelectorAll('tbody:first-of-type tr[aria-hidden]')) as HTMLTableRowElement[];
  const probe = (c: HTMLElement) => c.querySelectorAll('tbody')[1] as HTMLTableSectionElement | undefined;
  const probeTexts = (c: HTMLElement, col: number) =>
    Array.from(probe(c)?.querySelectorAll('tr') ?? []).map((tr) => tr.children[col].textContent);
  const scrollTo = async (rowsAbove: number) => {
    vi.spyOn(HTMLTableSectionElement.prototype, 'getBoundingClientRect').mockReturnValue({
      top: -rowsAbove * ESTIMATED_ROW_PX, bottom: 0, left: 0, right: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({}),
    } as DOMRect);
    await act(async () => {
      document.dispatchEvent(new Event('scroll'));
    });
  };

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('at or below the threshold renders every row exactly as before', () => {
    const { container } = render(
      <SortableTable columns={columns} rows={rows(10)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
    );
    expect(mounted(container)).toHaveLength(10);
    expect(spacers(container)).toHaveLength(0);
  });

  it('above the threshold mounts only the viewport plus overscan, with a bottom spacer', () => {
    const { container } = render(
      <SortableTable columns={columns} rows={rows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
    );
    // jsdom: no layout, so rows use the estimate; viewport = innerHeight (768).
    const visible = Math.ceil(window.innerHeight / ESTIMATED_ROW_PX);
    expect(mounted(container)).toHaveLength(visible + OVERSCAN_ROWS);
    const [bottom] = spacers(container);
    expect(bottom.style.height).toBe(`${(1000 - visible - OVERSCAN_ROWS) * ESTIMATED_ROW_PX}px`);
  });

  it('scrolling moves the window and adds a top spacer', async () => {
    const { container } = render(
      <SortableTable columns={columns} rows={rows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
    );
    vi.spyOn(HTMLTableSectionElement.prototype, 'getBoundingClientRect').mockReturnValue({
      top: -100 * ESTIMATED_ROW_PX, bottom: 0, left: 0, right: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({}),
    } as DOMRect);
    await act(async () => {
      document.dispatchEvent(new Event('scroll'));
    });
    const first = mounted(container)[0];
    expect(first.textContent).toContain(`r${100 - OVERSCAN_ROWS}`);
    expect(spacers(container)[0].style.height).toBe(`${(100 - OVERSCAN_ROWS) * ESTIMATED_ROW_PX}px`);
  });

  it('sorting still orders the full row set', () => {
    const { container } = render(
      <SortableTable columns={columns} rows={rows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Sort by N' }));
    expect(mounted(container)[0].textContent).toContain('r999'); // n = 1, ascending
  });
  describe('width probe', () => {
    // A wide-column fixture: row 900 carries the longest host AND the
    // longest duration, far outside the initially mounted window.
    type WRow = { id: string; host: string; ms: number | null; path: string };
    const wrows = (count: number): WRow[] =>
      Array.from({ length: count }, (_, i) => ({
        id: `w${i}`,
        host: i === 900 ? 'a-very-long-host-name.example.com' : `h${i % 7}.io`,
        ms: i === 900 ? 1234567 : i % 50,
        path: `/p/${i}`,
      }));
    const wcols: Column<WRow>[] = [
      { key: 'host', header: 'Host', fit: true, widthText: (r) => r.host, render: (r) => <span>{r.host}</span> },
      { key: 'path', header: 'Path', subject: true, titleValue: (r) => r.path, render: (r) => r.path },
      {
        key: 'ms',
        header: 'Duration',
        fit: true,
        align: 'right',
        // No widthText: falls back to the plain-string render.
        render: (r) => (r.ms != null ? `${r.ms} ms` : '—'),
      },
    ];

    it('is absent at or below the threshold (rendering unchanged)', () => {
      const { container } = render(
        <SortableTable columns={wcols} rows={wrows(500)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
      );
      expect(container.querySelectorAll('tbody')).toHaveLength(1);
      expect(mounted(container)).toHaveLength(500);
    });

    it('renders hidden, inert, zero-height probe rows holding each column\'s widest row', () => {
      const { container } = render(
        <SortableTable columns={wcols} rows={wrows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
      );
      // Row 900 is far outside the mounted window.
      expect(mounted(container).some((tr) => tr.textContent?.includes('a-very-long-host'))).toBe(false);
      const body = probe(container)!;
      expect(body).toBeDefined();
      expect(body).toHaveAttribute('aria-hidden', 'true');
      expect(body).toHaveAttribute('inert');
      expect(body.className).not.toMatch(/divide/);
      const trs = Array.from(body.querySelectorAll('tr'));
      expect(trs.length).toBeGreaterThan(0);
      expect(trs.length).toBeLessThanOrEqual(2 * 16);
      for (const tr of trs) {
        expect(tr.style.visibility).toBe('collapse');
        Array.from(tr.children).forEach((td, i) => {
          const cell = td as HTMLElement;
          expect(cell.style.visibility).toBe('hidden');
          expect(cell.className).toMatch(/\bpy-0\b/);
          expect(cell.className).toMatch(/\bpx-4\b/);
          if (i === 1) {
            // The subject column's probe cell is empty.
            expect(cell).toBeEmptyDOMElement();
          } else {
            const inner = cell.firstElementChild as HTMLElement;
            expect(inner.style.height).toBe('0px');
            expect(inner.style.overflow).toBe('visible');
          }
        });
      }
      // Same classes as the real cells of that column.
      expect(trs[0].children[0].className).toMatch(/w-\[1%\]/);
      expect(trs[0].children[2].className).toMatch(/text-right/);
      expect(probeTexts(container, 0)).toContain('a-very-long-host-name.example.com');
      expect(probeTexts(container, 2)).toContain('1234567 ms');
      // Probe rows are not accessible rows.
      expect(within(screen.getByRole('table')).queryByText('a-very-long-host-name.example.com')).toBeInTheDocument();
      expect(screen.getAllByRole('row').some((r) => r.textContent?.includes('a-very-long-host'))).toBe(false);
    });

    it('picks the probe rows once, in the measured fonts (no default-font pass first)', () => {
      // jsdom computes no font; give every element one so the table reads
      // real fonts from its mounted cells, as a browser does.
      const original = window.getComputedStyle.bind(window);
      vi.spyOn(window, 'getComputedStyle').mockImplementation((el, pseudo) => {
        const cs = original(el, pseudo);
        return new Proxy(cs, {
          get(target, prop) {
            if (prop === 'fontSize') return '13px';
            if (prop === 'fontFamily') return 'monospace';
            const v = Reflect.get(target, prop, target);
            return typeof v === 'function' ? v.bind(target) : v;
          },
        });
      });
      let widthTextCalls = 0;
      const counted = wcols.map((c) =>
        c.key === 'host'
          ? {
              ...c,
              widthText: (r: WRow) => {
                widthTextCalls++;
                return r.host;
              },
            }
          : c,
      );
      const { container } = render(
        <SortableTable columns={counted} rows={wrows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
      );
      // One selection pass reads each row's text once.
      expect(widthTextCalls).toBe(1000);
      expect(probeTexts(container, 0)).toContain('a-very-long-host-name.example.com');
      expect(probeTexts(container, 2)).toContain('1234567 ms');
    });

    it('does not change on scroll', async () => {
      const { container } = render(
        <SortableTable columns={wcols} rows={wrows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
      );
      const before = probe(container)!.innerHTML;
      await scrollTo(300);
      expect(mounted(container)[0].textContent).toContain(`/p/${300 - OVERSCAN_ROWS}`);
      expect(probe(container)!.innerHTML).toBe(before);
    });
  });

  describe('focus and row count', () => {
    const clickable = () => (
      <SortableTable
        columns={columns}
        rows={rows(1000)}
        rowKey={(r) => r.id}
        virtualize={{ threshold: 500 }}
        onRowClick={() => {}}
      />
    );

    it('keeps a focused row mounted (and focused) when it scrolls out of the window', async () => {
      const { container } = render(clickable());
      const row = mounted(container).find((tr) => tr.textContent?.startsWith('r5'))!;
      act(() => row.focus());
      expect(document.activeElement).toBe(row);
      await scrollTo(300);
      const now = mounted(container);
      expect(now).toContain(row);
      expect(document.activeElement).toBe(row);
      expect(row.getAttribute('aria-rowindex')).toBe('7');
      // Pinned at its true index: [spacer r0..r4] r5 [spacer r6..start) window [bottom].
      const start = 300 - OVERSCAN_ROWS;
      const sp = spacers(container).map((tr) => parseFloat(tr.style.height));
      expect(sp[0]).toBe(5 * ESTIMATED_ROW_PX);
      expect(sp[1]).toBe((start - 6) * ESTIMATED_ROW_PX);
      const body = container.querySelector('tbody')!;
      expect(body.children[1]).toBe(row);
      // Everything still adds up to the full table.
      const total = sp.reduce((a, b) => a + b, 0) + now.length * ESTIMATED_ROW_PX;
      expect(total).toBe(1000 * ESTIMATED_ROW_PX);
      // Blurring the row (the window keeps focus) releases the pin. jsdom
      // reports no document focus while <body> is active; a browser does.
      vi.spyOn(document, 'hasFocus').mockReturnValue(true);
      act(() => row.blur());
      await scrollTo(301);
      expect(mounted(container)).not.toContain(row);
    });

    it('keeps a focused row pinned below the window when scrolling back up', async () => {
      const { container } = render(clickable());
      await scrollTo(300);
      const row = mounted(container).find((tr) => tr.textContent?.startsWith('r310'))!;
      act(() => row.focus());
      await scrollTo(0);
      const now = mounted(container);
      expect(now[now.length - 1]).toBe(row);
      expect(document.activeElement).toBe(row);
      const sp = spacers(container).map((tr) => parseFloat(tr.style.height));
      const total = sp.reduce((a, b) => a + b, 0) + now.length * ESTIMATED_ROW_PX;
      expect(total).toBe(1000 * ESTIMATED_ROW_PX);
    });

    it('keeps the pin when the window (not the row) loses focus', async () => {
      const { container } = render(clickable());
      const row = mounted(container).find((tr) => tr.textContent?.startsWith('r5'))!;
      act(() => row.focus());
      // Window blur: focusout with no relatedTarget while the row is still
      // the document's active element (document focus reported, so the
      // active-element guard is what keeps the pin).
      vi.spyOn(document, 'hasFocus').mockReturnValue(true);
      act(() => {
        row.dispatchEvent(new FocusEvent('focusout', { bubbles: true, relatedTarget: null }));
      });
      expect(document.activeElement).toBe(row);
      await scrollTo(300);
      expect(mounted(container)).toContain(row);
    });

    it('sets aria-rowcount / aria-rowindex only when virtual', async () => {
      const { container, unmount } = render(clickable());
      expect(screen.getByRole('table')).toHaveAttribute('aria-rowcount', '1001');
      expect(container.querySelector('thead tr')).toHaveAttribute('aria-rowindex', '1');
      expect(mounted(container)[0]).toHaveAttribute('aria-rowindex', '2');
      await scrollTo(100);
      expect(mounted(container)[0]).toHaveAttribute('aria-rowindex', String(100 - OVERSCAN_ROWS + 2));
      unmount();
      vi.restoreAllMocks();

      const small = render(
        <SortableTable columns={columns} rows={rows(10)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
      );
      expect(screen.getByRole('table')).not.toHaveAttribute('aria-rowcount');
      for (const tr of Array.from(small.container.querySelectorAll('tr'))) {
        expect(tr).not.toHaveAttribute('aria-rowindex');
      }
    });
  });

  it('measures mounted rows synchronously even when ResizeObserver never calls back', () => {
    class SilentResizeObserver {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
    vi.stubGlobal('ResizeObserver', SilentResizeObserver);
    vi.spyOn(HTMLTableRowElement.prototype, 'getBoundingClientRect').mockReturnValue({
      top: 0, bottom: 42.5, left: 0, right: 0, width: 0, height: 42.5, x: 0, y: 0, toJSON: () => ({}),
    } as DOMRect);
    try {
      const { container } = render(
        <SortableTable columns={columns} rows={rows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
      );
      const n = mounted(container).length;
      expect(n).toBe(Math.ceil(window.innerHeight / 42.5) + OVERSCAN_ROWS);
      // Unmeasured rows use the average measured pitch, not the 41px estimate.
      expect(spacers(container)[0].style.height).toBe(`${(1000 - n) * 42.5}px`);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('re-sizes the spacers from observed row heights, without a scroll event', async () => {
    const observed = new Set<Element>();
    let fire: ((entries: unknown[]) => void) | undefined;
    class FakeResizeObserver {
      constructor(cb: (entries: unknown[]) => void) {
        fire = cb;
      }
      observe(el: Element) {
        observed.add(el);
      }
      unobserve(el: Element) {
        observed.delete(el);
      }
      disconnect() {
        observed.clear();
      }
    }
    vi.stubGlobal('ResizeObserver', FakeResizeObserver);
    try {
      const { container } = render(
        <SortableTable columns={columns} rows={rows(1000)} rowKey={(r) => r.id} virtualize={{ threshold: 500 }} />,
      );
      expect(observed.size).toBe(mounted(container).length);
      // Heights come from the row's bounding box (its pitch), not the RO
      // border box.
      vi.spyOn(HTMLTableRowElement.prototype, 'getBoundingClientRect').mockReturnValue({
        top: 0, bottom: 60, left: 0, right: 0, width: 0, height: 60, x: 0, y: 0, toJSON: () => ({}),
      } as DOMRect);
      await act(async () => {
        fire?.(Array.from(observed).map((target) => ({ target, borderBoxSize: [{ blockSize: 999 }] })));
      });
      const n = mounted(container).length;
      expect(n).toBeLessThan(Math.ceil(window.innerHeight / ESTIMATED_ROW_PX) + OVERSCAN_ROWS);
      expect(n).toBe(Math.ceil(window.innerHeight / 60) + OVERSCAN_ROWS);
      expect(spacers(container)[0].style.height).toBe(`${(1000 - n) * 60}px`);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
