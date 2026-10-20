// AutoflowExplainDrawer — why an autoflow would or wouldn't fire for one
// issue (`GET /api/autoflows/explain?issue_ref=`, `rupu autoflow explain`):
// its claim, last run, queued wakes and recent cycle events. Opened from a
// Library → Autoflows row's "Explain", or from Run now once a cycle started.
// The ref must be the full form; the server's 400 shows inline otherwise.

import { useCallback, useEffect, useId, useState, type FormEvent } from 'react';
import { apiErrorMessage } from '../../lib/api';
import { admin, type AutoflowExplainItem, type AutoflowWakeRow } from '../../lib/api3b/admin';
import { relativeTime } from '../../lib/time';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Skeleton } from '../ui/Skeleton';
import { StatusChip } from '../ui/StatusChip';
import { Drawer } from './Drawer';
import { ReportFields } from './ReportFields';

function WakeLine({ wake }: { wake: AutoflowWakeRow }) {
  return (
    <li className="flex flex-wrap items-center gap-2 px-3 py-1.5 text-note">
      <StatusChip tone={wake.state === 'queued' ? 'pending' : 'neutral'}>{wake.state}</StatusChip>
      <span className="font-mono text-ink">{wake.event}</span>
      <span className="text-ink-mute">{wake.source}</span>
      <span className="flex-1" />
      <span className="text-ink-mute" title={wake.not_before}>
        not before {relativeTime(wake.not_before)}
      </span>
    </li>
  );
}

function ExplainBody({ item }: { item: AutoflowExplainItem }) {
  const { claim, queued_wakes, recent_processed_wake, recent_cycle_events, ...summary } = item;
  const wakes = [...queued_wakes, ...(recent_processed_wake ? [recent_processed_wake] : [])];
  return (
    <div className="space-y-5">
      <ReportFields data={summary} />
      {claim ? (
        <ReportFields data={claim} title="Claim" />
      ) : (
        <p className="text-note text-ink-dim">No claim is held for this issue.</p>
      )}
      {wakes.length > 0 && (
        <section className="space-y-2">
          <h3 className="text-note font-semibold uppercase tracking-wide text-ink-mute">Wakes</h3>
          <ul className="divide-y divide-border rounded-lg border border-border">
            {wakes.map((w) => (
              <WakeLine key={w.wake_id} wake={w} />
            ))}
          </ul>
        </section>
      )}
      {recent_cycle_events.length > 0 && (
        <section className="space-y-2">
          <h3 className="text-note font-semibold uppercase tracking-wide text-ink-mute">Recent cycle events</h3>
          <ul className="divide-y divide-border rounded-lg border border-border">
            {recent_cycle_events.map((ev, i) => (
              <li key={i} className="px-3 py-1.5 text-note">
                <div className="flex gap-2">
                  <span className="font-mono text-ink">{String(ev.event ?? '')}</span>
                  <span className="text-ink-mute">{String(ev.workflow ?? '')}</span>
                  <span className="flex-1" />
                  <span className="text-ink-mute">{relativeTime(String(ev.at ?? ''))}</span>
                </div>
                {typeof ev.detail === 'string' && ev.detail && ev.detail !== '-' && (
                  <div className="mt-0.5 text-ink-dim">{ev.detail}</div>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

export function AutoflowExplainDrawer({
  autoflow,
  initialRef = '',
  onClose,
}: {
  autoflow: string;
  initialRef?: string;
  onClose: () => void;
}) {
  const inputId = useId();
  const [ref, setRef] = useState(initialRef);
  const [item, setItem] = useState<AutoflowExplainItem | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const explain = useCallback(async (issueRef: string) => {
    setLoading(true);
    setError(null);
    try {
      setItem((await admin.getAutoflowExplain(issueRef)).item);
    } catch (e) {
      setItem(null);
      setError(apiErrorMessage(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (initialRef) void explain(initialRef);
  }, [initialRef, explain]);

  function submit(e: FormEvent) {
    e.preventDefault();
    if (ref.trim()) void explain(ref.trim());
  }

  return (
    <Drawer
      title={`Explain ${autoflow}`}
      subtitle="Why this autoflow would or wouldn't fire for an issue."
      onClose={onClose}
    >
      <form onSubmit={submit} className="mb-4">
        <label htmlFor={inputId} className="mb-1 block text-ui font-semibold text-ink">
          Issue
        </label>
        <div className="flex gap-2">
          <Input
            id={inputId}
            value={ref}
            onChange={(e) => setRef(e.target.value)}
            placeholder="github:owner/repo/issues/42"
            className="max-w-none flex-1 font-mono"
            spellCheck={false}
          />
          <Button type="submit" variant="secondary" disabled={loading || !ref.trim()}>
            Explain
          </Button>
        </div>
      </form>
      {error && (
        <p role="alert" className="mb-3 text-note text-err">
          {error}
        </p>
      )}
      {loading ? (
        <div role="status" aria-label="Explaining…" className="space-y-2">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
          <Skeleton className="h-4 w-3/5" />
        </div>
      ) : item ? (
        <ExplainBody item={item} />
      ) : null}
    </Drawer>
  );
}

export default AutoflowExplainDrawer;
