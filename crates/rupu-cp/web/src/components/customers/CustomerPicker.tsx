// The customer scope picker — a trigger button (dot + scope name + chevrons)
// and a popover with a search box, the customer rows, "Unassigned", and a
// footer ("Show archived", "Manage →"). `sidebar` is the v1 rail block under
// the brand (with its CUSTOMER label); `compact` is the v2 top-bar button.
// Reads and writes the scope through `useCustomerScope()`.

import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { Check, ChevronsUpDown, Search } from 'lucide-react';
import { api, type CustomerRow } from '../../lib/api';
import { cn } from '../../lib/cn';
import { useCustomerScope } from '../../lib/customerScope';
import { CustomerDot } from './CustomerDot';

type Variant = 'sidebar' | 'compact';

type Item =
  | { kind: 'all'; key: string; label: string }
  | { kind: 'none'; key: string; label: string }
  | { kind: 'customer'; key: string; label: string; row: CustomerRow; muted: boolean };

const NEUTRAL_DOT = 'inline-block h-2 w-2 shrink-0 rounded-full border border-ink-mute';

export function CustomerPicker({ variant = 'sidebar' }: { variant?: Variant }) {
  const { scope, customer, customers, setScope } = useCustomerScope();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [showArchived, setShowArchived] = useState(false);
  const [archived, setArchived] = useState<CustomerRow[] | null>(null);
  const [archivedError, setArchivedError] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  // Archived customers load lazily, afresh each time the box is ticked.
  useEffect(() => {
    if (!showArchived) return;
    let cancelled = false;
    setArchivedError(false);
    api.getCustomers({ archived: true }).then(
      (rows) => {
        if (cancelled) return;
        setArchived(rows.filter((r) => r.archived));
        setArchivedError(false);
      },
      () => {
        if (!cancelled) setArchivedError(true);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [showArchived]);

  // Click outside closes (focus stays wherever the user clicked).
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  const items = useMemo<Item[]>(() => {
    const q = query.trim().toLowerCase();
    const matches = (s: string) => q === '' || s.toLowerCase().includes(q);
    const out: Item[] = [];
    if (matches('All customers')) {
      out.push({
        kind: 'all',
        key: 'all',
        label: 'All customers',
      });
    }
    for (const c of customers) {
      if (matches(c.name) || matches(c.slug)) {
        out.push({ kind: 'customer', key: c.slug, label: c.name, row: c, muted: false });
      }
    }
    if (showArchived) {
      for (const c of archived ?? []) {
        if (customers.some((a) => a.slug === c.slug)) continue;
        if (matches(c.name) || matches(c.slug)) {
          out.push({ kind: 'customer', key: c.slug, label: c.name, row: c, muted: true });
        }
      }
    }
    if (matches('Unassigned')) out.push({ kind: 'none', key: 'none', label: 'Unassigned' });
    return out;
  }, [query, customers, archived, showArchived]);

  const activeIdx = Math.max(0, Math.min(active, items.length - 1));
  const rowId = (i: number) => `${menuId}-row-${i}`;

  // Keep the active row in view while arrowing through a long list.
  useEffect(() => {
    if (!open) return;
    document.getElementById(rowId(activeIdx))?.scrollIntoView?.({ block: 'nearest' });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, activeIdx, items.length]);

  function close(returnFocus: boolean) {
    setOpen(false);
    setQuery('');
    setActive(0);
    if (returnFocus) triggerRef.current?.focus();
  }

  function pick(item: Item) {
    if (item.kind === 'all') setScope(null);
    else if (item.kind === 'none') setScope('none');
    else setScope(item.row.slug, item.row);
    close(true);
  }

  // On the popover container, so Escape and the arrows work from any element
  // inside it. Enter only picks from the search box — on a row, the checkbox
  // or the link it keeps its native action.
  function onPopoverKey(e: React.KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      close(true);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive(Math.max(0, Math.min(activeIdx + 1, items.length - 1)));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive(Math.max(activeIdx - 1, 0));
    } else if (e.key === 'Enter' && e.target === searchRef.current) {
      e.preventDefault();
      const item = items[activeIdx];
      if (item) pick(item);
    }
  }

  // Focus moving to an element outside the picker closes it.
  function onRootBlur(e: React.FocusEvent) {
    const next = e.relatedTarget as Node | null;
    if (open && next && rootRef.current && !rootRef.current.contains(next)) close(false);
  }

  const triggerName =
    scope === null ? 'All customers' : scope === 'none' ? 'Unassigned' : (customer?.name ?? scope);
  const scoped = scope !== null;

  const trigger = (
    <button
      ref={triggerRef}
      type="button"
      aria-label={`Customer scope: ${triggerName}`}
      aria-haspopup="menu"
      aria-expanded={open}
      aria-controls={open ? menuId : undefined}
      onClick={() => {
        setOpen((o) => !o);
        setQuery('');
        setActive(0);
      }}
      className={cn(
        'flex items-center gap-2 rounded-md border bg-surface text-left',
        variant === 'sidebar' ? 'w-full px-2.5 py-2 text-sm' : 'h-[26px] px-2 text-[11px]',
        scoped ? 'border-brand-500/40 bg-brand-50 text-brand-700' : 'border-border text-ink',
      )}
    >
      {scope !== null && scope !== 'none' && customer ? (
        <CustomerDot tint={customer.tint} />
      ) : (
        <span aria-hidden className={NEUTRAL_DOT} />
      )}
      <span className="min-w-0 flex-1 truncate">{triggerName}</span>
      <ChevronsUpDown size={variant === 'sidebar' ? 14 : 12} aria-hidden className="shrink-0 text-ink-mute" />
    </button>
  );

  return (
    <div ref={rootRef} onBlur={onRootBlur} className={cn('relative', variant === 'sidebar' && 'px-3 pt-3')}>
      {variant === 'sidebar' && (
        <div className="mb-1 text-meta font-medium uppercase tracking-wide text-ink-mute">Customer</div>
      )}
      {trigger}
      {open && (
        <div
          onKeyDown={onPopoverKey}
          className={cn(
            'absolute z-30 mt-1 w-64 rounded-md border border-border bg-panel shadow-lg',
            variant === 'sidebar' ? 'left-3' : 'left-0',
          )}
        >
          <div className="flex items-center gap-2 border-b border-border px-2.5 py-2">
            <Search size={13} aria-hidden className="text-ink-mute" />
            <input
              ref={searchRef}
              autoFocus
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setActive(0);
              }}
              aria-activedescendant={items.length > 0 ? rowId(activeIdx) : undefined}
              placeholder="Find a customer…"
              aria-label="Find a customer"
              className="w-full bg-transparent text-sm text-ink outline-none placeholder:text-ink-mute"
            />
          </div>
          <div id={menuId} role="menu" aria-label="Customers" className="max-h-64 overflow-y-auto py-1">
            {items.map((item, i) => {
              const current =
                item.kind === 'all'
                  ? scope === null
                  : item.kind === 'none'
                    ? scope === 'none'
                    : scope === item.row.slug;
              return (
                <button
                  key={item.key}
                  id={rowId(i)}
                  type="button"
                  role="menuitem"
                  aria-current={current ? 'true' : undefined}
                  data-active={i === activeIdx ? 'true' : undefined}
                  onClick={() => pick(item)}
                  onMouseEnter={() => setActive(i)}
                  className={cn(
                    'flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-sm',
                    i === activeIdx && 'bg-surface-hover',
                    item.kind === 'customer' && item.muted ? 'text-ink-mute' : 'text-ink',
                  )}
                >
                  {item.kind === 'customer' ? (
                    <CustomerDot tint={item.row.tint} />
                  ) : (
                    <span aria-hidden className={NEUTRAL_DOT} />
                  )}
                  <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  {item.kind === 'customer' && (
                    <span className="font-mono text-[11px] text-ink-mute">{item.row.rollup.projects}</span>
                  )}
                  {current && <Check size={13} aria-hidden className="text-brand-600" />}
                </button>
              );
            })}
            {items.length === 0 && <div className="px-2.5 py-2 text-sm text-ink-mute">No matching customer.</div>}
          </div>
          <div className="flex items-center justify-between border-t border-border px-2.5 py-2 text-[12px]">
            <label className="flex items-center gap-1.5 text-ink-dim">
              <input
                type="checkbox"
                checked={showArchived}
                onChange={(e) => setShowArchived(e.target.checked)}
              />
              Show archived
            </label>
            <Link to="/customers" onClick={() => close(false)} className="text-brand-600 hover:underline">
              Manage →
            </Link>
          </div>
          {showArchived && archivedError && (
            <div className="border-t border-border px-2.5 py-1.5 text-[12px] text-status-failed">
              Couldn’t load archived customers.
            </div>
          )}
        </div>
      )}
    </div>
  );
}
