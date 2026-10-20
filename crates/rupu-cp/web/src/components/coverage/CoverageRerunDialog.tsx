// CoverageRerunDialog — a coverage target's "Rerun…" (Plan 3b placement
// table, "Coverage rerun"): pick a recorded run from the target's runs,
// confirm, then `POST /api/coverage/:target/rerun {run}` replays it — the
// run's agent, prompt, permission mode and launch skills — as a new run on
// the same target. A 202 means the run was started, not that it recorded
// anything, so the dialog says "rerun requested" and links to the new run.

import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, apiErrorMessage, type RunListEntry } from '../../lib/api';
import { rerunCoverage, type CoverageRerun } from '../../lib/api3b/library';
import { relativeTime } from '../../lib/time';
import { DialogFrame } from '../customers/DialogFrame';
import { Button } from '../ui/Button';
import { Spinner } from '../ui/Spinner';
import { StatusChip } from '../ui/StatusChip';
import { cn } from '../../lib/cn';

/** Only a standalone agent run is replayed; the rest are a 501. */
const rerunnable = (r: RunListEntry) => r.surface === 'agent';

export function agentRunHref(runId: string): string {
  return `/activity?tab=agents&q=${encodeURIComponent(`id:${runId}`)}`;
}

/** The header's "Rerun…" and the dialog it opens. */
export function CoverageRerunButton({ target, wsId }: { target: string; wsId?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant="secondary" onClick={() => setOpen(true)}>
        Rerun…
      </Button>
      {open && <CoverageRerunDialog target={target} wsId={wsId} onClose={() => setOpen(false)} />}
    </>
  );
}

export function CoverageRerunDialog({ target, wsId, onClose }: { target: string; wsId?: string; onClose: () => void }) {
  const [runs, setRuns] = useState<RunListEntry[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState<CoverageRerun | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.getCoverageRuns(target, wsId).then(
      (r) => {
        if (cancelled) return;
        setRuns(r);
        setPicked(r.find(rerunnable)?.run_id ?? null);
      },
      (e: unknown) => !cancelled && setLoadError(apiErrorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  async function start() {
    if (!picked || busy) return;
    setBusy(true);
    setError(null);
    try {
      setStarted(await rerunCoverage(target, picked, wsId));
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const anyOther = (runs ?? []).some((r) => !rerunnable(r));

  return (
    <DialogFrame
      title="Rerun a recorded run"
      onRequestClose={onClose}
      onEscape={confirming && !started ? () => setConfirming(false) : undefined}
      className="max-w-xl"
    >
      <p className="mt-1 text-note text-ink-mute">
        Replays a run on <span className="font-mono">{target}</span> as a new run on the same target, so the two can be
        diffed.
      </p>

      {loadError ? (
        <p role="alert" className="mt-4 text-note text-err">
          {loadError}
        </p>
      ) : runs === null ? (
        <div className="mt-4">
          <Spinner size="sm" label="Loading runs…" />
        </div>
      ) : runs.length === 0 ? (
        <p className="mt-4 text-sm text-ink-dim">No runs are recorded on this target yet.</p>
      ) : (
        <fieldset className="mt-4" disabled={confirming || started !== null}>
          <legend className="sr-only">Run to rerun</legend>
          <ul className="max-h-72 divide-y divide-border overflow-y-auto rounded-lg border border-border">
            {/* Once confirming, only the run being replayed stays listed. */}
            {runs.filter((r) => !(confirming || started) || r.run_id === picked).map((r) => {
              const can = rerunnable(r);
              return (
                <li key={r.run_id}>
                  <label
                    className={cn(
                      'flex items-center gap-3 px-3 py-2',
                      can ? 'cursor-pointer hover:bg-surface-hover' : 'opacity-60',
                    )}
                  >
                    <input
                      type="radio"
                      name="rerun-run"
                      className="h-4 w-4 accent-brand-600"
                      checked={picked === r.run_id}
                      disabled={!can}
                      onChange={() => setPicked(r.run_id)}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-mono text-ui text-ink">{r.run_id}</span>
                      <span className="block text-note text-ink-mute">
                        {relativeTime(r.started_at)} · {r.model} · {r.cells_asserted} assertions · {r.findings} findings
                      </span>
                    </span>
                    <StatusChip tone="neutral">{r.surface}</StatusChip>
                  </label>
                </li>
              );
            })}
          </ul>
          {anyOther && (
            <p className="mt-1.5 text-note text-ink-mute">
              Only agent runs can be rerun: a session, workflow or autoflow run is replayed from its own page.
            </p>
          )}
        </fieldset>
      )}

      {confirming && !started && (
        <div className="mt-4 rounded-lg bg-warn-bg px-3 py-2 text-note text-warn ring-1 ring-warn/30">
          Rerun <span className="font-mono">{picked}</span>? This starts the run's agent again with its prompt, permission
          mode and launch skills; provider, model and concerns come from the agent's current file and config.
        </div>
      )}

      {started && (
        <div role="status" className="mt-4 rounded-lg bg-info-bg px-3 py-2 text-note text-info ring-1 ring-info/30">
          Rerun requested as{' '}
          <Link to={agentRunHref(started.run_id)} className="font-mono font-medium underline">
            {started.run_id} →
          </Link>
          . It joins this target's runs once it records, and can then be diffed with{' '}
          <span className="font-mono">{started.replays}</span>.
        </div>
      )}

      {error && (
        <p role="alert" className="mt-3 rounded-lg border border-err/30 bg-err-bg px-3 py-2 text-note text-err">
          {error}
        </p>
      )}

      <div className="mt-5 flex justify-end gap-2">
        {started ? (
          <Button variant="secondary" onClick={onClose}>
            Close
          </Button>
        ) : confirming ? (
          <>
            <Button variant="secondary" disabled={busy} onClick={() => setConfirming(false)}>
              Back
            </Button>
            <Button variant="solid" disabled={busy} onClick={start}>
              {busy ? 'Starting…' : 'Start rerun'}
            </Button>
          </>
        ) : (
          <>
            <Button variant="secondary" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="solid" disabled={!picked} onClick={() => setConfirming(true)}>
              Rerun this run
            </Button>
          </>
        )}
      </div>
    </DialogFrame>
  );
}

export default CoverageRerunDialog;
