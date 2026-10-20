// Settings → Automation (Plan 3b mock §10): the cron-triggered workflows and
// when each fires next (`GET /api/cron`), the event-triggered ones with their
// poll sources and cursors (`GET /api/cron/events`), and the tick `rupu cp
// serve`'s loop runs: Dry run shows what would fire and changes nothing; Tick
// now starts a real tick in the background (202 = started, not done) and
// shows the 409 when another tick holds the lock.

import { useEffect, useState } from 'react';
import { apiErrorMessage } from '../../lib/api';
import { admin, type CronEventsReport, type CronListReport, type WouldFire } from '../../lib/api3b/admin';
import { relativeTime } from '../../lib/time';
import { Button } from '../ui/Button';
import { Skeleton } from '../ui/Skeleton';
import { StatusChip } from '../ui/StatusChip';
import { FieldGroup } from './ConfigField';

/** `in 45s` / `in 12m` / `in 3h` / `in 2d` from a seconds count. */
export function inLabel(secs: number | null): string {
  if (secs === null) return '—';
  if (secs < 60) return `in ${secs}s`;
  if (secs < 3600) return `in ${Math.floor(secs / 60)}m`;
  if (secs < 86_400) return `in ${Math.floor(secs / 3600)}h`;
  return `in ${Math.floor(secs / 86_400)}d`;
}

const thCls = 'px-3 py-2 text-left text-meta font-semibold uppercase tracking-wide text-ink-mute';
const tdCls = 'px-3 py-2 align-top';

type TickState =
  | { kind: 'idle' }
  | { kind: 'busy'; dry: boolean }
  | { kind: 'dry'; wouldFire: WouldFire[] }
  | { kind: 'started' }
  | { kind: 'error'; message: string };

export function AutomationTab({ tick }: { tick: { enabled: boolean; intervalSecs: number } }) {
  const [cron, setCron] = useState<CronListReport | null>(null);
  const [events, setEvents] = useState<CronEventsReport | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [state, setState] = useState<TickState>({ kind: 'idle' });

  useEffect(() => {
    let live = true;
    Promise.all([admin.getCronList(), admin.getCronEvents()])
      .then(([c, e]) => {
        if (!live) return;
        setCron(c);
        setEvents(e);
      })
      .catch((e: unknown) => live && setLoadError(apiErrorMessage(e)));
    return () => {
      live = false;
    };
  }, []);

  async function runTick(dry: boolean) {
    setState({ kind: 'busy', dry });
    try {
      const reply = await admin.cronTick({ dry_run: dry });
      setState(reply.dry_run ? { kind: 'dry', wouldFire: reply.would_fire } : { kind: 'started' });
    } catch (e) {
      setState({ kind: 'error', message: apiErrorMessage(e) });
    }
  }

  if (loadError) {
    return (
      <p role="alert" className="text-sm text-err">
        {loadError}
      </p>
    );
  }
  if (!cron || !events) {
    return (
      <div role="status" aria-label="Loading schedules…" className="space-y-3 py-2">
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="h-4 w-2/3" />
        <Skeleton className="h-4 w-2/5" />
      </div>
    );
  }

  const busy = state.kind === 'busy';
  return (
    <div className="space-y-5">
      <FieldGroup title="Schedules" description="Workflows with a cron trigger, and when each fires next (UTC).">
        {cron.rows.length === 0 ? (
          <p className="py-3 text-sm text-ink-dim">No workflow has a cron trigger.</p>
        ) : (
          <table className="mt-2 w-full text-note">
            <thead className="bg-surface">
              <tr>
                <th className={thCls}>Schedule</th>
                <th className={thCls}>Cron</th>
                <th className={thCls}>Next</th>
                <th className={thCls} aria-label="In" />
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {cron.rows.map((r) => (
                <tr key={r.name}>
                  <td className={`${tdCls} font-mono text-ink`}>{r.name}</td>
                  <td className={`${tdCls} font-mono text-ink-dim`}>{r.schedule}</td>
                  <td className={`${tdCls} font-mono tabular-nums text-ink-dim`}>{r.next_utc ?? 'never'}</td>
                  <td className={`${tdCls} text-right text-ink-mute`}>{inLabel(r.in_seconds)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </FieldGroup>

      <FieldGroup
        title="Event triggers"
        description={
          events.summary.poll_sources.length > 0
            ? `Polled sources: ${events.summary.poll_sources.join(', ')}.`
            : 'No poll sources are configured.'
        }
      >
        {events.rows.length === 0 ? (
          <p className="py-3 text-sm text-ink-dim">No workflow has an event trigger.</p>
        ) : (
          <table className="mt-2 w-full text-note">
            <thead className="bg-surface">
              <tr>
                <th className={thCls}>Workflow</th>
                <th className={thCls}>Event</th>
                <th className={thCls}>Sources</th>
                <th className={thCls}>Cursor</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {events.rows.map((r) => (
                <tr key={`${r.name}:${r.event}`}>
                  <td className={`${tdCls} font-mono text-ink`}>{r.name}</td>
                  <td className={`${tdCls} font-mono text-ink-dim`}>{r.event}</td>
                  <td className={`${tdCls} font-mono text-ink-dim`}>{r.sources.join(', ') || '—'}</td>
                  <td className={`${tdCls} text-ink-mute`} title={r.cursor ?? undefined}>
                    {r.cursor ? relativeTime(r.cursor) : 'not polled yet'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </FieldGroup>

      <div className="flex flex-wrap items-center gap-2 border-t border-border pt-4">
        <span className="text-note text-ink-mute">
          {tick.enabled
            ? `The cp loop ticks every ${tick.intervalSecs} s.`
            : 'The cp loop does not tick ([cp].cron_tick_enabled is off).'}
        </span>
        <span className="flex-1" />
        {state.kind === 'started' && (
          <StatusChip tone="running" dot>
            tick started
          </StatusChip>
        )}
        <Button variant="secondary" onClick={() => void runTick(true)} disabled={busy}>
          {busy && state.dry ? 'Checking…' : 'Dry run'}
        </Button>
        <Button variant="solid" onClick={() => void runTick(false)} disabled={busy}>
          {busy && !state.dry ? 'Ticking…' : 'Tick now'}
        </Button>
      </div>
      {state.kind === 'started' && (
        <p className="-mt-3 text-right text-note text-ink-dim">
          Due workflows fire in the background; their runs appear under Activity.
        </p>
      )}
      {state.kind === 'error' && (
        <p role="alert" className="-mt-3 text-right text-note text-err">
          {state.message}
        </p>
      )}
      {state.kind === 'dry' && (
        <div
          role="status"
          aria-label="Dry run result"
          className="rounded-lg border border-border bg-surface px-4 py-3 text-note text-ink-dim"
        >
          {state.wouldFire.length === 0 ? (
            'Dry run: nothing is due.'
          ) : (
            <>
              <p className="font-semibold text-ink">
                Would fire {state.wouldFire.length} workflow{state.wouldFire.length === 1 ? '' : 's'}
              </p>
              <ul className="mt-1.5 space-y-1">
                {state.wouldFire.map((w, i) => (
                  <li key={i} className="flex flex-wrap gap-2">
                    <StatusChip tone="neutral">{w.tier}</StatusChip>
                    <span className="font-mono text-ink">{w.workflow}</span>
                    {w.event && <span className="font-mono">{w.event}</span>}
                    {w.last_fired && <span className="text-ink-mute">last fired {relativeTime(w.last_fired)}</span>}
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}
    </div>
  );
}

export default AutomationTab;
