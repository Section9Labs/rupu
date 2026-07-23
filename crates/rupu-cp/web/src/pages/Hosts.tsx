// Fleet → Hosts — registered hosts (local + remote HTTP CP), with live health,
// active run counts, and last-seen freshness. Provides an Add Host form (name,
// base URL, token) and a per-row Remove action (local host is immutable).

import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, type HostView, type HostTransportKind } from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import { SectionHeader } from '../components/lists/SectionHeader';
import { shortId } from '../lib/shortId';
import { relativeTime } from '../lib/time';
import { Chip } from '../components/ui/Chip';
import { HostStatusBadge } from '../components/ui/HostStatusBadge';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';

// ---------------------------------------------------------------------------
// Transport visual tokens (status handled by HostStatusBadge)
// ---------------------------------------------------------------------------

const TRANSPORT_CLASS: Record<HostTransportKind, string> = {
  local:   'bg-surface text-ink-dim ring-border',
  http_cp: 'bg-info-bg text-info ring-info/30',
  tunnel:  'bg-warn-bg text-warn ring-warn/30',
  ssh:     'bg-ok-bg text-ok ring-ok/30',
  bucket:  'bg-surface text-ink-dim ring-border',
};

const TRANSPORT_LABEL: Record<HostTransportKind, string> = {
  local: 'local',
  http_cp: 'http-cp',
  tunnel: 'tunnel',
  ssh: 'SSH',
  bucket: 'Bucket',
};

// ---------------------------------------------------------------------------
// Add-host form state
// ---------------------------------------------------------------------------

type AddMode = 'http_cp' | 'tunnel' | 'ssh' | 'bucket';

interface AddForm {
  mode: AddMode;
  name: string;
  // http_cp fields
  base_url: string;
  token: string;
  // ssh fields
  ssh_host: string;
  ssh_port: string;
  ssh_identity: string;
  // bucket fields
  bucket_url: string;
  bucket_prefix: string;
}

const EMPTY_FORM: AddForm = {
  mode: 'http_cp',
  name: '',
  base_url: '',
  token: '',
  ssh_host: '',
  ssh_port: '',
  ssh_identity: '',
  bucket_url: '',
  bucket_prefix: '',
};

