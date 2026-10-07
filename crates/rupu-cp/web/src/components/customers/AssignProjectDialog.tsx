// AssignProjectDialog — pick a project to assign to a customer. Lists the
// projects with no customer; "Show projects of other customers" adds the rest
// (never this customer's own) with their current chip. Picking an unassigned
// project assigns it at once; picking one that belongs elsewhere first says
// "Moves <project> from <other> to <this>" and needs a second click — the PUT
// replaces any earlier assignment. A refusal (e.g. 409 archived) is shown
// inline. Rendered by its owner only while open.

import { useEffect, useMemo, useState } from 'react';
import { api, apiErrorMessage, type CustomerRef, type ProjectRow } from '../../lib/api';
import { Button } from '../ui/Button';
import { ErrorBanner } from '../ui/ErrorBanner';
import { SearchInput } from '../ui/SearchInput';
import { Spinner } from '../ui/Spinner';
import { CustomerChip } from './CustomerChip';
import { DialogFrame } from './DialogFrame';

export interface AssignProjectDialogProps {
  customer: Pick<CustomerRef, 'slug' | 'name'>;
  onAssigned: (project: ProjectRow) => void;
  onClose: () => void;
}

/** `customer: null` = no customer; an ABSENT key = the CP can't read it. */
function isUnassigned(p: ProjectRow): boolean {
  return 'customer' in p && p.customer === null;
}
function isUnknown(p: ProjectRow): boolean {
  return p.customer === undefined;
}

const byName = (a: ProjectRow, b: ProjectRow) => a.name.localeCompare(b.name);

export function AssignProjectDialog({ customer, onAssigned, onClose }: AssignProjectDialogProps) {
  const [projects, setProjects] = useState<ProjectRow[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [showOthers, setShowOthers] = useState(false);
  const [moving, setMoving] = useState<ProjectRow | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.getProjects().then(
      (rows) => {
        if (!cancelled) setProjects(rows);
      },
      (e: unknown) => {
        if (!cancelled) setLoadError(apiErrorMessage(e));
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const { free, others } = useMemo(() => {
    const all = projects ?? [];
    return {
      free: all.filter(isUnassigned).sort(byName),
      others: all.filter((p) => !isUnassigned(p) && p.customer?.slug !== customer.slug).sort(byName),
    };
  }, [projects, customer.slug]);

  const listed = useMemo(() => {
    const rows = showOthers ? [...free, ...others] : free;
    const q = query.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q));
  }, [free, others, showOthers, query]);

  async function assign(p: ProjectRow) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const row = await api.assignProject(customer.slug, p.ws_id);
      onAssigned(row);
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
      setBusy(false);
    }
  }

  function pick(p: ProjectRow) {
    setError(null);
    if (isUnassigned(p)) void assign(p);
    else setMoving(p);
  }

  function requestClose() {
    if (!busy) onClose();
  }

  return (
    <DialogFrame title={`Assign a project to ${customer.name}`} onRequestClose={requestClose} className="max-w-lg">
      <p className="mt-1 text-note text-ink-dim">
        Runs in the project — and in its subdirectories — use {customer.name}’s config layer and accounts.
      </p>

      <div className="mt-4 flex flex-wrap items-center gap-3">
        <div className="min-w-0 flex-1">
          <SearchInput
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Find a project…"
            aria-label="Find a project"
          />
        </div>
        <label className="inline-flex items-center gap-1.5 text-note text-ink-dim">
          <input
            type="checkbox"
            checked={showOthers}
            onChange={(e) => {
              setShowOthers(e.target.checked);
              setMoving(null);
            }}
          />
          Show projects of other customers
        </label>
      </div>

      <div className="mt-3">
        {loadError && <ErrorBanner>{loadError}</ErrorBanner>}
        {projects === null && !loadError && (
          <div className="py-8 flex items-center justify-center">
            <Spinner label="Loading projects…" />
          </div>
        )}
        {projects !== null && listed.length === 0 && (
          <p className="rounded-lg border border-dashed border-border py-6 text-center text-note text-ink-mute">
            {projects.length === 0
              ? 'No projects are registered with this control plane yet.'
              : query.trim()
                ? 'No projects match the filter.'
                : showOthers
                  ? 'No other projects to assign.'
                  : 'Every project already has a customer — show other customers’ projects to move one.'}
          </p>
        )}
        {listed.length > 0 && (
          <ul
            aria-label="Projects"
            className="max-h-72 overflow-y-auto rounded-lg border border-border divide-y divide-border"
          >
            {listed.map((p) => {
              const selected = moving?.ws_id === p.ws_id;
              return (
                <li key={p.ws_id}>
                  <button
                    type="button"
                    data-ws={p.ws_id}
                    disabled={busy}
                    aria-pressed={isUnassigned(p) ? undefined : selected}
                    onClick={() => pick(p)}
                    className={
                      'flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-surface-hover focus:outline-none focus-visible:bg-surface-hover disabled:cursor-not-allowed disabled:opacity-60' +
                      (selected ? ' bg-brand-50' : '')
                    }
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-ui font-medium text-ink">{p.name}</span>
                      <span className="block truncate font-mono text-note text-ink-mute">{p.path}</span>
                    </span>
                    {!isUnassigned(p) && (
                      <CustomerChip customer={p.customer ?? null} unknown={isUnknown(p)} size="sm" />
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {moving && (
        <div
          role="group"
          aria-label="Confirm move"
          className="mt-3 flex items-center gap-2 rounded-lg border border-warn/30 bg-warn-bg px-3 py-2"
        >
          <span className="mr-auto text-note text-ink">
            {isUnknown(moving)
              ? `Assigns ${moving.name} to ${customer.name}, replacing an assignment this control plane can’t read`
              : `Moves ${moving.name} from ${moving.customer?.name ?? 'its customer'} to ${customer.name}`}
          </span>
          <Button variant="secondary" size="sm" onClick={() => setMoving(null)} disabled={busy}>
            Cancel
          </Button>
          <Button size="sm" onClick={() => void assign(moving)} disabled={busy}>
            Move
          </Button>
        </div>
      )}

      {error && <ErrorBanner className="mt-3">{error}</ErrorBanner>}

      <div className="mt-4 flex justify-end">
        <Button variant="secondary" onClick={requestClose} disabled={busy}>
          Close
        </Button>
      </div>
    </DialogFrame>
  );
}

export default AssignProjectDialog;
