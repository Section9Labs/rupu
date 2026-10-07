// Customer detail — `/customers/:slug[/:tab]`. A breadcrumb, an identity
// header (dot, name, slug, contact · created · layer path, notes; Edit /
// Archive and a kebab with Delete), five rollup tiles over a range, and a tab
// bar: Overview (cost by project + recent runs), Projects (the assigned
// projects, Assign / Unassign), Runs, Findings, Usage and Config.
//
// Runs, Findings and Usage embed the CP's own Workflow/Agent Runs tables,
// Findings table and Usage page, each scoped with `customer=<slug>` (remote
// hosts that can't filter show as unavailable, never as zero). Config shows
// what this page can say from the detail; Task 8 (the config-layer editor)
// replaces its body.

import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import {
  BarChart3,
  FolderKanban,
  LayoutDashboard,
  ListOrdered,
  MoreHorizontal,
  Settings as SettingsIcon,
  ShieldAlert,
  ShieldCheck,
} from 'lucide-react';
import {
  api,
  apiErrorMessage,
  ApiError,
  parseCustomerConflict,
  type CustomerConflict,
  type CustomerDetail as CustomerDetailDto,
  type CustomerDto,
  type DashboardRange,
  type DefaultAccount,
  type ProjectRow,
  type RunListRow,
} from '../../lib/api';
import { useCustomerScope } from '../../lib/customerScope';
import { formatCost, formatTokens } from '../../lib/usage';
import { relativeTime } from '../../lib/time';
import { runHref } from '../../lib/runs';
import { TabBar, TabButton } from '../../components/TabBar';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import { ListCard } from '../../components/lists/ListCard';
import { StatusPill } from '../../components/StatusPill';
import { TriggerChip } from '../../components/TriggerChip';
import { Badge } from '../../components/ui/Badge';
import { Button } from '../../components/ui/Button';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { Segmented } from '../../components/ui/Segmented';
import { Spinner } from '../../components/ui/Spinner';
import { CustomerDot } from '../../components/customers/CustomerDot';
import { CustomerFormDialog } from '../../components/customers/CustomerFormDialog';
import { PricingErrorMark } from '../../components/customers/PricingErrorMark';
import { AssignProjectDialog } from '../../components/customers/AssignProjectDialog';
import { DialogFrame } from '../../components/customers/DialogFrame';
import { CustomerRunsTab } from '../../components/customers/CustomerRunsTab';
import { StatTile } from '../../components/customers/StatTile';
import { PROJECT_COLUMNS } from '../Projects';
import Findings from '../Findings';
import Usage from '../Usage';

export type CustomerTab = 'overview' | 'projects' | 'runs' | 'findings' | 'usage' | 'config';

const TABS: { id: CustomerTab; label: string; icon: typeof LayoutDashboard }[] = [
  { id: 'overview', label: 'Overview', icon: LayoutDashboard },
  { id: 'projects', label: 'Projects', icon: FolderKanban },
  { id: 'runs', label: 'Runs', icon: ListOrdered },
  { id: 'findings', label: 'Findings', icon: ShieldAlert },
  { id: 'usage', label: 'Usage', icon: BarChart3 },
  { id: 'config', label: 'Config', icon: SettingsIcon },
];

const RANGE_OPTIONS = [
  { value: '7d', label: '7d' },
  { value: '30d', label: '30d' },
  { value: 'all', label: 'all' },
];

function isTab(t: string | undefined): t is CustomerTab {
  return TABS.some((x) => x.id === t);
}

function layerPath(slug: string): string {
  return `~/.rupu/customers/${slug}/config.toml`;
}

function tabHref(slug: string, tab: CustomerTab): string {
  const base = `/customers/${encodeURIComponent(slug)}`;
  return tab === 'overview' ? base : `${base}/${tab}`;
}

/** Where a customer's default account comes from. */
function billsToSource(a: DefaultAccount): string {
  if (a.locked_by === 'customer') return 'locked by customer';
  if (a.locked_by === 'global') return 'locked by global policy';
  if (a.inherited) return 'inherits global';
  return 'customer default';
}

