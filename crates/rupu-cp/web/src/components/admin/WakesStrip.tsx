// WakesStrip — the line above Activity → Autoflows' table (Plan 3b, UI-5):
// the queued autoflow wakes and the ten most recently processed (`GET
// /api/autoflows/wakes`). It renders nothing while there are no wakes, or
// when the endpoint can't answer here. Queued wakes are listed; processed
// ones sit behind "Show". Polled every 30 s while the tab is visible (and
// once more when it comes back); a 501 (no `rupu cp serve` here) stops the
// polling for good, since asking again can't change that answer.

import { useEffect, useState } from 'react';
import { BellRing } from 'lucide-react';
import { ApiError } from '../../lib/api';
import { admin, wakeDue, type AutoflowWakeRow } from '../../lib/api3b/admin';
import { shortIssueRef } from '../../lib/issueRef';
import { relativeTime } from '../../lib/time';
import { StatusChip } from '../ui/StatusChip';

const POLL_MS = 30_000;

function WakeRow({ wake }: { wake: AutoflowWakeRow }) {
  return (
    <li className="flex flex-wrap items-center gap-2 py-1">
      <StatusChip tone={wake.state === 'queued' ? 'pending' : 'neutral'} dot={wake.state === 'queued'}>
        {wake.state}
      </StatusChip>
      <span className="font-mono text-ink" title={wake.entity}>
        {shortIssueRef(wake.entity)}
      </span>
      <span className="text-ink-dim">{wake.event}</span>
      <span className="text-ink-mute">{wake.source}</span>
      <span className="flex-1" />
      <span className="text-ink-mute" title={wake.not_before}>
        {wake.state === 'queued' ? wakeDue(wake.not_before) : relativeTime(wake.not_before)}
      </span>
    </li>
  );
}

export function WakesStrip() {
  const [rows, setRows] = useState<AutoflowWakeRow[]>([]);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let live = true;
    let unsupported = false;
    let inFlight = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = undefined;
      if (live && !unsupported && !document.hidden) timer = setTimeout(() => void tick(), POLL_MS);
    };
    const tick = async () => {
      if (!live || unsupported || inFlight || document.hidden) return;
      inFlight = true;
      try {
        const r = await admin.getAutoflowWakes();
        if (live) setRows(r.rows);
      } catch (e) {
        if (e instanceof ApiError && e.status === 501) unsupported = true;
        if (live) setRows([]);
      } finally {
        inFlight = false;
      }
      schedule();
    };
    const onVisibility = () => {
      if (document.hidden) {
        if (timer) clearTimeout(timer);
        timer = undefined;
      } else {
        void tick();
      }
    };
    document.addEventListener('visibilitychange', onVisibility);
    void tick();
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, []);

  if (rows.length === 0) return null;
  const queued = rows.filter((w) => w.state === 'queued');
  const processed = rows.filter((w) => w.state !== 'queued');

  return (
    <section aria-label="Autoflow wakes" className="mb-4 rounded-xl border border-border bg-panel px-4 py-2.5 text-note shadow-card">
      <div className="flex items-center gap-2">
        <BellRing size={13} className="text-ink-mute" aria-hidden />
        <span className="font-semibold text-ink">Wakes</span>
        <span className="text-ink-dim">{queued.length} queued</span>
        {processed.length > 0 && (
          <>
            <span className="text-ink-mute">·</span>
            <span className="text-ink-dim">{processed.length} processed recently</span>
            <button
              type="button"
              onClick={() => setOpen((v) => !v)}
              aria-expanded={open}
              className="text-brand-600 hover:underline"
            >
              {open ? 'Hide' : 'Show'}
            </button>
          </>
        )}
      </div>
      {(queued.length > 0 || open) && (
        <ul className="mt-1.5 divide-y divide-border border-t border-border">
          {queued.map((w) => (
            <WakeRow key={w.wake_id} wake={w} />
          ))}
          {open && processed.map((w) => <WakeRow key={w.wake_id} wake={w} />)}
        </ul>
      )}
    </section>
  );
}

export default WakesStrip;
