// Fleet → Hosts — registered hosts (local + remote HTTP CP), with live health,
// active run counts, and last-seen freshness. Provides an Add Host form (name,
// base URL, token) and a per-row Remove action (local host is immutable).

import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { Server } from 'lucide-react';
import { api, type HostView, type HostTransportKind } from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { SectionHeader } from '../components/lists/SectionHeader';
import { shortId } from '../lib/shortId';
import { relativeTime } from '../lib/time';
import { Chip } from '../components/ui/Chip';
import { HostStatusBadge } from '../components/ui/HostStatusBadge';

// ---------------------------------------------------------------------------
// Transport visual tokens (status handled by HostStatusBadge)
// ---------------------------------------------------------------------------

const TRANSPORT_CLASS: Record<HostTransportKind, string> = {
  local:   'bg-surface text-ink-dim ring-border',
  http_cp: 'bg-info-bg text-info ring-info/30',
};

const TRANSPORT_LABEL: Record<HostTransportKind, string> = {
  local: 'local',
  http_cp: 'http-cp',
};

// ---------------------------------------------------------------------------
// Add-host form state
// ---------------------------------------------------------------------------

interface AddForm {
  name: string;
  base_url: string;
  token: string;
}

const EMPTY_FORM: AddForm = { name: '', base_url: '', token: '' };

// ---------------------------------------------------------------------------
// Column definitions
// ---------------------------------------------------------------------------