function createdLabel(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString('en-US', { year: 'numeric', month: 'short', day: 'numeric' });
}

/** Focus `selector` inside `ref` once a closing dialog has unmounted. */
function focusLater(ref: React.RefObject<HTMLElement>, selector: string) {
  setTimeout(() => ref.current?.querySelector<HTMLElement>(selector)?.focus(), 0);
}

function Sep() {
  return (
    <span aria-hidden className="text-ink-mute">
      ·
    </span>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function CustomerDetail() {
  const { slug = '', tab: rawTab } = useParams<{ slug: string; tab?: string }>();
  const tab: CustomerTab = isTab(rawTab) ? rawTab : 'overview';
  const navigate = useNavigate();
  const { scope, setScope, reload } = useCustomerScope();

  const [range, setRange] = useState<DashboardRange>('30d');
  const [detail, setDetail] = useState<CustomerDetailDto | null>(null);
  // The range `detail` was loaded for — the tiles say what they show, not
  // what is still loading.
  const [loadedRange, setLoadedRange] = useState<DashboardRange>('30d');
  const [loadError, setLoadError] = useState<string | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [nonce, setNonce] = useState(0);
  const [actionError, setActionError] = useState<string | null>(null);
  const [archiving, setArchiving] = useState(false);
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const actionsRef = useRef<HTMLDivElement>(null);
  const kebabRef = useRef<HTMLButtonElement>(null);

  const refetch = () => setNonce((n) => n + 1);

  // A different customer starts from a clean page.
  useEffect(() => {
    setDetail(null);
    setNotFound(false);
    setLoadError(null);
    setActionError(null);
  }, [slug]);

  useEffect(() => {
    if (!slug) return;
    let cancelled = false;
    api.getCustomer(slug, range).then(
      (d) => {
        if (cancelled) return;
        setDetail(d);
        setLoadedRange(range);
        setLoadError(null);
      },
      (e: unknown) => {
        if (cancelled) return;
        if (e instanceof ApiError && e.status === 404) setNotFound(true);
        else setLoadError(apiErrorMessage(e));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [slug, range, nonce]);

  if (notFound) {
    return (
      <div className="p-8">
        <EmptyState
          title="Customer not found"
          hint={
            <>
              No customer <span className="font-mono">{slug}</span> exists on this control plane.
            </>
          }
          action={
            <Link to="/customers" className="text-ui font-medium text-brand-600 hover:text-brand-700">
              ← Back to customers
            </Link>
          }
        />
      </div>
    );
  }

  if (detail === null) {
    return (
      <div className="p-8">
        {loadError ? (
          <ErrorBanner>{loadError}</ErrorBanner>
        ) : (
          <div className="py-16 flex items-center justify-center">
            <Spinner label="Loading customer…" />
          </div>
        )}
      </div>
    );
  }

  const c = detail.customer;
  const { rollup } = detail;
  const rangeLabel = loadedRange === 'all' ? 'all time' : loadedRange;

  async function toggleArchive() {
    setArchiving(true);
    setActionError(null);
    try {
      await api.archiveCustomer(c.slug, !c.archived);
      reload();
      refetch();
    } catch (e: unknown) {
      setActionError(apiErrorMessage(e));
    } finally {
      setArchiving(false);
    }
  }

  function onDeleted() {
    if (scope === c.slug) setScope(null);
    reload();
    navigate('/customers');
  }

  return (
    <div className="p-8 space-y-6">
      <div>
        <nav aria-label="Breadcrumb" className="mb-3 flex items-center gap-1.5 text-note text-ink-dim">
          <Link to="/customers" className="hover:text-ink hover:underline">
            Customers
          </Link>
          <span aria-hidden className="text-ink-mute">
            /
          </span>
          <span aria-current="page" className="text-ink">
            {c.name}
          </span>
        </nav>

        <header className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
          <div className="flex flex-wrap items-start justify-between gap-4">
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <CustomerDot tint={c.tint} size={12} />
                <h1 className="text-lg font-bold text-ink">{c.name}</h1>
                <span
                  data-testid="customer-slug"
                  className="rounded border border-border bg-surface px-1.5 py-0.5 font-mono text-note text-ink-dim"
                >
                  {c.slug}
                </span>
                {c.archived && (
                  <Badge tone="neutral" ring className="text-ink-mute">
                    Archived
                  </Badge>
                )}
              </div>
              <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-note text-ink-dim">
                {c.contact && (
                  <>
                    <span>{c.contact}</span>
                    <Sep />
                  </>
                )}
                <span>created {createdLabel(c.created_at)}</span>
                <Sep />
                <span className="font-mono text-ink-mute">{layerPath(c.slug)}</span>
              </div>
              {c.notes && <p className="mt-2 max-w-3xl whitespace-pre-line text-ui text-ink-dim">{c.notes}</p>}
            </div>
            <div ref={actionsRef} className="flex shrink-0 items-center gap-2">
              <Button data-action="edit" variant="secondary" onClick={() => setEditing(true)}>
                Edit details
              </Button>
              <Button variant="secondary" onClick={() => void toggleArchive()} disabled={archiving}>
                {c.archived ? 'Unarchive' : 'Archive'}
              </Button>
              <KebabMenu triggerRef={kebabRef} onDelete={() => setDeleting(true)} />
            </div>
          </div>
        </header>
      </div>

      {detail.layer_error && (
        <ErrorBanner>
          <p>This customer's config layer doesn't parse — runs of its projects will fail until it's fixed.</p>
          <p className="mt-1 font-mono text-note">{detail.layer_error}</p>
          {tab !== 'config' && (
            <Link to={tabHref(c.slug, 'config')} className="mt-1 inline-block font-medium underline">
              Open the Config tab →
            </Link>
          )}
        </ErrorBanner>
      )}
      {loadError && <ErrorBanner>Couldn’t refresh this customer: {loadError}</ErrorBanner>}
      {actionError && <ErrorBanner>{actionError}</ErrorBanner>}

      <section aria-label="Rollup" className="space-y-2">
        <div className="flex justify-end">
          <Segmented
            ariaLabel="Range"
            size="sm"
            options={RANGE_OPTIONS}
            value={range}
            onChange={(v) => setRange(v as DashboardRange)}
          />
        </div>
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-5">
          <StatTile
            id="projects"
            label="Projects"
            value={rollup.projects}
            sub={rollup.last_active ? `last active ${relativeTime(rollup.last_active)}` : 'no runs yet'}
          />
          <StatTile id="runs" label={`Runs · ${rangeLabel}`} value={rollup.run_count} />
          <StatTile
            id="findings"
            label="Findings"
            value={rollup.findings_open}
            sub={
              rollup.findings_open > 0 ? (
                <span className="inline-flex items-center gap-1">
                  <ShieldAlert size={11} aria-hidden />
                  needs attention
                </span>
              ) : (
                <span className="inline-flex items-center gap-1">
                  <ShieldCheck size={11} aria-hidden />
                  all clear
                </span>
              )
            }
            subTone={rollup.findings_open > 0 ? 'err' : 'ok'}
          />
          <StatTile
            id="cost"
            label={`Cost · ${rangeLabel}`}
            value={
              <span className="inline-flex items-center gap-1.5">
                {formatCost(rollup.usage.cost_usd)}
                <PricingErrorMark error={rollup.usage.pricing_error} />
              </span>
            }
            sub={`${formatTokens(rollup.usage.total_tokens)} tokens`}
          />
          <BillsToTile account={detail.default_account} layerError={detail.layer_error} />
        </div>
        {rollup.hosts_without_customer && rollup.hosts_without_customer.length > 0 && (
          <p className="text-note text-ink-mute">
            Runs on {rollup.hosts_without_customer.join(', ')} are left out of these numbers — their
            customer can’t be known.
          </p>
        )}
      </section>

      <div className="-mx-8">
        <TabBar>
          {TABS.map((t) => (
            <TabButton
              key={t.id}
              active={tab === t.id}
              onClick={() => navigate(tabHref(c.slug, t.id))}
              icon={t.icon}
              label={t.label}
            />
          ))}
        </TabBar>
      </div>

      {tab === 'overview' && (
        <div className="space-y-6">
          <CostByProject projects={detail.projects} />
          <RecentRuns slug={c.slug} limit={10} title="Recent runs" />
        </div>
      )}
      {tab === 'projects' && (
        <ProjectsTab
          detail={detail}
          onChanged={() => {
            reload();
            refetch();
          }}
        />
      )}
      {tab === 'runs' && <CustomerRunsTab slug={c.slug} />}
      {tab === 'findings' && <Findings key={c.slug} customer={c.slug} />}
      {tab === 'usage' && <Usage key={c.slug} customer={c.slug} />}
      {tab === 'config' && <ConfigSummary detail={detail} />}

      {editing && (
        <CustomerFormDialog
          mode="edit"
          initial={c}
          onSaved={() => {
            setEditing(false);
            refetch();
            focusLater(actionsRef, '[data-action="edit"]');
          }}
          onClose={() => {
            setEditing(false);
            focusLater(actionsRef, '[data-action="edit"]');
          }}
        />
      )}
      {deleting && (
        <DeleteCustomerDialog
          customer={c}
          onDeleted={onDeleted}
          onClose={(changed) => {
            setDeleting(false);
            // Some projects were unassigned before a failure: the page and
            // the scope's customer list no longer match the server.
            if (changed) {
              reload();
              refetch();
            }
            setTimeout(() => kebabRef.current?.focus(), 0);
          }}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Header pieces
// ---------------------------------------------------------------------------

function BillsToTile({ account, layerError }: { account: DefaultAccount | null; layerError: string | null }) {
  if (layerError) {
    return <StatTile id="bills" label="Bills to" value="—" sub="config layer doesn’t parse" subTone="err" />;
  }
  if (!account) {
    return <StatTile id="bills" label="Bills to" value="—" sub="no default account set" />;
  }
  return (
    <StatTile
      id="bills"
      label="Bills to"
      value={
        <span className="block truncate font-mono text-base font-semibold" title={account.account}>
          {account.account}
        </span>
      }
      sub={billsToSource(account)}
    />
  );
}

/** "⋯" menu holding Delete. Escape closes it and returns focus to the trigger. */
function KebabMenu({
  triggerRef,
  onDelete,
}: {
  triggerRef: React.RefObject<HTMLButtonElement>;
  onDelete: () => void;
}) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    menuRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    function onDown(e: MouseEvent) {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  return (
    <div ref={wrapRef} className="relative">
      <button
        ref={triggerRef}
        type="button"
        aria-label="More actions"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className="inline-flex items-center justify-center rounded-md border border-border bg-panel px-2 py-1.5 text-ink hover:bg-bg focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500 focus-visible:ring-offset-1"
      >
        <MoreHorizontal size={14} aria-hidden />
      </button>
      {open && (
        <div
          ref={menuRef}
          role="menu"
          aria-label="Customer actions"
          onKeyDown={(e) => {
            if (e.key === 'Escape' || e.key === 'Tab') {
              if (e.key === 'Escape') e.preventDefault();
              setOpen(false);
              triggerRef.current?.focus();
            }
          }}
          className="absolute right-0 z-20 mt-1 w-48 rounded-lg border border-border bg-panel py-1 shadow-card"
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onDelete();
            }}
            className="block w-full px-3 py-1.5 text-left text-ui text-err hover:bg-err-bg focus:bg-err-bg focus:outline-none"
          >
            Delete customer…
          </button>
        </div>
      )}
    </div>
  );
}

/** Confirm, then delete. A 409 (projects still assigned) lists them and
 *  offers "Unassign all and delete" — explicit, never automatic. If that
 *  sequence fails part-way, the error names what was already unassigned, the
 *  list keeps only what is left, and `onClose(true)` tells the page to
 *  refetch. */
function DeleteCustomerDialog({
  customer,
  onDeleted,
  onClose,
}: {
  customer: CustomerDto;
  onDeleted: () => void;
  /** `changed`: some projects were unassigned while the dialog was open. */
  onClose: (changed: boolean) => void;
}) {
  const [conflict, setConflict] = useState<CustomerConflict | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const changedRef = useRef(false);

  const close = () => onClose(changedRef.current);

  function fail(e: unknown) {
    const c = parseCustomerConflict(e);
    if (c) setConflict(c);
    else setError(apiErrorMessage(e));
    setBusy(false);
  }

  async function doDelete() {
    setBusy(true);
    setError(null);
    try {
      await api.deleteCustomer(customer.slug);
      onDeleted();
    } catch (e: unknown) {
      fail(e);
    }
  }

  async function unassignAllAndDelete() {
    if (!conflict) return;
    setBusy(true);
    setError(null);
    const done: string[] = [];
    for (const p of conflict.projects) {
      try {
        await api.unassignProject(customer.slug, p.ws_id);
      } catch (e: unknown) {
        // Already unassigned (404) is what we wanted; anything else stops.
        if (!(e instanceof ApiError && e.status === 404)) {
          setConflict({ ...conflict, projects: conflict.projects.filter((q) => !done.includes(q.path)) });
          setError(
            done.length > 0
              ? `Unassigned ${done.join(', ')}, then failed on ${p.path}: ${apiErrorMessage(e)}`
              : `Failed to unassign ${p.path}: ${apiErrorMessage(e)}`,
          );
          setBusy(false);
          return;
        }
      }
      done.push(p.path);
      changedRef.current = true;
    }
    try {
      await api.deleteCustomer(customer.slug);
      onDeleted();
    } catch (e: unknown) {
      const c = parseCustomerConflict(e);
      if (c) setConflict(c);
      else setError(`Unassigned ${done.join(', ')}, then the delete failed: ${apiErrorMessage(e)}`);
      setBusy(false);
    }
  }

  return (
    <DialogFrame title={`Delete ${customer.name}?`} onRequestClose={() => !busy && close()}>
      {conflict ? (
        <div className="mt-3 space-y-3">
          {conflict.error && <p className="text-ui text-err">{conflict.error}</p>}
          <p className="text-ui text-ink-dim">
            {conflict.projects.length === 1 ? 'This project is' : 'These projects are'} still assigned
            to {customer.name}. Unassign {conflict.projects.length === 1 ? 'it' : 'them'} first — they
            will run on the global config.
          </p>
          <ul className="max-h-48 overflow-y-auto rounded-lg border border-border divide-y divide-border">
            {conflict.projects.map((p) => (
              <li key={p.ws_id} className="px-3 py-1.5 font-mono text-note text-ink">
                {p.path}
              </li>
            ))}
          </ul>
        </div>
      ) : (
        <p className="mt-3 text-ui text-ink-dim">
          Deletes {customer.name} and{' '}
          <span className="font-mono text-ink">~/.rupu/customers/{customer.slug}/</span>, its config layer
          included. This can’t be undone — Archive keeps it instead.
        </p>
      )}
      {error && <ErrorBanner className="mt-3">{error}</ErrorBanner>}
      <div className="mt-4 flex items-center justify-end gap-2">
        <Button variant="secondary" onClick={close} disabled={busy}>
          Cancel
        </Button>
        {conflict ? (
          <Button variant="danger-outline" onClick={() => void unassignAllAndDelete()} disabled={busy}>
            Unassign all and delete
          </Button>
        ) : (
          <Button variant="danger" onClick={() => void doDelete()} disabled={busy}>
            Delete
          </Button>
        )}
      </div>
    </DialogFrame>
  );
}

// ---------------------------------------------------------------------------
// Tab bodies
// ---------------------------------------------------------------------------

function ProjectsTab({ detail, onChanged }: { detail: CustomerDetailDto; onChanged: () => void }) {
  const c = detail.customer;
  const [assigning, setAssigning] = useState(false);
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const headRef = useRef<HTMLDivElement>(null);
  const n = detail.projects.length;

  async function unassign(p: ProjectRow) {
    setPending((s) => new Set(s).add(p.ws_id));
    setNotice(null);
    setError(null);
    try {
      await api.unassignProject(c.slug, p.ws_id);
      onChanged();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 404) {
        setNotice(`Already unassigned — ${p.name} no longer belongs to ${c.name}.`);
        onChanged();
      } else {
        setError(apiErrorMessage(e));
      }
    } finally {
      setPending((s) => {
        const next = new Set(s);
        next.delete(p.ws_id);
        return next;
      });
    }
  }

  const columns: Column<ProjectRow>[] = [
    ...PROJECT_COLUMNS,
    {
      key: 'actions',
      header: '',
      align: 'right',
      fit: true,
      interactive: true,
      render: (p) => (
        <Button
          variant="ring"
          aria-label={`Unassign ${p.name}`}
          disabled={pending.has(p.ws_id)}
          onClick={() => void unassign(p)}
        >
          Unassign
        </Button>
      ),
    },
  ];

  function closeDialog() {
    setAssigning(false);
    focusLater(headRef, 'button');
  }

  return (
    <section className="space-y-3">
      <div ref={headRef} className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-ui text-ink-dim">
          {n} {n === 1 ? 'project' : 'projects'} · runs in their subdirectories count too
        </p>
        <Button
          onClick={() => setAssigning(true)}
          disabled={c.archived}
          title={c.archived ? 'Unarchive the customer to assign projects' : undefined}
        >
          Assign project
        </Button>
      </div>
      {notice && (
        <p role="status" className="text-note text-ink-dim">
          {notice}
        </p>
      )}
      {error && <ErrorBanner>{error}</ErrorBanner>}
      {n === 0 ? (
        <EmptyState
          title="No projects yet"
          hint={`Assign a project and its runs — and its subdirectories’ — use ${c.name}’s config layer and accounts.`}
        />
      ) : (
        <SortableTable<ProjectRow>
          columns={columns}
          rows={detail.projects}
          rowKey={(p) => p.ws_id}
          rowHref={(p) => `/projects/${encodeURIComponent(p.ws_id)}`}
          initialSort={{ key: 'last_active', dir: 'desc' }}
        />
      )}
      {assigning && (
        <AssignProjectDialog
          customer={c}
          onAssigned={() => {
            closeDialog();
            onChanged();
          }}
          onClose={closeDialog}
        />
      )}
    </section>
  );
}

/** Horizontal bars of each assigned project's cost (`detail.projects[].usage`). */
function CostByProject({ projects }: { projects: ProjectRow[] }) {
  const rows = [...projects].sort((a, b) => (b.usage?.cost_usd ?? 0) - (a.usage?.cost_usd ?? 0));
  const max = rows.reduce((m, p) => Math.max(m, p.usage?.cost_usd ?? 0), 0);
  return (
    <section className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
      <h2 className="text-[9px] font-semibold uppercase tracking-widest text-ink-mute mb-3">Cost by project</h2>
      {rows.length === 0 ? (
        <p className="text-note text-ink-mute">No projects assigned yet.</p>
      ) : (
        <ul className="space-y-2">
          {rows.map((p) => {
            const cost = p.usage?.cost_usd ?? null;
            const pct = max > 0 && cost ? Math.max(2, (cost / max) * 100) : 0;
            return (
              <li key={p.ws_id} className="grid grid-cols-[minmax(0,10rem)_1fr_auto] items-center gap-3 text-note">
                <Link
                  to={`/projects/${encodeURIComponent(p.ws_id)}`}
                  className="truncate text-ink hover:underline"
                  title={p.path}
                >
                  {p.name}
                </Link>
                <div className="h-1.5 rounded-full bg-surface-hover overflow-hidden" aria-hidden>
                  <div className="h-full rounded-full bg-brand-500" style={{ width: `${pct}%` }} />
                </div>
                <span className="inline-flex items-center justify-end gap-1 tabular-nums text-ink">
                  <PricingErrorMark error={p.usage?.pricing_error} />
                  {formatCost(cost)}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** The customer's latest runs on this control plane (`?host=local`, so one
 *  slow remote host never holds the page). */
function RecentRuns({ slug, limit, title }: { slug: string; limit: number; title: string }) {
  const [runs, setRuns] = useState<RunListRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const ctl = new AbortController();
    setRuns(null);
    setError(null);
    api.getRuns({ customer: slug, limit, host: 'local', signal: ctl.signal }).then(
      (rows) => {
        if (!ctl.signal.aborted) setRuns(rows);
      },
      (e: unknown) => {
        if (!ctl.signal.aborted) setError(apiErrorMessage(e));
      },
    );
    return () => ctl.abort();
  }, [slug, limit]);

  return (
    <section>
      <div className="mb-2 flex items-baseline gap-2">
        <h2 className="text-sm font-semibold text-ink">{title}</h2>
        <span className="text-note text-ink-mute">on this control plane</span>
      </div>
      {error && <ErrorBanner>{error}</ErrorBanner>}
      {runs === null && !error && (
        <div className="py-8 flex items-center justify-center">
          <Spinner label="Loading runs…" />
        </div>
      )}
      {runs !== null && runs.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-8 flex items-center justify-center">
          <p className="text-xs text-ink-mute">No runs yet</p>
        </div>
      )}
      {runs !== null && runs.length > 0 && (
        <ListCard>
          {runs.map((r) => (
            <Link
              key={`${r.host_id ?? 'local'}:${r.id}`}
              to={runHref(r)}
              className="flex items-center gap-4 px-4 py-3 hover:bg-surface-hover transition-colors"
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="text-sm font-medium text-ink truncate">{r.workflow_name}</span>
                  <TriggerChip trigger={r.trigger} />
                </div>
                <p className="text-note text-ink-dim mt-0.5">{relativeTime(r.started_at)}</p>
              </div>
              <span className="inline-flex items-center gap-1 text-note tabular-nums text-ink-dim">
                <PricingErrorMark error={r.usage?.pricing_error} />
                {formatCost(r.usage?.cost_usd ?? null)}
              </span>
              <StatusPill status={r.status} />
            </Link>
          ))}
        </ListCard>
      )}
    </section>
  );
}

function ConfigSummary({ detail }: { detail: CustomerDetailDto }) {
  const c = detail.customer;
  return (
    <section className="bg-panel border border-border rounded-xl shadow-card px-5 py-4 space-y-3 text-ui text-ink-dim">
      <h2 className="text-sm font-semibold text-ink">Config layer</h2>
      <p>
        <span className="font-mono text-ink">{layerPath(c.slug)}</span> sits between the global config and
        each project’s own <span className="font-mono">.rupu/config.toml</span>: a project wins on any key{' '}
        {c.name} hasn’t locked with its <span className="font-mono">[policy].lock</span>.
      </p>
      {detail.layer_error && (
        <pre className="whitespace-pre-wrap rounded-lg border border-err/30 bg-err-bg px-3 py-2 font-mono text-note text-err">
          {detail.layer_error}
        </pre>
      )}
      <p>
        Edit and validate it from a terminal with <code className="font-mono text-ink">rupu customer edit {c.slug}</code>
        ; see the effective config with sources and locks with{' '}
        <code className="font-mono text-ink">rupu customer show {c.slug}</code>.
      </p>
    </section>
  );
}
