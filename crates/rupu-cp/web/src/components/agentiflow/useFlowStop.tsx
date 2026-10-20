// The Agentiflow page's two stops (operator MCP Plan 3b, UI-2, mock §4).
//
// Stop (`POST …/stop {now:false}`) queues the graceful stop: the lead stops
// at the next round boundary. Its 200 means "queued", not "stopped", so the
// page shows a pending state until the run itself stops reading `running`.
// Stop now asks first (the warn card), then `{now:true}` kills the
// coordinator and its units; the page reloads to show the closed-out run.
//
// The hook returns the header buttons and the panel under the header (the
// confirm card, the pending line, an inline error), so the page places each.

import { useEffect, useState, type ReactNode } from 'react';
import { ApiError, apiErrorMessage } from '../../lib/api';
import { agentiflowsApi } from '../../lib/api3b/agentiflows';
import { Button } from '../ui/Button';
import { StatusChip } from '../ui/StatusChip';
import { ConfirmCard } from './ConfirmCard';

export function useFlowStop({
  id,
  running,
  coordinatorGone,
  liveUnits,
  reload,
}: {
  id: string;
  running: boolean;
  /** A `running` record whose coordinator process is gone: the graceful stop
   *  has nothing to read it, so only Stop now (which closes it out) applies. */
  coordinatorGone: boolean;
  liveUnits: number;
  reload: () => void;
}): { buttons: ReactNode; panel: ReactNode } {
  const [requested, setRequested] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [stoppingNow, setStoppingNow] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A 501: this control plane can't stop flows (no `rupu cp serve`).
  const [unavailable, setUnavailable] = useState<string | null>(null);

  // Pending ends when the data says the run stopped, never on the POST.
  useEffect(() => {
    if (running) return;
    setRequested(false);
    setConfirming(false);
    setStoppingNow(false);
  }, [running]);

  if (!running) return { buttons: null, panel: null };

  function fail(e: unknown) {
    if (e instanceof ApiError && e.status === 501) setUnavailable(apiErrorMessage(e));
    else setError(apiErrorMessage(e));
  }

  async function stop() {
    setBusy(true);
    setError(null);
    try {
      await agentiflowsApi.stop(id, false);
      setRequested(true);
    } catch (e: unknown) {
      fail(e);
    } finally {
      setBusy(false);
    }
  }

  async function stopNow() {
    setBusy(true);
    setError(null);
    setStoppingNow(true);
    try {
      await agentiflowsApi.stop(id, true);
      reload();
    } catch (e: unknown) {
      setStoppingNow(false);
      fail(e);
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  }

  const off = unavailable !== null;
  const buttons = (
    <>
      {requested && (
        <StatusChip tone="pending" dot title="Stop queued; the flow stops at the next round boundary.">
          stop requested
        </StatusChip>
      )}
      <Button
        variant="secondary"
        onClick={() => void stop()}
        disabled={off || busy || requested || stoppingNow || coordinatorGone}
        title={
          coordinatorGone
            ? 'The coordinator is gone, so nothing would read a graceful stop. Stop now closes the run out.'
            : 'Finish the current round, then stop.'
        }
      >
        Stop
      </Button>
      <Button
        variant="danger-outline"
        onClick={() => {
          setError(null);
          setConfirming(true);
        }}
        disabled={off || busy || stoppingNow || confirming}
      >
        {stoppingNow ? 'Stopping…' : 'Stop now'}
      </Button>
    </>
  );

  const panel =
    confirming || requested || error || unavailable ? (
      <div className="space-y-2">
        {confirming && (
          <ConfirmCard
            title="Stop now?"
            cancelLabel="Keep running"
            confirmLabel="Stop now"
            busyLabel="Stopping…"
            busy={busy}
            onCancel={() => setConfirming(false)}
            onConfirm={() => void stopNow()}
          >
            The coordinator and its {liveUnits} running {liveUnits === 1 ? 'unit is' : 'units are'} killed at once, so
            the current round's work is lost. Use <b className="font-semibold">Stop</b> to finish the round first.
          </ConfirmCard>
        )}
        {requested && !stoppingNow && (
          <p role="status" className="text-note text-ink-dim">
            Stop requested. The lead stops after this round; the page updates when it does.
          </p>
        )}
        {error && (
          <p role="alert" className="text-ui font-medium text-err">
            {error}
          </p>
        )}
        {unavailable && <p className="text-ui font-medium text-warn">{unavailable}</p>}
      </div>
    ) : null;

  return { buttons, panel };
}
