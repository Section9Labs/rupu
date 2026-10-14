// Customers — who the projects belong to. A list of customers with rollups
// over a range (cost, runs, open findings), each opening its detail page, and
// a footer that points at the projects no customer owns (they run on the
// global config). One query filters it: `is:` picks active (the default) or
// archived customers, and `range:` is the rollup window the server folds.

import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { Lock } from 'lucide-react';
import {
  api,
  apiErrorMessage,
  type CustomerRow,
  type ProjectRow,
} from '../../lib/api';
import { useCustomerScope } from '../../lib/customerScope';
import { formatCost } from '../../lib/usage';
import { relativeTime } from '../../lib/time';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import { Button } from '../../components/ui/Button';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { Spinner } from '../../components/ui/Spinner';
import { CustomerDot } from '../../components/customers/CustomerDot';
import { CustomerFormDialog } from '../../components/customers/CustomerFormDialog';
import { PricingErrorMark } from '../../components/customers/PricingErrorMark';
import { RollupHostsWithoutCustomerBanner } from '../../components/customers/HostsWithoutCustomerBanner';
import { StatTile as Tile } from '../../components/customers/StatTile';
import { ListQueryBar, NoMatches, useListQuery } from '../../components/query/ListQueryBar';
import { filterRows, rowFacets } from '../../lib/query/rows';
import { CUSTOMER_FIELDS, customersRange, customersRowQuery } from '../../lib/query/lists/customers';

function costOf(r: CustomerRow): number {
  return r.rollup.usage.cost_usd ?? 0;
}

const COLUMNS: Column<CustomerRow>[] = [
  {
    key: 'customer',
    header: 'Customer',
    subject: true,
    sortable: true,
    sortValue: (r) => r.name,
    titleValue: (r) => `${r.name} (${r.slug})`,
    render: (r) => (
      <Link
        to={`/customers/${encodeURIComponent(r.slug)}`}
        className="inline-flex items-center gap-2 max-w-full hover:underline"
      >
        <CustomerDot tint={r.tint} size={9} />
        <span className="text-sm font-semibold text-ink truncate">{r.name}</span>
        <span className="text-note text-ink-mute font-mono truncate">{r.slug}</span>
        {r.archived && <span className="text-meta uppercase text-ink-mute">archived</span>}
      </Link>
    ),
  },
  {
    key: 'account',
    header: 'Default account',
    fit: true,
    sortable: true,
    sortValue: (r) => r.default_account?.account ?? null,
    render: (r) => {
      if (r.layer_error) {
        return (
          <span className="text-note text-ink-mute" title={r.layer_error}>
            layer error
          </span>
        );
      }
      const a = r.default_account;
      if (!a) return <span className="text-ink-mute">—</span>;
      return (
        <span className="inline-flex items-center gap-1.5">
          <span className="inline-flex items-center gap-1 rounded border border-border bg-surface px-1.5 py-0.5 font-mono text-note text-ink">
            {a.account}
            {a.locked_by && (
              <span title={`Locked by ${a.locked_by} policy`} className="inline-flex">
                <Lock size={10} className="text-ink-mute" role="img" aria-label={`Locked by ${a.locked_by}`} />
              </span>
            )}
          </span>
          {a.inherited && <span className="text-note text-ink-mute">inherits global</span>}
        </span>
      );
    },
  },
  {
    key: 'projects',
    header: 'Projects',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.rollup.projects,
    render: (r) => <span className="text-ink">{r.rollup.projects}</span>,
  },
  {
    key: 'runs',
    header: 'Runs',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.rollup.run_count,
    render: (r) => <span className="text-ink">{r.rollup.run_count}</span>,
  },
  {
    key: 'cost',
    header: 'Cost',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.rollup.usage.cost_usd,
    render: (r) => (
      <span className="inline-flex items-center justify-end gap-1 text-ink font-medium">
        <PricingErrorMark error={r.rollup.usage.pricing_error} />
        {formatCost(r.rollup.usage.cost_usd)}
      </span>
    ),
  },
  {
    key: 'findings',
    header: 'Open findings',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.rollup.findings_open,
    render: (r) => (
      <span className={r.rollup.findings_open > 0 ? 'text-err font-medium' : 'text-ink-mute'}>
        {r.rollup.findings_open}
      </span>
    ),
  },
  {
    key: 'last_active',
    header: 'Last active',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => (r.rollup.last_active ? Date.parse(r.rollup.last_active) : null),
    render: (r) => (
      <span className="text-ink-mute">{r.rollup.last_active ? relativeTime(r.rollup.last_active) : 'no runs'}</span>
    ),
  },
];

