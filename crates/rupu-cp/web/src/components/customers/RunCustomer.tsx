// How a run's customer shows on a run: the header chip on the run page and the
// table cell on the runs lists. A row's `customer` is tri-state — a slug,
// `null` (no customer) or ABSENT (the host/record can't say; never "none") —
// and `derived` marks an attribution taken from the project's current customer.

import { useEffect, useRef, useState } from 'react';
import { api, type CustomerRef, type CustomerRow } from '../../lib/api';
import { useCustomerDirectoryState } from '../../lib/customerScope';
import { CustomerChip, DERIVED_CUSTOMER_TITLE, UNKNOWN_CUSTOMER_TITLE } from './CustomerChip';
import type { Column } from '../lists/SortableTable';
import { CustomerDot } from './CustomerDot';

/** The customer's ref from the active list; a slug the list doesn't hold (an
 *  archived or since-deleted customer) gets a neutral dot — `currentColor`,
 *  never a hard-coded color — and its slug as the name. */
export function resolveCustomerRef(slug: string, customers: CustomerRow[]): CustomerRef {
  const hit = customers.find((c) => c.slug === slug);
  if (hit) return { slug: hit.slug, name: hit.name, tint: hit.tint, archived: hit.archived };
  return { slug, name: slug, tint: { light: 'currentColor', dark: 'currentColor' }, archived: false };
}

/** The customers a run surface can name: the active ones plus — loaded ONCE,
 *  lazily, the first time a slug in `slugs` is not an active customer — the
 *  archived ones (a run keeps the customer it recorded after it is archived).
 *  A slug in neither list stays unresolved (a deleted customer). */
export function useRunCustomers(slugs: (string | null | undefined)[]): CustomerRow[] {
  const { customers: active, loaded } = useCustomerDirectoryState();
  const [archived, setArchived] = useState<CustomerRow[]>([]);
  const asked = useRef(false);
  const unresolved = slugs.some((s) => typeof s === 'string' && !active.some((c) => c.slug === s));
  useEffect(() => {
    if (!loaded || !unresolved || asked.current) return;
    asked.current = true;
    api.getCustomers({ archived: true }).then(
      (rows) => setArchived(rows.filter((r) => r.archived)),
      () => {
        // Can't tell: such a slug keeps showing by its slug.
      },
    );
  }, [loaded, unresolved]);
  return archived.length === 0 ? active : [...active, ...archived.filter((a) => !active.some((c) => c.slug === a.slug))];
}

/** The run page's header chip. With no customers defined, a run with none (or
 *  an unknowable one) shows nothing — the feature is not in play. */
export function RunCustomerChip({
  customer,
  derived,
}: {
  customer: string | null | undefined;
  derived?: boolean;
}) {
  const customers = useRunCustomers([customer]);
  if (typeof customer === 'string') {
    return <CustomerChip customer={resolveCustomerRef(customer, customers)} derived={derived} />;
  }
  if (customers.length === 0) return null;
  return <CustomerChip customer={null} unknown={customer === undefined} />;
}

/** A runs-table cell: dot + name (muted, with a tooltip, when derived); "—"
 *  for none; a muted "Unknown" when the row can't say. */
export function RunCustomerCell({
  customer,
  derived,
  customers,
}: {
  customer: string | null | undefined;
  derived?: boolean;
  customers: CustomerRow[];
}) {
  if (customer === undefined) {
    return (
      <span title={UNKNOWN_CUSTOMER_TITLE} className="text-note text-ink-mute">
        Unknown
      </span>
    );
  }
  if (customer === null) return <span className="text-ink-mute">—</span>;
  const ref = resolveCustomerRef(customer, customers);
  return (
    <span
      title={derived ? DERIVED_CUSTOMER_TITLE : undefined}
      className={`inline-flex items-center gap-1.5 text-sm ${derived ? 'text-ink-dim' : 'text-ink'}`}
    >
      <CustomerDot tint={ref.tint} size={7} />
      {ref.name}
    </span>
  );
}

/** The sort key of a customer column: the display name, rows that can't say
 *  and rows with none last. */
export function customerSortValue(
  customer: string | null | undefined,
  customers: CustomerRow[],
): string | null {
  if (typeof customer !== 'string') return null;
  return resolveCustomerRef(customer, customers).name.toLowerCase();
}

/** The runs tables' Customer column — placed right after Host. Add it only
 *  while the list is NOT scoped to a customer (when scoped, every row is that
 *  customer's) and the feature is in play (customers exist — archived ones
 *  included — or a listed row names one); otherwise `columns` is returned as is. */
export function withCustomerColumn<T extends { customer?: string | null; customer_derived?: boolean }>(
  columns: Column<T>[],
  customers: CustomerRow[],
  rows: T[],
  show: boolean,
): Column<T>[] {
  if (!show || (customers.length === 0 && !rows.some((r) => typeof r.customer === 'string'))) return columns;
  const col: Column<T> = {
    key: 'customer',
    header: 'Customer',
    fit: true,
    sortable: true,
    sortValue: (r) => customerSortValue(r.customer, customers),
    render: (r) => <RunCustomerCell customer={r.customer} derived={r.customer_derived} customers={customers} />,
  };
  const at = columns.findIndex((c) => c.key === 'host');
  return at < 0 ? [...columns, col] : [...columns.slice(0, at + 1), col, ...columns.slice(at + 1)];
}
