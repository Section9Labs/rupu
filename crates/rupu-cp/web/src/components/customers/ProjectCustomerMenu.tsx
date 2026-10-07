// ProjectCustomerMenu — the project header's customer chip and the "assign to
// customer" menu it opens (mockup artboard "Project — assign to customer").
//
// The trigger is the project's CustomerChip (a dashed "No customer" when none;
// the muted "Unknown customer" when the CP can't say). The menu lists the
// ACTIVE customers (dot, name, mono default account), then "Unassign" (when
// assigned) and "New customer…" (creates, then assigns). Focusing or hovering
// a customer shows what the assignment would change, built from its
// `default_account` and — read lazily from `getCustomerConfig`, and only when
// the customer's own layer sets `scm.rules` — its SCM routing. Picking assigns
// at once (the PUT replaces any earlier assignment); a refusal (409 archived,
// 404, …) is shown inline and nothing changes. "Already unassigned" (404) is
// not a failure: the chip is reconciled. Escape closes and returns focus to
// the trigger; the arrows move between items.

import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import {
  api,
  apiErrorMessage,
  ApiError,
  type ConfigView,
  type CustomerDto,
  type CustomerRef,
  type CustomerRow,
} from '../../lib/api';
import { cn } from '../../lib/cn';
import { useCustomerScope } from '../../lib/customerScope';
import { CustomerChip } from './CustomerChip';
import { CustomerDot } from './CustomerDot';
import { CustomerFormDialog } from './CustomerFormDialog';

export interface ProjectCustomerMenuProps {
  wsId: string;
  /** The project's customer: a ref, `null` = none, `undefined` = the CP can't say. */
  customer: CustomerRef | null | undefined;
  /** The project's new customer after an assign / unassign (`null` = none). */
  onChange: (customer: CustomerRef | null) => void;
}

function refOf(c: Pick<CustomerDto, 'slug' | 'name' | 'tint' | 'archived'>): CustomerRef {
  return { slug: c.slug, name: c.name, tint: c.tint, archived: c.archived };
}

interface ScmRoute {
  owner: string;
  account: string;
}

/** The customer layer's own `scm.rules` — empty unless the customer's layer
 *  sets them (otherwise the effective rules are the global ones). */