export default function Customers() {
  const { setScope } = useCustomerScope();
  const [q, setQ] = useListQuery();
  const { range, rest, error: rangeError } = useMemo(() => customersRange(q), [q]);
  const rowQuery = customersRowQuery(rest);
  const [rows, setRows] = useState<CustomerRow[] | null>(null);
  // True from the start of a fetch until it settles — rows from another range
  // are never shown while the next ones load.
  const [loading, setLoading] = useState(true);
  const [loadedOnce, setLoadedOnce] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [projects, setProjects] = useState<ProjectRow[] | null>(null);
  const [reloadNonce, setReloadNonce] = useState(0);
  const [creating, setCreating] = useState(false);
  const headerRef = useRef<HTMLElement>(null);

  // One fetch per range: `archived=1` returns active AND archived customers,
  // and `is:` picks among them here.
  useEffect(() => {
    let cancelled = false;
    setError(null);
    setLoading(true);
    api.getCustomers({ archived: true, range }).then(
      (data) => {
        if (cancelled) return;
        setRows(data);
        setLoading(false);
        setLoadedOnce(true);
      },
      (e: unknown) => {
        if (cancelled) return;
        setRows(null);
        setError(apiErrorMessage(e));
        setLoading(false);
        setLoadedOnce(true);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [range, reloadNonce]);

  // Totals for the "projects assigned" tile and the footer. Best effort: a
  // failure leaves the tile at "—" rather than blocking the page.
  useEffect(() => {
    let cancelled = false;
    api.getProjects().then(
      (data) => {
        if (!cancelled) setProjects(data);
      },
      () => {
        if (!cancelled) setProjects(null);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [reloadNonce]);

  const archivedTotal = (rows ?? []).filter((r) => r.archived).length;
  const activeTotal = (rows ?? []).length - archivedTotal;
  const filtered = useMemo(() => filterRows(rows ?? [], rowQuery, CUSTOMER_FIELDS), [rows, rowQuery]);
  const visible = filtered.rows;
  const facets = useMemo(() => rowFacets(rows ?? [], CUSTOMER_FIELDS), [rows]);
  // With no `is:` the list is the active customers, so they are the whole.
  const total = rowQuery === rest ? (rows ?? []).length : activeTotal;
  // The tiles sum what the query shows.
  const stats = useMemo(() => {
    const totalCost = visible.reduce((s, r) => s + costOf(r), 0);
    const top = visible.reduce<CustomerRow | null>((t, r) => (t === null || costOf(r) > costOf(t) ? r : t), null);
    const anyCost = visible.some((r) => r.rollup.usage.cost_usd !== null);
    // Unpriced work is left OUT of the headline total; a partial fold (some usage
    // unreadable — it can also report `runs: 0`) is IN it but understated.
    const unpriced = visible.filter((r) => r.rollup.usage.runs > 0 && !r.rollup.usage.priced);
    const partial = visible.filter((r) => r.rollup.usage.partial === true);
    const priceError = visible.find((r) => r.rollup.usage.pricing_error)?.rollup.usage.pricing_error;
    const withFindings = visible.filter((r) => r.rollup.findings_open > 0).length;
    const findings = visible.reduce((s, r) => s + r.rollup.findings_open, 0);
    const hosts = [...new Set(visible.flatMap((r) => r.rollup.hosts_without_customer ?? []))].sort();
    return { totalCost, top, anyCost, unpriced, partial, priceError, withFindings, findings, hosts };
  }, [visible]);

  // null `customer` = no customer; an ABSENT key can't say, so it is neither.
  const assigned = projects?.filter((p) => p.customer).length ?? 0;
  const unassigned = projects?.filter((p) => p.customer === null).length ?? 0;
  const topShare =
    stats.top && stats.totalCost > 0 ? Math.round((costOf(stats.top) / stats.totalCost) * 100) : null;
  const who = (rows: CustomerRow[]) => (rows.length === 1 ? rows[0].name : `${rows.length} customers`);
  const excludes =
    stats.unpriced.length > 0 ? `excludes unpriced usage from ${who(stats.unpriced)}` : null;
  const understated = stats.partial.length > 0 ? 'some usage unreadable — may be understated' : null;
  const costSub =
    excludes && understated
      ? `${excludes}; ${understated}`
      : (excludes ??
        understated ??
        (topShare !== null && stats.top ? `${topShare}% from ${stats.top.name}` : undefined));
  const rangeLabel = range === 'all' ? 'all time' : range;

  function closeDialog() {
    setCreating(false);
    // Hand focus back to the trigger once the dialog is gone.
    setTimeout(() => headerRef.current?.querySelector('button')?.focus(), 0);
  }

  return (
    <div className="p-8">
      <header ref={headerRef} className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Customers</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Who your projects belong to — each customer has its own config layer, accounts and spend.
          </p>
        </div>
        <Button onClick={() => setCreating(true)}>
          New customer
        </Button>
      </header>

      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {loadedOnce && (
        <ListQueryBar
          value={q}
          onChange={setQ}
          fields={CUSTOMER_FIELDS}
          facets={facets}
          total={total}
          shown={visible.length}
          error={rangeError ?? filtered.error}
          label="Filter customers"
          placeholder="Filter customers… e.g. is:archived range:7d has:findings"
        />
      )}

      {loading && (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading customers…" />
        </div>
      )}

      {!loading && rows !== null && (
        <>
          <div className="mb-4 grid grid-cols-2 gap-3 lg:grid-cols-4">
            <Tile
              id="customers"
              label="Customers"
              value={activeTotal}
              sub={`${archivedTotal} archived`}
            />
            <Tile
              id="assigned"
              label="Projects assigned"
              value={projects ? `${assigned} / ${projects.length}` : '—'}
              sub={projects && unassigned > 0 ? `${unassigned} unassigned` : undefined}
              subTone="warn"
            />
            <Tile
              id="cost"
              label={`Cost · ${rangeLabel}`}
              value={
                <span className="inline-flex items-center gap-1.5">
                  {stats.anyCost ? formatCost(stats.totalCost) : '—'}
                  <PricingErrorMark error={stats.priceError} />
                </span>
              }
              sub={costSub}
              subTone={stats.unpriced.length > 0 || stats.partial.length > 0 ? 'warn' : undefined}
            />
            <Tile
              id="findings"
              label="Open findings"
              value={stats.findings}
              sub={
                stats.findings > 0
                  ? `across ${stats.withFindings} ${stats.withFindings === 1 ? 'customer' : 'customers'}`
                  : undefined
              }
            />
          </div>

          {rows.length === 0 ? (
            <EmptyState
              title="No customers yet"
              hint="Create a customer, then assign projects to it to give them their own config layer, accounts and spend."
              action={<Button onClick={() => setCreating(true)}>New customer</Button>}
            />
          ) : visible.length === 0 ? (
            rest.trim() ? <NoMatches noun="customers" /> : <EmptyState title="No active customers" />
          ) : (
            <SortableTable<CustomerRow>
              columns={COLUMNS}
              rows={visible}
              rowKey={(r) => r.slug}
              initialSort={{ key: 'last_active', dir: 'desc' }}
            />
          )}

          <RollupHostsWithoutCustomerBanner className="mt-3" without={stats.hosts} />
        </>
      )}

      {/* Whatever the table shows (or doesn't), the projects no customer owns
          stay one click away. */}
      {!loading && unassigned > 0 && (
        <div className="mt-3 flex items-center justify-between gap-3 rounded-xl border border-border bg-panel px-4 py-2.5 text-note">
          <span className="text-ink-dim">
            {unassigned} {unassigned === 1 ? 'project has' : 'projects have'} no customer — they run on the
            global config.
          </span>
          <Link
            to="/projects?customer=none"
            onClick={() => setScope('none')}
            className="font-medium text-brand-600 hover:text-brand-700 hover:underline"
          >
            Review unassigned →
          </Link>
        </div>
      )}

      {creating && (
        <CustomerFormDialog mode="create" onSaved={() => { setReloadNonce((n) => n + 1); closeDialog(); }} onClose={closeDialog} />
      )}
    </div>
  );
}
