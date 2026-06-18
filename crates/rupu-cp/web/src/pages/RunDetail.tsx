// Run detail — the live view. Owns the ONE SSE subscription for the run and
// fans the event stream down to both the RunGraph (live step status) and the
// RunEventFeed (scrolling timeline). The browser twin of rupu's live TUI run
// view.

import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowLeft, GitBranch, ListOrdered, Pause } from 'lucide-react';
import {
  api,
  isKnownRunEvent,
  type RunRecord,
  type StepResultRecord,
} from '../lib/api';
import { StatusPill } from '../components/StatusPill';
import { TabBar, TabButton } from '../components/TabBar';
import RunGraph from '../components/RunGraph';
import RunEventFeed, { type ConnectionState, type SeqEvent } from '../components/RunEventFeed';
import { emptyRunStatus, reduceRunStatus, type RunStatusState } from '../lib/runStatus';
import { absoluteTime } from '../lib/time';

const MAX_EVENTS = 2000;

type Tab = 'graph' | 'events';

export default function RunDetail() {
  const { id = '' } = useParams<{ id: string }>();

  const [run, setRun] = useState<RunRecord | null>(null);
  const [steps, setSteps] = useState<StepResultRecord[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>('graph');

  // Live state, fed by the single SSE subscription below.
  const [events, setEvents] = useState<SeqEvent[]>([]);
  const [live, setLive] = useState<RunStatusState>(emptyRunStatus);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  // The latest run status as overridden by run_completed / run_failed events.
  const [liveRunStatus, setLiveRunStatus] = useState<RunRecord['status'] | null>(null);
  // Monotonically-increasing sequence counter — stable key source for SeqEvent.
  const seqRef = useRef<number>(0);

  // Initial fetch of the persisted run + step records.
  useEffect(() => {
    let cancelled = false;
    setRun(null);
    setSteps([]);
    setLoadError(null);
    api
      .getRun(id)
      .then((res) => {
        if (cancelled) return;
        setRun(res.run);
        setSteps(res.steps);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setLoadError(e instanceof Error ? e.message : 'Failed to load run');
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  // ONE SSE subscription per open run — shared by graph + feed.
  const liveRef = useRef<RunStatusState>(emptyRunStatus());
  useEffect(() => {
    if (!id) return;
    setEvents([]);
    setLive(emptyRunStatus());
    liveRef.current = emptyRunStatus();
    seqRef.current = 0;
    setConnection('connecting');

    const unsubscribe = api.subscribeRunLog(
      id,
      (ev) => {
        setConnection('live');
        const seq = ++seqRef.current;
        setEvents((prev) => {
          const next = prev.length >= MAX_EVENTS ? prev.slice(prev.length - MAX_EVENTS + 1) : prev;
          return [...next, { seq, event: ev }];
        });
        const nextLive = reduceRunStatus(liveRef.current, ev);
        liveRef.current = nextLive;
        setLive(nextLive);
        if (isKnownRunEvent(ev)) {
          if (ev.type === 'run_completed') setLiveRunStatus(ev.status);
          else if (ev.type === 'run_failed') setLiveRunStatus('failed');
        }
      },
      () => setConnection('reconnecting'),
    );
    return unsubscribe;
  }, [id]);

  const agentByStepId = useMemo(() => {
    const m: Record<string, string> = {};
    for (const { event: ev } of events) {
      if (!isKnownRunEvent(ev)) continue;
      if ((ev.type === 'step_started' || ev.type === 'unit_started') && ev.agent) {
        m[ev.step_id] = ev.agent;
      }
    }
    return m;
  }, [events]);

  // Prefer the live-derived awaiting info; fall back to the persisted record.
  const awaiting =
    live.awaiting ??
    (run?.awaiting_step_id
      ? { stepId: run.awaiting_step_id, reason: run.approval_prompt ?? 'Awaiting approval' }
      : undefined);

  const effectiveStatus = liveRunStatus ?? run?.status ?? 'pending';

  if (loadError) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {loadError}
        </div>
      </div>
    );
  }

  if (!run) {
    return (
      <div className="p-8">
        <BackLink />
        <div className="mt-4 text-sm text-ink-dim">Loading run…</div>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="px-8 pt-6">
        <BackLink />
        <header className="mt-3 flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-3">
              <h1 className="text-2xl font-semibold text-ink truncate">{run.workflow_name}</h1>
              <StatusPill status={effectiveStatus} />
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-0.5 text-[11px] text-ink-dim">
              <span className="font-mono">{run.id}</span>
              <span>started {absoluteTime(run.started_at)}</span>
              {run.finished_at && <span>finished {absoluteTime(run.finished_at)}</span>}
            </div>
          </div>
        </header>

        {run.error_message && (
          <div className="mt-3 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
            {run.error_message}
          </div>
        )}

        {awaiting && (
          <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 flex items-start gap-3">
            <Pause size={16} className="text-amber-600 mt-0.5 shrink-0" />
            <div className="min-w-0">
              <div className="text-sm font-medium text-amber-800">
                Awaiting approval · <span className="font-mono">{awaiting.stepId}</span>
              </div>
              <p className="text-[12px] text-amber-700 mt-0.5 break-words">{awaiting.reason}</p>
              <p className="text-[11px] text-amber-600/80 mt-1">
                Approve / Reject controls arrive in a later phase — view only for now.
              </p>
            </div>
          </div>
        )}
      </div>

      <div className="mt-4">
        <TabBar>
          <TabButton active={tab === 'graph'} onClick={() => setTab('graph')} icon={GitBranch} label="Graph" />
          <TabButton active={tab === 'events'} onClick={() => setTab('events')} icon={ListOrdered} label="Events" />
        </TabBar>
      </div>

      <div className="flex-1 min-h-0 px-8 py-6">
        {tab === 'graph' ? (
          <RunGraph steps={steps} live={live} agentByStepId={agentByStepId} />
        ) : (
          <div className="h-full min-h-0">
            <RunEventFeed events={events} connection={connection} />
          </div>
        )}
      </div>
    </div>
  );
}

function BackLink() {
  return (
    <Link
      to="/runs"
      className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink"
    >
      <ArrowLeft size={14} />
      Runs
    </Link>
  );
}
