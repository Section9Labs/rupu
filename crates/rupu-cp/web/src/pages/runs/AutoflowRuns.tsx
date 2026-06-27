// Autoflow run-stream page — leads with individual launched runs (clickable),
// not opaque batch cycle ticks. A secondary "Cycles" tab keeps the batch view.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Inbox, RefreshCw } from 'lucide-react';
import {
  api,
  type AutoflowClaim,
  type AutoflowCycleRow,
  type AutoflowEventRow,
} from '../../lib/api';
import { ListCard } from '../../components/lists/ListCard';
import { SectionHeader } from '../../components/lists/SectionHeader';
import UsageChip from '../../components/UsageChip';
import { cn } from '../../lib/cn';
import { durationBetween, relativeTime } from '../../lib/time';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';

const PAGE = 20;

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

const MODE_CLS: Record<string, string> = {
  ask:       'bg-amber-50 text-amber-700 ring-amber-200',
  bypass:    'bg-green-50 text-green-700 ring-green-200',
  readonly:  'bg-slate-100 text-slate-600 ring-slate-200',
  tick:      'bg-slate-100 text-slate-600 ring-slate-200',
  serve:     'bg-sky-50 text-sky-700 ring-sky-200',
};

function ModeChip({ mode }: { mode: string }) {
  const cls = MODE_CLS[mode] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {mode}
    </span>
  );
}

// Per-kind badge styling + human label for the events view.
const KIND_CLS: Record<string, string> = {
  run_launched:     'bg-green-50 text-green-700 ring-green-200',
  awaiting_human:   'bg-amber-50 text-amber-700 ring-amber-200',
  awaiting_external:'bg-sky-50 text-sky-700 ring-sky-200',
  cycle_failed:     'bg-red-50 text-red-700 ring-red-200',
};

const KIND_LABEL: Record<string, string> = {
  run_launched:     'launched',
  awaiting_human:   'awaiting human',
  awaiting_external:'awaiting external',
  cycle_failed:     'failed',
};

function KindBadge({ kind }: { kind: string }) {
  const cls = KIND_CLS[kind] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  const label = KIND_LABEL[kind] ?? kind.replace(/_/g, ' ');
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {label}
    </span>
  );
}

function IssueChip({ displayRef }: { displayRef: string }) {
  return (
    <span className="inline-flex items-center rounded bg-slate-100 text-slate-600 ring-1 ring-slate-200 text-[10px] font-medium px-1.5 py-0.5">
      {displayRef}
    </span>
  );
}

// Title-case a snake_case status (e.g. `await_human` → `Await Human`).
function titleCase(s: string): string {
  return s
    .split('_')
    .filter(Boolean)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(' ');
}

// Per-status badge styling for the claim lifecycle.
const CLAIM_STATUS_CLS: Record<string, string> = {
  await_human: 'bg-amber-50 text-amber-700 ring-amber-200',
  running:     'bg-blue-50 text-blue-700 ring-blue-200',
  blocked:     'bg-red-50 text-red-700 ring-red-200',
  complete:    'bg-green-50 text-green-700 ring-green-200',
  released:    'bg-slate-100 text-slate-600 ring-slate-200',
};

function ClaimStatusBadge({ status }: { status: string }) {
  const cls = CLAIM_STATUS_CLS[status] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {titleCase(status)}
    </span>
  );
}

type Tab = 'runs' | 'cycles' | 'claims';

