// ValidateAllDrawer — Library → Agents' "Validate all" (Plan 3b placement
// table, "Agent validation"): what `rupu agent validate --all` checks, from
// `GET /api/agents/validate`, as a results drawer — one row per agent file,
// the failing ones first with the reason. It writes nothing.

import { useCallback, useEffect, useState } from 'react';
import { apiErrorMessage } from '../../lib/api';
import { launchApi, type AgentValidationReport } from '../../lib/api3b/launch';
import DialogFrame from '../customers/DialogFrame';
import { Button } from '../ui/Button';
import { StatusChip } from '../ui/StatusChip';
import { Skeleton } from '../ui/Skeleton';

/** A file's last two path segments ("…/agents/x.md"); the row's tooltip has the whole path. */
function pathTail(path: string): string {
  const parts = path.split('/').filter(Boolean);
  return parts.length > 2 ? `…/${parts.slice(-2).join('/')}` : path;
}

export default function ValidateAllDrawer({ onClose }: { onClose: () => void }) {
  const [report, setReport] = useState<AgentValidationReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  const check = useCallback(async () => {
    setChecking(true);
    setError(null);
    try {
      setReport(await launchApi.validateAllAgents());
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    void check();
  }, [check]);

  const rows = (report?.rows ?? []).slice().sort((a, b) => Number(a.ok) - Number(b.ok) || a.name.localeCompare(b.name));
  const failing = rows.filter((r) => !r.ok).length;

  return (
    <DialogFrame title="Validate all agents" onRequestClose={onClose} className="ml-auto max-w-2xl">
      <p className="mt-1 text-ui text-ink-dim">
        Each agent file parses, its <span className="font-mono">tools:</span> name known tools and its skills resolve:
        what <span className="font-mono">rupu agent validate --all</span> checks.
      </p>
      <div className="mt-4 flex items-center gap-2">
        {report && (
          <span className={failing > 0 ? 'text-ui font-semibold text-err' : 'text-ui font-semibold text-ok'}>
            {failing > 0 ? `${failing} of ${rows.length} agent files fail` : `All ${rows.length} agent files pass`}
          </span>
        )}
        <span className="flex-1" />
        <Button variant="secondary" size="sm" onClick={() => void check()} disabled={checking}>
          {checking && report ? 'Checking…' : 'Check again'}
        </Button>
      </div>
      {error && (
        <p role="alert" className="mt-3 text-ui font-medium text-err">
          {error}
        </p>
      )}
      <div className="mt-3 max-h-[60vh] overflow-y-auto rounded-lg border border-border">
        {report === null && !error ? (
          <div className="space-y-2 p-3" aria-label="Checking agents…" role="status">
            {Array.from({ length: 4 }).map((_, i) => (
              <Skeleton key={i} className="h-4 w-full" />
            ))}
          </div>
        ) : (
          <table className="w-full text-left text-ui">
            <thead className="bg-surface text-note uppercase tracking-wide text-ink-mute">
              <tr>
                <th className="px-3 py-2 font-semibold">Agent</th>
                <th className="px-3 py-2 font-semibold">Layer</th>
                <th className="px-3 py-2 font-semibold">Result</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.path} className="border-t border-border align-top">
                  <td className="px-3 py-2">
                    <div className="font-mono text-ink">{r.name}</div>
                    <div className="font-mono text-note text-ink-mute" title={r.path}>
                      {pathTail(r.path)}
                    </div>
                    {r.error && <div className="mt-1 text-note text-err">{r.error}</div>}
                  </td>
                  <td className="px-3 py-2">
                    <StatusChip tone={r.layer === 'project' ? 'origin-project' : 'origin-global'}>{r.layer}</StatusChip>
                  </td>
                  <td className="px-3 py-2">
                    <StatusChip tone={r.ok ? 'completed' : 'failed'}>{r.ok ? 'valid' : 'invalid'}</StatusChip>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      <div className="mt-4 flex justify-end">
        <Button variant="secondary" onClick={onClose}>
          Close
        </Button>
      </div>
    </DialogFrame>
  );
}