// Remove callback threaded through via a closure — columns are defined outside
// the component but close over the `onRemove` function provided at call-site.
function buildColumns(onRemove: (id: string) => void): Column<HostView>[] {
  return [
    {
      key: 'name',
      header: 'Name',
      sortable: true,
      // local pinned first (asc): prefix \x00 so it sorts before any real name.
      sortValue: (h) => (h.transport_kind === 'local' ? '\x00' + h.name : '\x01' + h.name),
      render: (h) => (
        <Link to={`/hosts/${encodeURIComponent(h.id)}`} className="block group">
          <div className="text-sm font-medium text-ink group-hover:text-brand-600 truncate">
            {h.name}
          </div>
          <div className="text-note text-ink-mute font-mono truncate">{shortId(h.id)}</div>
        </Link>
      ),
    },
    {
      key: 'transport',
      header: 'Transport',
      sortable: true,
      sortValue: (h) => h.transport_kind,
      render: (h) => (
        <Chip className={TRANSPORT_CLASS[h.transport_kind]}>
          {TRANSPORT_LABEL[h.transport_kind]}
        </Chip>
      ),
    },
    {
      key: 'status',
      header: 'Status',
      sortable: true,
      sortValue: (h) => h.status,
      render: (h) => <HostStatusBadge status={h.status} />,
    },
    {
      key: 'version',
      header: 'Version',
      render: (h) =>
        h.version ? (
          <span className="text-note text-ink-mute font-mono">{h.version}</span>
        ) : (
          <span className="text-ink-mute">—</span>
        ),
    },
    {
      key: 'active_runs',
      header: 'Active runs',
      align: 'right',
      width: 'w-24',
      sortable: true,
      sortValue: (h) => h.active_run_count,
      render: (h) =>
        h.active_run_count > 0 ? (
          <span className="font-medium text-brand-600 tabular-nums">{h.active_run_count}</span>
        ) : (
          <span className="text-ink-mute tabular-nums">0</span>
        ),
    },
    {
      key: 'last_seen',
      header: 'Last seen',
      sortable: true,
      sortValue: (h) => {
        if (!h.last_seen_at) return null;
        const t = Date.parse(h.last_seen_at);
        return Number.isNaN(t) ? null : t;
      },
      render: (h) =>
        h.last_seen_at ? (
          <span className="text-ui text-ink-dim">{relativeTime(h.last_seen_at)}</span>
        ) : (
          <span className="text-ink-mute">—</span>
        ),
    },
    {
      key: 'actions',
      header: '',
      width: 'w-20',
      render: (h) =>
        h.transport_kind === 'local' ? null : (
          <button
            type="button"
            aria-label={`Remove host ${h.name}`}
            onClick={() => onRemove(h.id)}
            className="text-note text-err hover:text-err hover:underline"
          >
            Remove
          </button>
        ),
    },
  ];
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function Hosts() {
  const [hosts, setHosts] = useState<HostView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);

  // Add-host form
  const [showAdd, setShowAdd] = useState(false);
  const [form, setForm] = useState<AddForm>(EMPTY_FORM);
  const [submitting, setSubmitting] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);

  function load() {
    let cancelled = false;
    api
      .getHosts()
      .then((data) => {
        if (cancelled) return;
        setHosts(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load hosts');
      });
    return () => {
      cancelled = true;
    };
  }

  useEffect(load, []);

  async function handleAdd(e: React.FormEvent) {
    e.preventDefault();
    if (!form.name.trim() || !form.base_url.trim()) return;
    setSubmitting(true);
    setAddError(null);
    try {
      await api.addHost({
        name: form.name.trim(),
        base_url: form.base_url.trim(),
        token: form.token.trim() || undefined,
      });
      setForm(EMPTY_FORM);
      setShowAdd(false);
      void load();
    } catch (e: unknown) {
      setAddError(e instanceof Error ? e.message : 'Failed to add host');
    } finally {
      setSubmitting(false);
    }
  }

  async function handleRemove(id: string) {
    setRemoveError(null);
    try {
      await api.removeHost(id);
      void load();
    } catch (e: unknown) {
      setRemoveError(e instanceof Error ? e.message : 'Failed to remove host');
    }
  }

  const columns = useMemo(() => buildColumns(handleRemove), [handleRemove]);

  return (
    <div className="p-8">
      <header className="mb-6 flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Hosts</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Registered execution hosts — the local process and any remote HTTP CP peers.
          </p>
        </div>
        <button
          type="button"
          onClick={() => {
            setShowAdd((v) => !v);
            setAddError(null);
            setForm(EMPTY_FORM);
            if (!showAdd) {
              // Focus name field after state update paints
              requestAnimationFrame(() => nameRef.current?.focus());
            }
          }}
          className="inline-flex items-center gap-1.5 rounded-lg border border-border bg-panel px-3 py-1.5 text-sm font-medium text-ink hover:bg-bg transition-colors"
        >
          {showAdd ? 'Cancel' : 'Add host'}
        </button>
      </header>

      {/* Add host inline form */}
      {showAdd && (
        <form
          onSubmit={handleAdd}
          aria-label="Add host form"
          className="mb-6 rounded-xl border border-border bg-panel/70 px-5 py-4 flex flex-col gap-3"
        >
          <h2 className="text-sm font-semibold text-ink">Add remote host</h2>

          {addError && (
            <p className="text-note text-err">{addError}</p>
          )}

          <div className="flex flex-col gap-1">
            <label htmlFor="host-name" className="text-note font-medium text-ink-dim">
              Name
            </label>
            <input
              id="host-name"
              ref={nameRef}
              type="text"
              value={form.name}
              onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
              placeholder="prod-east"
              required
              className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
            />
          </div>

          <div className="flex flex-col gap-1">
            <label htmlFor="host-base-url" className="text-note font-medium text-ink-dim">
              Base URL
            </label>
            <input
              id="host-base-url"
              type="url"
              value={form.base_url}
              onChange={(e) => setForm((f) => ({ ...f, base_url: e.target.value }))}
              placeholder="https://rupu.example.com"
              required
              className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
            />
            {form.base_url.startsWith('http://') && (
              <p className="text-note text-warn">
                Use https for remote hosts — http is unencrypted.
              </p>
            )}
          </div>

          <div className="flex flex-col gap-1">
            <label htmlFor="host-token" className="text-note font-medium text-ink-dim">
              Token <span className="font-normal text-ink-mute">(optional)</span>
            </label>
            <input
              id="host-token"
              type="password"
              value={form.token}
              onChange={(e) => setForm((f) => ({ ...f, token: e.target.value }))}
              placeholder="Bearer token for remote CP"
              className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
            />
          </div>

          <div className="flex items-center gap-2">
            <button
              type="submit"
              disabled={submitting}
              className="rounded-lg bg-brand-600 px-4 py-1.5 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-60 transition-colors"
            >
              {submitting ? 'Adding…' : 'Add host'}
            </button>
            <button
              type="button"
              onClick={() => { setShowAdd(false); setAddError(null); }}
              className="text-sm text-ink-dim hover:text-ink"
            >
              Cancel
            </button>
          </div>
        </form>
      )}

      {/* Errors */}
      {error && (
        <div className="mb-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}
      {removeError && (
        <div className="mb-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {removeError}
        </div>
      )}

      {hosts === null ? (
        <div className="text-sm text-ink-dim">Loading hosts…</div>
      ) : hosts.length === 0 ? (
        <EmptyState />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Hosts" count={hosts.length} />
          <SortableTable<HostView>
            columns={columns}
            rows={hosts}
            rowKey={(h) => h.id}
            initialSort={{ key: 'name', dir: 'asc' }}
          />
        </section>
      )}
    </div>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-surface flex items-center justify-center mb-3">
        <Server size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No hosts registered</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        The local host appears here automatically once the control plane starts. Add a remote host
        above to federate with another rupu CP deployment.
      </p>
    </div>
  );
}