/// One-time enrollment result shown after a successful tunnel node enroll.
interface EnrollResult {
  command: string;
  token: string;
}

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
      subject: true,
      sortable: true,
      // local pinned first (asc): prefix \x00 so it sorts before any real name.
      sortValue: (h) => (h.transport_kind === 'local' ? '\x00' + h.name : '\x01' + h.name),
      titleValue: (h) => h.name,
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
      fit: true,
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
      fit: true,
      sortable: true,
      sortValue: (h) => h.status,
      render: (h) => <HostStatusBadge status={h.status} />,
    },
    {
      key: 'version',
      header: 'Version',
      fit: true,
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
      fit: true,
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
      fit: true,
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
      fit: true,
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
  // One-time enrollment result (tunnel mode only).
  const [enrollResult, setEnrollResult] = useState<EnrollResult | null>(null);
  const [copied, setCopied] = useState(false);
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
    if (!form.name.trim()) return;
    setSubmitting(true);
    setAddError(null);

    if (form.mode === 'tunnel') {
      try {
        const result = await api.enrollNode({ name: form.name.trim() });
        setEnrollResult({ command: result.command, token: result.token });
        setForm(EMPTY_FORM);
        setShowAdd(false);
        void load();
      } catch (e: unknown) {
        setAddError(e instanceof Error ? e.message : 'Failed to enroll node');
      } finally {
        setSubmitting(false);
      }
      return;
    }

    if (form.mode === 'ssh') {
      if (!form.ssh_host.trim()) {
        setSubmitting(false);
        return;
      }
      const rawPort = parseInt(form.ssh_port.trim(), 10);
      try {
        await api.addSshHost({
          name: form.name.trim(),
          host: form.ssh_host.trim(),
          port: Number.isNaN(rawPort) ? undefined : rawPort,
          identity_file: form.ssh_identity.trim() || undefined,
        });
        setForm(EMPTY_FORM);
        setShowAdd(false);
        void load();
      } catch (e: unknown) {
        setAddError(e instanceof Error ? e.message : 'Failed to add SSH host');
      } finally {
        setSubmitting(false);
      }
      return;
    }

    if (form.mode === 'bucket') {
      if (!form.bucket_url.trim()) {
        setSubmitting(false);
        return;
      }
      try {
        await api.addBucketHost({
          name: form.name.trim(),
          url: form.bucket_url.trim(),
          prefix: form.bucket_prefix.trim() || undefined,
        });
        setForm(EMPTY_FORM);
        setShowAdd(false);
        void load();
      } catch (e: unknown) {
        setAddError(e instanceof Error ? e.message : 'Failed to add bucket host');
      } finally {
        setSubmitting(false);
      }
      return;
    }

    // HTTP CP mode.
    if (!form.base_url.trim()) {
      setSubmitting(false);
      return;
    }
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

  function handleCopyCommand(command: string) {
    void navigator.clipboard.writeText(command).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
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

      {/* One-time enrollment panel — shown after a successful tunnel node enroll */}
      {enrollResult && (
        <div
          role="region"
          aria-label="Node enrollment result"
          className="mb-6 rounded-xl border border-warn/40 bg-warn-bg px-5 py-4 flex flex-col gap-3"
        >
          <p className="text-sm font-semibold text-warn">
            ⚠ Token shown once — copy it now
          </p>
          <p className="text-xs text-ink-dim">
            This token is displayed only once and cannot be recovered. Run the
            command below on the target machine to connect the node to this
            Control Plane.
          </p>
          <div className="flex items-start gap-2">
            <code className="flex-1 rounded border border-border bg-bg px-3 py-2 text-xs font-mono text-ink break-all select-all">
              {enrollResult.command}
            </code>
            <button
              type="button"
              aria-label="Copy command"
              onClick={() => handleCopyCommand(enrollResult.command)}
              className="shrink-0 rounded border border-border bg-panel px-3 py-2 text-xs font-medium text-ink hover:bg-bg transition-colors"
            >
              {copied ? 'Copied!' : 'Copy'}
            </button>
          </div>
          <button
            type="button"
            onClick={() => { setEnrollResult(null); setCopied(false); }}
            className="self-start text-xs text-ink-dim hover:text-ink"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* Add host inline form */}
      {showAdd && (
        <form
          onSubmit={handleAdd}
          aria-label="Add host form"
          className="mb-6 rounded-xl border border-border bg-panel/70 px-5 py-4 flex flex-col gap-3"
        >
          <h2 className="text-sm font-semibold text-ink">Add host</h2>

          {addError && (
            <p className="text-note text-err">{addError}</p>
          )}

          {/* Transport type selector */}
          <fieldset className="flex flex-col gap-1">
            <legend className="text-note font-medium text-ink-dim mb-1">Type</legend>
            <div className="flex gap-4 flex-wrap">
              <label className="flex items-center gap-1.5 text-sm text-ink cursor-pointer">
                <input
                  type="radio"
                  name="host-mode"
                  value="http_cp"
                  checked={form.mode === 'http_cp'}
                  onChange={() => setForm((f) => ({ ...f, mode: 'http_cp' }))}
                  className="accent-brand-600"
                />
                HTTP CP
              </label>
              <label className="flex items-center gap-1.5 text-sm text-ink cursor-pointer">
                <input
                  type="radio"
                  name="host-mode"
                  value="tunnel"
                  checked={form.mode === 'tunnel'}
                  onChange={() => setForm((f) => ({ ...f, mode: 'tunnel' }))}
                  className="accent-brand-600"
                />
                Tunnel node
              </label>
              <label className="flex items-center gap-1.5 text-sm text-ink cursor-pointer">
                <input
                  type="radio"
                  name="host-mode"
                  value="ssh"
                  checked={form.mode === 'ssh'}
                  onChange={() => setForm((f) => ({ ...f, mode: 'ssh' }))}
                  className="accent-brand-600"
                />
                SSH host
              </label>
              <label className="flex items-center gap-1.5 text-sm text-ink cursor-pointer">
                <input
                  type="radio"
                  name="host-mode"
                  value="bucket"
                  checked={form.mode === 'bucket'}
                  onChange={() => setForm((f) => ({ ...f, mode: 'bucket' }))}
                  className="accent-brand-600"
                />
                Bucket (dead-drop)
              </label>
            </div>
          </fieldset>

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
              placeholder={
                form.mode === 'tunnel' ? 'my-box'
                : form.mode === 'ssh' ? 'prod-ssh'
                : form.mode === 'bucket' ? 'drop-bucket'
                : 'prod-east'
              }
              required
              className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
            />
          </div>

          {form.mode === 'http_cp' && (
            <>
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
            </>
          )}

          {form.mode === 'ssh' && (
            <>
              <div className="flex flex-col gap-1">
                <label htmlFor="host-ssh-host" className="text-note font-medium text-ink-dim">
                  Destination
                </label>
                <input
                  id="host-ssh-host"
                  type="text"
                  value={form.ssh_host}
                  onChange={(e) => setForm((f) => ({ ...f, ssh_host: e.target.value }))}
                  placeholder="user@hostname"
                  required
                  className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
                />
              </div>

              <div className="flex flex-col gap-1">
                <label htmlFor="host-ssh-port" className="text-note font-medium text-ink-dim">
                  Port <span className="font-normal text-ink-mute">(optional, default 22)</span>
                </label>
                <input
                  id="host-ssh-port"
                  type="number"
                  min={1}
                  max={65535}
                  value={form.ssh_port}
                  onChange={(e) => setForm((f) => ({ ...f, ssh_port: e.target.value }))}
                  placeholder="22"
                  className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
                />
              </div>

              <div className="flex flex-col gap-1">
                <label htmlFor="host-ssh-identity" className="text-note font-medium text-ink-dim">
                  Identity file <span className="font-normal text-ink-mute">(optional)</span>
                </label>
                <input
                  id="host-ssh-identity"
                  type="text"
                  value={form.ssh_identity}
                  onChange={(e) => setForm((f) => ({ ...f, ssh_identity: e.target.value }))}
                  placeholder="~/.ssh/id_ed25519"
                  className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
                />
                <p className="text-note text-ink-mute">
                  Authentication uses the system SSH / ssh-agent. No password or key is stored here.
                </p>
              </div>
            </>
          )}

          {form.mode === 'bucket' && (
            <>
              <div className="flex flex-col gap-1">
                <label htmlFor="host-bucket-url" className="text-note font-medium text-ink-dim">
                  Bucket URL
                </label>
                <input
                  id="host-bucket-url"
                  type="text"
                  value={form.bucket_url}
                  onChange={(e) => setForm((f) => ({ ...f, bucket_url: e.target.value }))}
                  placeholder="s3://my-bucket  /  gs://my-bucket  /  file:///tmp/drop"
                  required
                  className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
                />
              </div>

              <div className="flex flex-col gap-1">
                <label htmlFor="host-bucket-prefix" className="text-note font-medium text-ink-dim">
                  Prefix <span className="font-normal text-ink-mute">(optional)</span>
                </label>
                <input
                  id="host-bucket-prefix"
                  type="text"
                  value={form.bucket_prefix}
                  onChange={(e) => setForm((f) => ({ ...f, bucket_prefix: e.target.value }))}
                  placeholder="rupu/drops/"
                  className="rounded border border-border bg-bg px-3 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:outline-none focus:ring-2 focus:ring-brand-400"
                />
                <p className="text-note text-ink-mute">
                  Credentials come from the environment (AWS_*, GOOGLE_APPLICATION_CREDENTIALS, etc.) — no secrets are stored here.
                </p>
              </div>
            </>
          )}

          {form.mode === 'tunnel' && (
            <p className="text-xs text-ink-dim">
              A one-time token and connection command will be shown after enrollment.
              Run the command on the target machine to bring the node online.
            </p>
          )}

          <div className="flex items-center gap-2">
            <button
              type="submit"
              disabled={submitting}
              className="rounded-lg bg-brand-600 px-4 py-1.5 text-sm font-medium text-white hover:bg-brand-700 disabled:opacity-60 transition-colors"
            >
              {submitting
                ? (form.mode === 'tunnel' ? 'Enrolling…' : 'Adding…')
                : (form.mode === 'tunnel' ? 'Enroll node'
                  : form.mode === 'ssh' ? 'Add SSH host'
                  : form.mode === 'bucket' ? 'Add bucket host'
                  : 'Add host')}
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
      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}
      {removeError && <ErrorBanner className="mb-4">{removeError}</ErrorBanner>}

      {hosts === null ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading hosts…" />
        </div>
      ) : hosts.length === 0 ? (
        <EmptyState
          title="No hosts registered"
          hint="The local host appears here automatically once the control plane starts. Add a remote host above to federate with another rupu CP deployment."
        />
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
