// InitProjectDialog — Projects' "Initialize project" (Plan 3b placement
// table, shaped like the Install skills dialog of mock §6): an absolute path
// and "Add sample agents and workflows", then `POST /api/projects/init` (what
// `rupu init` does). It lists every file the init wrote. A directory that is
// already a project is a 409: its message shows, with a danger re-run that
// maps to `force` (samples are overwritten, never `config.toml`). An init
// doesn't register the project; the dialog says how it gets listed.

import { useState } from 'react';
import { apiErrorMessage, ApiError } from '../../lib/api';
import { launchApi, type FileAction, type WriteReport } from '../../lib/api3b/launch';
import DialogFrame from '../customers/DialogFrame';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { StatusChip, type ChipTone } from '../ui/StatusChip';

const ACTION: Record<FileAction, ChipTone> = {
  created: 'completed',
  updated: 'running',
  skipped: 'neutral',
  overwrote: 'awaiting_approval',
};

export default function InitProjectDialog({ onClose }: { onClose: () => void }) {
  const [path, setPath] = useState('');
  const [samples, setSamples] = useState(false);
  const [exists, setExists] = useState(false);
  const [force, setForce] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<WriteReport | null>(null);

  const trimmed = path.trim();
  const absolute = trimmed.startsWith('/');

  async function init() {
    if (busy || !absolute) return;
    setBusy(true);
    setError(null);
    try {
      setReport(await launchApi.initProject({ path: trimmed, with_samples: samples, ...(force ? { force: true } : {}) }));
      setExists(false);
      setForce(false);
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
      if (e instanceof ApiError && e.status === 409) setExists(true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <DialogFrame title="Initialize project" onRequestClose={onClose} className="max-w-lg">
      <p className="mt-1 text-ui text-ink-dim">
        Make a directory a rupu project: a <span className="font-mono">.rupu/</span> with its config, as{' '}
        <span className="font-mono">rupu init</span> does.
      </p>

      <label className="mt-4 block">
        <span className="mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim">Directory</span>
        <Input
          value={path}
          onChange={(e) => {
            setPath(e.target.value);
            setExists(false);
            setForce(false);
            setReport(null);
          }}
          disabled={busy}
          placeholder="/path/to/repo"
          aria-label="Project directory"
          className="w-full max-w-none font-mono"
        />
        <span className="mt-1 block text-note text-ink-mute">
          {trimmed && !absolute ? 'Enter an absolute path.' : 'An existing directory on this control plane’s machine.'}
        </span>
      </label>

      <label className="mt-3 flex items-center gap-2 text-ui text-ink">
        <input
          type="checkbox"
          checked={samples}
          onChange={(e) => setSamples(e.target.checked)}
          disabled={busy}
          aria-label="Add sample agents and workflows"
        />
        Add sample agents and workflows
      </label>

      {error && (
        <p role="alert" className="mt-3 text-ui font-medium text-err">
          {error}
        </p>
      )}
      {exists && (
        <label className="mt-2 flex items-center gap-2 text-ui font-medium text-err">
          <input
            type="checkbox"
            checked={force}
            onChange={(e) => setForce(e.target.checked)}
            disabled={busy}
            className="accent-err"
          />
          Re-run it, overwriting the samples (never config.toml)
        </label>
      )}

      {report && (
        <div className="mt-3 space-y-2">
          <div className="max-h-[40vh] overflow-y-auto rounded-lg border border-border">
            <table className="w-full text-left text-ui">
              <tbody>
                {report.files.map((f) => (
                  <tr key={f.path} className="border-t border-border first:border-t-0">
                    <td className="px-3 py-1.5 font-mono text-ink">{f.path}</td>
                    <td className="px-3 py-1.5 text-note text-ink-mute">{f.detail ?? ''}</td>
                    <td className="px-3 py-1.5 text-right">
                      <StatusChip tone={ACTION[f.action]}>{f.action}</StatusChip>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="text-note text-ink-dim">
            <span className="font-mono">{report.root}</span> is a rupu project now. It shows in this list once a run
            starts there.
          </p>
        </div>
      )}

      <div className="mt-4 flex justify-end gap-2">
        <Button variant="secondary" onClick={onClose} disabled={busy}>
          {report ? 'Done' : 'Cancel'}
        </Button>
        {!report && (
          <Button
            variant={exists && force ? 'danger' : 'solid'}
            onClick={() => void init()}
            disabled={busy || !absolute || (exists && !force)}
          >
            {busy ? 'Initializing…' : exists ? 'Re-run init' : 'Initialize'}
          </Button>
        )}
      </div>
    </DialogFrame>
  );
}