export default function AutoflowRuns() {
  const [tab, setTab] = useState<Tab>('runs');
  const [events, setEvents] = useState<AutoflowEventRow[] | null>(null);
  const [cycles, setCycles] = useState<AutoflowCycleRow[] | null>(null);
  const [eventsHasMore, setEventsHasMore] = useState(true);
  const [cyclesHasMore, setCyclesHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  // Claims tab: lazily fetched on selection (cancel-guarded). `null` = loading.
  const [claims, setClaims] = useState<AutoflowClaim[] | null>(null);
  const [claimsError, setClaimsError] = useState<string | null>(null);

  // Manual refetch after a row action mutates the claim set.
  const refetchClaims = useCallback(async () => {
    try {
      const rows = await api.getAutoflowClaims();
      setClaims(rows);
      setClaimsError(null);
    } catch (e) {
      setClaimsError(e instanceof Error ? e.message : 'Failed to load autoflow claims');
    }
  }, []);

  useEffect(() => {
    if (tab !== 'claims') return;
    let cancelled = false;
    setClaims(null);
    setClaimsError(null);
    void (async () => {
      try {
        const rows = await api.getAutoflowClaims();
        if (!cancelled) setClaims(rows);
      } catch (e) {
        if (!cancelled) {
          setClaimsError(e instanceof Error ? e.message : 'Failed to load autoflow claims');
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tab]);

  // Page-0 fetch (mount + 5 s refresh). Only replaces a list when the user
  // hasn't scroll-extended past page 0 — otherwise the poll would discard
  // accumulated pages and cause the reset/regrow flicker.
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const [ev, cy] = await Promise.all([
        api.getAutoflowEvents({ limit: PAGE }),
        api.getAutoflowRuns({ limit: PAGE }),
      ]);
      // Functional setState so the guard reads the CURRENT length, not a
      // stale closure (refresh is memoised with [] deps).
      setEvents((prev) => {
        if (prev == null || prev.length <= PAGE) {
          setEventsHasMore(ev.length >= PAGE);
          return ev;
        }
        return prev;
      });
      setCycles((prev) => {
        if (prev == null || prev.length <= PAGE) {
          setCyclesHasMore(cy.length >= PAGE);
          return cy;
        }
        return prev;
      });
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load autoflow activity');
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(t);
  }, [refresh]);

  // Two independent infinite lists: the "Launched runs" events feed and the
  // "Cycles" feed each get their own pagination state + sentinel.
  const loadMoreEvents = async () => {
    const current = events ?? [];
    const next = await api.getAutoflowEvents({ offset: current.length, limit: PAGE });
    if (next.length === 0) { setEventsHasMore(false); return; }
    setEvents([...current, ...next]);
    if (next.length < PAGE) setEventsHasMore(false);
  };

  const loadMoreCycles = async () => {
    const current = cycles ?? [];
    const next = await api.getAutoflowRuns({ offset: current.length, limit: PAGE });
    if (next.length === 0) { setCyclesHasMore(false); return; }
    setCycles([...current, ...next]);
    if (next.length < PAGE) setCyclesHasMore(false);
  };

  const { sentinelRef: eventsSentinelRef, loading: eventsLoading } =
    useInfiniteScroll({ hasMore: eventsHasMore, loadMore: loadMoreEvents });
  const { sentinelRef: cyclesSentinelRef, loading: cyclesLoading } =
    useInfiniteScroll({ hasMore: cyclesHasMore, loadMore: loadMoreCycles });

  const sortedEvents = [...(events ?? [])].sort(
    (a, b) => Date.parse(b.at) - Date.parse(a.at),
  );
  const sortedCycles = [...(cycles ?? [])].sort(
    (a, b) => Date.parse(b.started_at) - Date.parse(a.started_at),
  );

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Autoflows</h1>
          <p className="mt-1 text-sm text-ink-dim">Runs launched by the autoflow worker across this control plane.</p>
        </div>
        <button
          onClick={() => void refresh()}
          className="inline-flex items-center gap-1.5 text-xs font-medium px-3 py-1.5 rounded-md border border-border bg-panel text-ink hover:bg-slate-100"
        >
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </button>
      </header>

      <div className="mb-5 inline-flex rounded-md border border-border bg-panel p-0.5 text-xs font-medium">
        <button
          onClick={() => setTab('runs')}
          className={cn(
            'px-3 py-1 rounded',
            tab === 'runs' ? 'bg-slate-100 text-ink' : 'text-ink-dim hover:text-ink',
          )}
        >
          Launched runs
        </button>
        <button
          onClick={() => setTab('cycles')}
          className={cn(
            'px-3 py-1 rounded',
            tab === 'cycles' ? 'bg-slate-100 text-ink' : 'text-ink-dim hover:text-ink',
          )}
        >
          Cycles
        </button>
        <button
          onClick={() => setTab('claims')}
          className={cn(
            'px-3 py-1 rounded',
            tab === 'claims' ? 'bg-slate-100 text-ink' : 'text-ink-dim hover:text-ink',
          )}
        >
          Claims
        </button>
      </div>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {tab === 'runs' ? (
        events === null ? (
          <div className="text-sm text-ink-dim">Loading autoflow activity…</div>
        ) : sortedEvents.length === 0 ? (
          <AutoflowEventsEmpty />
        ) : (
          <section>
            <SectionHeader tone="muted" label="Activity" count={sortedEvents.length} />
            <ListCard>
              {sortedEvents.map((e) => (
                <AutoflowEventItem key={e.event_id} event={e} />
              ))}
            </ListCard>
            {sortedEvents.length > 0 && (
              <div ref={eventsSentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
                {eventsLoading ? 'loading more…' : eventsHasMore ? 'scroll for more' : `— end of ${sortedEvents.length} —`}
              </div>
            )}
          </section>
        )
      ) : tab === 'cycles' ? (
        cycles === null ? (
          <div className="text-sm text-ink-dim">Loading autoflow cycles…</div>
        ) : sortedCycles.length === 0 ? (
          <AutoflowCyclesEmpty />
        ) : (
          <section>
            <SectionHeader tone="muted" label="Cycles" count={sortedCycles.length} />
            <ListCard>
              {sortedCycles.map((c) => (
                <AutoflowCycleItem key={c.cycle_id} cycle={c} />
              ))}
            </ListCard>
            {sortedCycles.length > 0 && (
              <div ref={cyclesSentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
                {cyclesLoading ? 'loading more…' : cyclesHasMore ? 'scroll for more' : `— end of ${sortedCycles.length} —`}
              </div>
            )}
          </section>
        )
      ) : claimsError ? (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {claimsError}
        </div>
      ) : claims === null ? (
        <div className="text-sm text-ink-dim">Loading autoflow claims…</div>
      ) : claims.length === 0 ? (
        <AutoflowClaimsEmpty />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Claims" count={claims.length} />
          <ListCard>
            {claims.map((c) => (
              <AutoflowClaimItem
                key={c.issue_ref}
                claim={c}
                onChanged={() => void refetchClaims()}
              />
            ))}
          </ListCard>
        </section>
      )}
    </div>
  );
}

function AutoflowClaimItem({
  claim,
  onChanged,
}: {
  claim: AutoflowClaim;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<'requeue' | 'release' | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [requeued, setRequeued] = useState(false);

  const label = claim.issue_display_ref ?? claim.issue_ref;

  const onRequeue = async () => {
    if (busy) return;
    if (!window.confirm('Requeue this autoflow?')) return;
    setBusy('requeue');
    setActionError(null);
    try {
      await api.requeueClaim(claim.issue_ref);
      setRequeued(true);
      onChanged();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Failed to requeue claim');
    } finally {
      setBusy(null);
    }
  };

  const onRelease = async () => {
    if (busy) return;
    if (!window.confirm('Release this claim?')) return;
    setBusy('release');
    setActionError(null);
    try {
      await api.releaseClaim(claim.issue_ref);
      onChanged();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Failed to release claim');
      setBusy(null);
    }
  };

  return (
    <div className="flex items-start gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          {claim.issue_url ? (
            <a
              href={claim.issue_url}
              target="_blank"
              rel="noreferrer"
              className="text-sm font-medium text-brand-600 hover:underline truncate"
            >
              {label}
            </a>
          ) : (
            <span className="text-sm font-medium text-ink truncate">{label}</span>
          )}
          <ClaimStatusBadge status={claim.status} />
          <IssueChip displayRef={claim.workflow} />
          {requeued && (
            <span className="text-[10px] font-medium text-green-700">requeued</span>
          )}
        </div>
        {claim.issue_title && (
          <div className="text-[11px] text-ink-dim mt-0.5 truncate">{claim.issue_title}</div>
        )}
        <div className="text-[11px] text-ink-dim mt-0.5">
          {claim.repo_ref}
          {' · '}updated {relativeTime(claim.updated_at)}
          {claim.claim_owner && <> · {claim.claim_owner}</>}
        </div>
        {claim.last_error && (
          <div className="text-[11px] text-red-600 mt-1">{claim.last_error}</div>
        )}
        {!claim.last_error && claim.last_summary && (
          <div className="text-[11px] text-ink-dim mt-1">{claim.last_summary}</div>
        )}
        {claim.pr_url && (
          <a
            href={claim.pr_url}
            target="_blank"
            rel="noreferrer"
            className="inline-block text-[11px] text-brand-600 hover:underline mt-1"
          >
            View PR
          </a>
        )}
        {actionError && (
          <div role="alert" className="text-[11px] text-red-600 mt-1">
            {actionError}
          </div>
        )}
      </div>
      <div className="flex items-center gap-2 shrink-0">
        <button
          onClick={() => void onRequeue()}
          disabled={busy !== null}
          className="inline-flex items-center text-xs font-medium px-2.5 py-1 rounded-md border border-border bg-panel text-ink hover:bg-slate-100 disabled:opacity-50"
        >
          {busy === 'requeue' ? 'Requeuing…' : 'Requeue'}
        </button>
        <button
          onClick={() => void onRelease()}
          disabled={busy !== null}
          className="inline-flex items-center text-xs font-medium px-2.5 py-1 rounded-md border border-red-200 bg-red-50 text-red-700 hover:bg-red-100 disabled:opacity-50"
        >
          {busy === 'release' ? 'Releasing…' : 'Release'}
        </button>
      </div>
    </div>
  );
}

function AutoflowClaimsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No active claims</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Issues the autoflow worker has leased will appear here, each with requeue and release controls.
      </p>
    </div>
  );
}

function AutoflowEventItem({ event }: { event: AutoflowEventRow }) {
  const headline = event.workflow ?? KIND_LABEL[event.kind] ?? event.kind.replace(/_/g, ' ');
  const body = (
    <div className="flex items-start gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-ink truncate">{headline}</span>
          <KindBadge kind={event.kind} />
          {event.issue_display_ref && <IssueChip displayRef={event.issue_display_ref} />}
        </div>
        <div className="text-[11px] text-ink-dim mt-0.5 flex items-center flex-wrap">
          <span>
            {relativeTime(event.at)}
            {event.worker_name && <> · {event.worker_name}</>}
            {event.status && <> · {event.status}</>}
            {event.run_id && <> · <span className="font-mono">{shortId(event.run_id)}</span></>}
          </span>
          <UsageChip usage={event.usage} className="ml-2" />
        </div>
      </div>
    </div>
  );

  if (event.run_id) {
    return (
      <Link
        to={`/runs/${encodeURIComponent(event.run_id)}`}
        className="block hover:bg-slate-50"
      >
        {body}
      </Link>
    );
  }
  return body;
}

function AutoflowCycleItem({ cycle }: { cycle: AutoflowCycleRow }) {
  const hasFailed = cycle.failed_cycles > 0;
  return (
    <div className="flex items-start gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-ink font-mono">{shortId(cycle.cycle_id)}</span>
          <ModeChip mode={cycle.mode} />
          {cycle.worker_name && (
            <span className="text-[11px] text-ink-mute">{cycle.worker_name}</span>
          )}
        </div>
        <div className="text-[11px] text-ink-dim mt-0.5">
          started {relativeTime(cycle.started_at)}
          {' · '}
          {durationBetween(cycle.started_at, cycle.finished_at)}
        </div>
        <div className={cn('text-[11px] mt-1', hasFailed ? 'text-red-600' : 'text-ink-dim')}>
          ran {cycle.ran_cycles}
          {' · '}
          skipped {cycle.skipped_cycles}
          {hasFailed && (
            <span className="text-red-600"> · failed {cycle.failed_cycles}</span>
          )}
          {' '}
          of {cycle.workflow_count}
          <UsageChip usage={cycle.usage} className="ml-2" />
        </div>
        {cycle.run_ids.length > 0 && (
          <div className="flex items-center gap-1.5 flex-wrap mt-1.5">
            <span className="text-[10px] text-ink-mute uppercase tracking-wide">runs:</span>
            {cycle.run_ids.map((rid) => (
              <Link
                key={rid}
                to={`/runs/${encodeURIComponent(rid)}`}
                className="text-[11px] font-mono text-brand-600 hover:underline"
              >
                {shortId(rid)}
              </Link>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function AutoflowEventsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No autoflow activity yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Runs launched by the autoflow worker will appear here, each linking to its run graph.
      </p>
    </div>
  );
}

function AutoflowCyclesEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No autoflow cycles yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Autoflow scheduling cycles will appear here once the autoflow worker runs.
      </p>
    </div>
  );
}