function customerScmRoutes(view: ConfigView): ScmRoute[] {
  const scm = view.effective?.scm as { rules?: unknown } | undefined;
  const rules = scm?.rules;
  if (!Array.isArray(rules)) return [];
  // Only the provenance says whose rules these are (keys under `scm.rules`
  // resolved from the customer layer) — never a guess from the raw TOML.
  const own = Object.entries(view.provenance ?? {}).some(
    ([key, p]) => /^scm\.rules(\.|\[|$)/.test(key) && p.source === 'customer',
  );
  if (!own) return [];
  const out: ScmRoute[] = [];
  for (const r of rules) {
    const rule = r as { owner?: unknown; account?: unknown };
    if (typeof rule.owner === 'string' && typeof rule.account === 'string') {
      out.push({ owner: rule.owner, account: rule.account });
    }
  }
  return out;
}

function accountTag(a: NonNullable<CustomerRow['default_account']>): string {
  if (a.locked_by) return 'locked';
  return a.inherited ? 'inherits global' : 'customer default';
}

/** What assigning the project to `c` changes — a pure function of the row and
 *  (when loaded) its SCM routes. */
export function assignPreview(c: CustomerRow, routes: ScmRoute[]): string {
  const history = 'Runs already finished keep their history.';
  if (c.layer_error) {
    return `${c.name}’s config layer doesn’t resolve (${c.layer_error}) — runs in this project would fail until it is fixed. ${history}`;
  }
  const a = c.default_account;
  let text = a
    ? `Assigning to ${c.name} switches this project’s runs to ${a.account} (${accountTag(a)})`
    : `Assigning to ${c.name} switches this project’s runs to its config layer`;
  if (routes.length > 0) {
    const list = routes
      .map((r) => `${r.owner.endsWith('*') ? r.owner : `${r.owner}/*`} repos to ${r.account}`)
      .join(', ');
    text += ` and routes ${list}`;
  }
  return `${text}. ${history}`;
}

export function ProjectCustomerMenu({ wsId, customer, onChange }: ProjectCustomerMenuProps) {
  const { customers, reload } = useCustomerScope();
  const [open, setOpen] = useState(false);
  const [previewSlug, setPreviewSlug] = useState<string | null>(null);
  const [configs, setConfigs] = useState<Record<string, ScmRoute[]>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const requested = useRef(new Set<string>());
  const menuId = useId();
  const noteId = useId();

  const assigned = customer ?? null;

  const items = useCallback(
    () => Array.from(menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"], [role="menuitemradio"]') ?? []),
    [],
  );

  function close(returnFocus: boolean) {
    setOpen(false);
    setPreviewSlug(null);
    if (returnFocus) triggerRef.current?.focus();
  }

  // Opening focuses the current customer (else the first item).
  useEffect(() => {
    if (!open) return;
    const all = items();
    (all.find((el) => el.getAttribute('aria-checked') === 'true') ?? all[0])?.focus();
  }, [open, items]);

  // After a refusal the busy items were disabled (focus fell to <body>): put
  // focus back on the menu so Escape still reaches it.
  useEffect(() => {
    if (!open || busy || !error) return;
    const all = items();
    (all.find((el) => el.getAttribute('aria-checked') === 'true') ?? all[0])?.focus();
  }, [open, busy, error, items]);

  // Click outside closes.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  function loadRoutes(slug: string) {
    if (requested.current.has(slug)) return;
    requested.current.add(slug);
    api.getCustomerConfig(slug).then(
      (view) => setConfigs((m) => ({ ...m, [slug]: customerScmRoutes(view) })),
      () => {
        // Not loaded — the preview simply omits SCM routing.
      },
    );
  }

  // The shown preview follows the focused customer and picks up its SCM
  // routes when they arrive.
  const previewRow = customers.find((c) => c.slug === previewSlug);
  const previewText = previewRow ? assignPreview(previewRow, configs[previewRow.slug] ?? []) : null;

  function focusCustomer(c: CustomerRow) {
    setPreviewSlug(c.slug);
    loadRoutes(c.slug);
  }
  /** The preview follows keyboard focus when it is on a customer, else clears. */
  function restorePreview() {
    const f = document.activeElement as HTMLElement | null;
    setPreviewSlug(f && menuRef.current?.contains(f) ? (f.dataset.slug ?? null) : null);
  }
  function clearPreview() {
    setPreviewSlug(null);
  }

  async function assign(slug: string) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const row = await api.assignProject(slug, wsId);
      const known = customers.find((c) => c.slug === slug);
      const next = row.customer ?? (known ? refOf(known) : null);
      onChange(next);
      reload();
      close(true);
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function unassign() {
    if (busy || !assigned) return;
    setBusy(true);
    setError(null);
    try {
      await api.unassignProject(assigned.slug, wsId);
      onChange(null);
      reload();
      close(true);
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 404) {
        // Not assigned to that slug any more — somebody else got there first.
        onChange(null);
        reload();
        setError('Already unassigned');
      } else {
        setError(apiErrorMessage(e));
      }
    } finally {
      setBusy(false);
    }
  }

  async function created(dto: CustomerDto) {
    setCreating(false);
    try {
      const row = await api.assignProject(dto.slug, wsId);
      onChange(row.customer ?? refOf(dto));
      reload();
      close(true);
    } catch (e: unknown) {
      // The customer exists; only the assignment failed.
      reload();
      setError(`${dto.name} was created, but assigning failed: ${apiErrorMessage(e)}`);
      setOpen(true);
    }
  }

  function onMenuKey(e: React.KeyboardEvent) {
    const all = items();
    const at = all.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      all[(at + 1) % all.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      all[at <= 0 ? all.length - 1 : at - 1]?.focus();
    } else if (e.key === 'Home') {
      e.preventDefault();
      all[0]?.focus();
    } else if (e.key === 'End') {
      e.preventDefault();
      all[all.length - 1]?.focus();
    } else if (e.key === 'Tab') {
      close(false);
    }
  }

  const triggerLabel = customer === undefined
    ? 'Customer unknown — assign to a customer'
    : assigned
      ? `Customer: ${assigned.name} — change`
      : 'No customer — assign to a customer';

  return (
    <div ref={rootRef} className="relative inline-flex">
      <button
        ref={triggerRef}
        type="button"
        aria-label={triggerLabel}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => {
          setOpen((o) => !o);
          setError(null);
        }}
        className="inline-flex items-center gap-1 rounded-full focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500"
      >
        <CustomerChip customer={assigned} unknown={customer === undefined} />
        <ChevronDown size={12} aria-hidden className="text-ink-mute" />
      </button>

      {open && (
        <div
          onKeyDown={onMenuKey}
          className="absolute left-0 top-full z-30 mt-1 w-80 rounded-lg border border-border bg-panel py-1 shadow-lg"
        >
          <div
            ref={menuRef}
            id={menuId}
            role="menu"
            aria-label="Assign to customer"
            aria-describedby={noteId}
          >
            <div
              aria-hidden
              className="px-3 pb-1 pt-1.5 text-meta font-semibold uppercase tracking-wide text-ink-mute"
            >
              Assign to customer
            </div>
            {customers.length === 0 && (
              <div className="px-3 py-2 text-note text-ink-mute">No customers yet.</div>
            )}
            {customers.map((c) => {
              const current = assigned?.slug === c.slug;
              return (
                <button
                  key={c.slug}
                  type="button"
                  role="menuitemradio"
                  aria-checked={current}
                  disabled={busy}
                  data-slug={c.slug}
                  onClick={() => void assign(c.slug)}
                  onFocus={() => focusCustomer(c)}
                  onMouseEnter={() => focusCustomer(c)}
                  onMouseLeave={restorePreview}
                  className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm text-ink hover:bg-surface-hover focus:bg-surface-hover focus:outline-none disabled:opacity-60"
                >
                  <CustomerDot tint={c.tint} />
                  <span className="min-w-0 flex-1 truncate">{c.name}</span>
                  {c.default_account && (
                    <span className="font-mono text-note text-ink-mute">{c.default_account.account}</span>
                  )}
                  {current && <Check size={13} aria-hidden className="text-brand-600" />}
                </button>
              );
            })}
            <div role="separator" className="my-1 border-t border-border" />
            {assigned && (
              <button
                type="button"
                role="menuitem"
                disabled={busy}
                onClick={() => void unassign()}
                onFocus={clearPreview}
                className="flex w-full items-center px-3 py-1.5 text-left text-sm text-ink hover:bg-surface-hover focus:bg-surface-hover focus:outline-none disabled:opacity-60"
              >
                Unassign
              </button>
            )}
            <button
              type="button"
              role="menuitem"
              disabled={busy}
              onClick={() => {
                setCreating(true);
                setOpen(false);
              }}
              onFocus={clearPreview}
              className="flex w-full items-center px-3 py-1.5 text-left text-sm text-brand-700 hover:bg-surface-hover focus:bg-surface-hover focus:outline-none disabled:opacity-60"
            >
              New customer…
            </button>
          </div>
          <p
            id={noteId}
            role="note"
            aria-live="polite"
            className={cn(
              'mx-2 mt-1 rounded-md bg-surface px-2.5 py-2 text-note text-ink-dim',
              !previewText && 'hidden',
            )}
          >
            {previewText}
          </p>
          {error && (
            <p role="alert" className="mx-2 mt-1 rounded-md bg-err-bg px-2.5 py-2 text-note text-err">
              {error}
            </p>
          )}
        </div>
      )}

      {creating && (
        <CustomerFormDialog
          mode="create"
          onSaved={(dto) => void created(dto)}
          onClose={() => {
            setCreating(false);
            triggerRef.current?.focus();
          }}
        />
      )}
    </div>
  );
}

export default ProjectCustomerMenu;
