// FleetInstallDialog — Library → Agents' "Install stock fleet" (Plan 3b
// placement table, shaped like the Install skills dialog of mock §6): every
// stock fleet file with its status at the chosen root (`GET /api/fleet`), an
// install-to picker (global, or a project's `.rupu/`), and a danger "Replace
// edited files" that maps to `force`. Install calls `POST /api/fleet/install`;
// the statuses shown afterwards come from re-reading the listing, not from
// the POST.

import { useCallback, useEffect, useState } from 'react';
import { api, apiErrorMessage, type ProjectRow } from '../../lib/api';
import {
  fleetWriteCount,
  launchApi,
  type FleetInstalled,
  type FleetListing,
  type WriteReport,
} from '../../lib/api3b/launch';
import DialogFrame from '../customers/DialogFrame';
import { Button } from '../ui/Button';
import { Select } from '../ui/Select';
import { StatusChip, type ChipTone } from '../ui/StatusChip';
import { Skeleton } from '../ui/Skeleton';

const STATE: Record<FleetInstalled, { label: string; tone: ChipTone }> = {
  current: { label: 'installed', tone: 'completed' },
  missing: { label: 'missing', tone: 'neutral' },
  modified: { label: 'edited', tone: 'awaiting_approval' },
  unreadable: { label: 'unreadable', tone: 'failed' },
};

export default function FleetInstallDialog({
  onClose,
  onInstalled,
}: {
  onClose: () => void;
  /** Called after an install, once the listing has been re-read. */
  onInstalled?: () => void;
}) {
  const [projects, setProjects] = useState<ProjectRow[]>([]);
  const [wsId, setWsId] = useState('');
  const [listing, setListing] = useState<FleetListing | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [force, setForce] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<WriteReport | null>(null);

  useEffect(() => {
    api
      .getProjects()
      .then(setProjects)
      .catch(() => {
        // Non-fatal: only the global root is offered.
      });
  }, []);

  const load = useCallback(async (id: string) => {
    setLoadError(null);
    try {
      setListing(await launchApi.getFleet(id || undefined));
    } catch (e: unknown) {
      setLoadError(apiErrorMessage(e));
    }
  }, []);

  useEffect(() => {
    setListing(null);
    setReport(null);
    void load(wsId);
  }, [wsId, load]);

  const stateAt = (e: FleetListing['entries'][number]): FleetInstalled =>
    wsId ? (e.project ?? 'missing') : e.global;
  const states = (listing?.entries ?? []).map(stateAt);
  const count = fleetWriteCount(states, force);
  const root = wsId ? (listing?.project_root ?? '') : (listing?.global_root ?? '');

  async function install() {
    if (installing || count === 0) return;
    if (force) {
      const edited = states.filter((s) => s === 'modified' || s === 'unreadable').length;
      if (
        edited > 0 &&
        !window.confirm(`Replace ${edited} edited ${edited === 1 ? 'file' : 'files'} with the shipped ${edited === 1 ? 'copy' : 'copies'}? Your edits are lost.`)
      ) {
        return;
      }
    }
    setInstalling(true);
    setError(null);
    try {
      const r = await launchApi.installFleet({ force, ...(wsId ? { project: wsId } : {}) });
      await load(wsId);
      setReport(r);
      setForce(false);
      onInstalled?.();
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setInstalling(false);
    }
  }

  return (
    <DialogFrame title="Install stock fleet" onRequestClose={onClose} className="max-w-2xl">
      <p className="mt-1 text-ui text-ink-dim">
        The security-assessment agents and workflows the built-in engagement profiles name, as files you can read and
        edit: what <span className="font-mono">rupu fleet install</span> writes. Existing files are kept.
      </p>

      <label className="mt-4 block">
        <span className="mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim">Install to</span>
        <Select value={wsId} onChange={(e) => setWsId(e.target.value)} disabled={installing} aria-label="Install to" className="w-full">
          <option value="">Global</option>
          {projects.map((p) => (
            <option key={p.ws_id} value={p.ws_id}>
              {p.name} ({p.path})
            </option>
          ))}
        </Select>
      </label>

      {!loadError && (
      <div className="mt-3 max-h-[45vh] overflow-y-auto rounded-lg border border-border">
        {listing === null ? (
          <div className="space-y-2 p-3" role="status" aria-label="Loading the fleet…">
            {Array.from({ length: 5 }).map((_, i) => (
              <Skeleton key={i} className="h-4 w-full" />
            ))}
          </div>
        ) : (
          <table className="w-full text-left text-ui">
            <thead className="bg-surface text-note uppercase tracking-wide text-ink-mute">
              <tr>
                <th className="px-3 py-2 font-semibold">File</th>
                <th className="px-3 py-2 font-semibold">Kind</th>
                <th className="px-3 py-2 text-right font-semibold">Status</th>
              </tr>
            </thead>
            <tbody>
              {(listing?.entries ?? []).map((e) => {
                const st = STATE[stateAt(e)];
                return (
                  <tr key={e.path} className="border-t border-border">
                    <td className="px-3 py-1.5 font-mono text-ink">{e.path}</td>
                    <td className="px-3 py-1.5 text-ink-dim">{e.kind}</td>
                    <td className="px-3 py-1.5 text-right">
                      <StatusChip tone={st.tone}>{st.label}</StatusChip>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
      )}
      {loadError && <p className="mt-3 text-ui text-err">{loadError}</p>}

      <label className="mt-3 flex items-center gap-2 text-ui font-medium text-err">
        <input
          type="checkbox"
          checked={force}
          onChange={(e) => setForce(e.target.checked)}
          disabled={installing}
          aria-label="Replace edited files"
          className="accent-err"
        />
        Replace edited files
        <span className="font-normal text-ink-mute">(overwrites them with the shipped copy)</span>
      </label>

      {report && (
        <p className="mt-3 text-ui text-ok">
          Created {report.created} · kept {report.skipped} · overwrote {report.overwrote}.
        </p>
      )}
      {error && (
        <p role="alert" className="mt-3 text-ui font-medium text-err">
          {error}
        </p>
      )}

      <div className="mt-4 flex items-center justify-end gap-2">
        <span className="min-w-0 flex-1 truncate font-mono text-note text-ink-mute" title={root}>
          {root}
        </span>
        <Button variant="secondary" onClick={onClose} disabled={installing} className="shrink-0">
          {report ? 'Done' : 'Cancel'}
        </Button>
        <Button
          variant={force ? 'danger' : 'solid'}
          className="shrink-0 whitespace-nowrap"
          onClick={() => void install()}
          disabled={installing || listing === null || count === 0}
        >
          {installing ? 'Installing…' : count === 0 ? 'Nothing to install' : `Install ${count} ${count === 1 ? 'file' : 'files'}`}
        </Button>
      </div>
    </DialogFrame>
  );
}
