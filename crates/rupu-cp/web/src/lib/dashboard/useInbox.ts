// useInbox — the Inbox board's data engine. It answers the screen's one
// question, "what is waiting on me, and can I clear it?", by loading the fleet's
// blocked work from EXISTING endpoints (no new API), then letting the operator
// drive each item to zero: ACT on it (the real mutation), SNOOZE it (hide for a
// while), or DISMISS it (clear from view). Snooze/dismiss are per-operator view
// state in `localStorage` (inboxStore), never a server fact.
//
// Per registered host it runs two cheap list reads — the blocking set
// (`status:awaiting_approval,paused`) and recent failures (`status:failed`) —
// and merges them. The split the screen draws is honest about the backend:
//   • ACTIONABLE (gates + paused) have a real mutation (approve/reject/resume);
//   • FAILURES can be retried (local runs, the run's recorded mode), opened or
//     cleared; a retry is a request, so the row shows it until the run's
//     record says the resume took it (its marker consumed, a new failure time,
//     or the run gone from the failed list), or it outlives RETRY_STALL_MS,
//     which drops it with a message and offers Retry again.
//
// The "stalled run time" headline (sum of how long each actionable item has
// been waiting) is derived here from the same `since` timestamps, and only ever
// falls when the operator acts — the number that makes the board a ledger, not
// a list. A ticking `now` keeps ages, the stalled total and snooze expiry live.
//
// Mutations are pending-state, never optimistic: a 200 means recorded; the item
// leaves only when a refetch no longer returns it. Snooze/dismiss ARE applied
// immediately — they're a view preference, not a claim about the run.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  apiErrorMessage,
  type CustomerScope,
  type RunListRow,
  type RunStatusStr,
} from '../api';
import { isAgentRunRecord, isRetryable, RETRY_STALL_MS, RETRY_STALLED_MESSAGE, runsApi } from '../api3b/runs';
import {
  activeSnoozes,
  addDismissed,
  addSnooze,
  readDismissed,
  removeDismissed,
  removeSnooze,
  snoozeUntil,
  type SnoozeDuration,
} from './inboxStore';

export type InboxKind = 'awaiting' | 'paused' | 'failed';
export type InboxVerb = 'approve' | 'reject' | 'resume' | 'retry';

/** A run that is blocked on, or recently failed under, the operator. */
export interface InboxItem {
  id: string;
  host: string;
  codename: string;
  codenameDerived: boolean;
  workflowName: string;
  status: RunStatusStr;
  kind: InboxKind;
  /** RFC-3339 the wait/failure began (approx): started_at, or finished_at for failed. */
  since: string | null;
}

export interface SnoozedEntry {
  item: InboxItem;
  until: number;
}

export interface UseInbox {
  /** Gates + paused runs you can clear here, oldest wait first. */
  actionable: InboxItem[];
  /** Recent failures, most recent first. */
  failures: InboxItem[];
  /** Items currently snoozed that still match live data, soonest-return first. */
  snoozed: SnoozedEntry[];
  /** Dismissed items that still match live data (for "show dismissed"). */
  dismissed: InboxItem[];
  /** Visible counts by kind (snoozed/dismissed excluded). */
  counts: { gates: number; paused: number; failed: number };
  /** Total visible actionable wait time, ms (the stalled-run-time headline). */
  stalledMs: number;
  /** The oldest visible actionable item, or null. */
  oldestActionable: InboxItem | null;
  /** How many items this operator has cleared (acted or dismissed) this session. */
  clearedThisSession: number;
  loading: boolean;
  error: string | null;
  /** Per-run-id in-flight ACT (hides the row's act buttons while it settles). */
  pending: Record<string, InboxVerb>;
  /** Live clock the view reads for ages/the stalled total. */
  now: number;
  approve: (item: InboxItem) => void;
  reject: (item: InboxItem) => void;
  resume: (item: InboxItem) => void;
  /** Retry a failed LOCAL run in its recorded mode (`POST /api/runs/:id/retry`). */
  retry: (item: InboxItem) => void;
  /** Failed runs whose retry was recorded and that a reload still lists as failed. */
  retryRequested: Record<string, true>;
  /** A refused retry's message, per run id (cleared by the next retry of it). */
  retryErrors: Record<string, string>;
  snooze: (item: InboxItem, dur: SnoozeDuration) => void;
  dismiss: (item: InboxItem) => void;
  unsnooze: (id: string) => void;
  undismiss: (id: string) => void;
  refresh: () => void;
}

const BLOCKING_LIMIT = 25;
const FAILED_LIMIT = 10;
const RECONCILE_MS = 60_000;
/** Faster reconcile while a requested retry waits to take its run. */
const RETRY_RECONCILE_MS = 5_000;
const TICK_MS = 30_000;

/** Mutation host arg: remote host id, or undefined for local. */
function hostArg(host: string): string | undefined {
  return host && host !== 'local' ? host : undefined;
}

function kindOf(status: RunStatusStr): InboxKind | null {
  if (status === 'awaiting_approval') return 'awaiting';
  if (status === 'paused') return 'paused';
  if (status === 'failed') return 'failed';
  return null;
}

function toItem(row: RunListRow, queriedHost: string): InboxItem | null {
  const kind = kindOf(row.status);
  if (!kind) return null;
  return {
    id: row.id,
    host: row.host_id ?? queriedHost,
    codename: row.codename,
    codenameDerived: row.codename_derived,
    workflowName: row.workflow_name,
    status: row.status,
    kind,
    since: kind === 'failed' ? row.finished_at ?? row.started_at : row.started_at,
  };
}

function sinceMs(item: InboxItem): number {
  return item.since ? Date.parse(item.since) : 0;
}

/** A requested retry: when it was asked for, and the failure time the run
 *  had then (`InboxItem.since` of a failed row). */
interface RetryMark {
  at: number;
  failedAt: string | null;
}

export function useInbox(customer?: CustomerScope): UseInbox {
  const [allItems, setAllItems] = useState<InboxItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<Record<string, InboxVerb>>({});
  const [now, setNow] = useState(() => Date.now());
  const [cleared, setCleared] = useState(0);
  // A requested retry: when it was asked for, and the run's failure time then
  // (a new one means the retry ran and failed again).
  const [retryMarks, setRetryMarks] = useState<Record<string, RetryMark>>({});
  const [retryErrors, setRetryErrors] = useState<Record<string, string>>({});

  // View state (localStorage-backed). `now` at mount prunes lapsed snoozes.
  const [dismissedIds, setDismissedIds] = useState<Set<string>>(() => new Set(readDismissed()));
  const [snoozeMap, setSnoozeMap] = useState<Record<string, number>>(() =>
    activeSnoozes(Date.now()),
  );

  const customerRef = useRef(customer);
  customerRef.current = customer;
  const genRef = useRef(0);

  const load = useCallback(() => {
    const gen = ++genRef.current;
    const scoped = customerRef.current;
    setLoading(true);
    api.getRegisteredHosts().then(
      async (hosts) => {
        if (genRef.current !== gen) return;
        const reqs = hosts.flatMap((h) => {
          const base = scoped ? { host: h.id, customer: scoped } : { host: h.id };
          return [
            api
              .getWorkflowRuns({ ...base, q: 'status:awaiting_approval,paused', limit: BLOCKING_LIMIT })
              .then((rows) => rows.map((r) => toItem(r, h.id)))
              .catch(() => [] as (InboxItem | null)[]),
            api
              .getWorkflowRuns({ ...base, q: 'status:failed', limit: FAILED_LIMIT })
              .then((rows) => rows.map((r) => toItem(r, h.id)))
              .catch(() => [] as (InboxItem | null)[]),
          ];
        });
        const settled = await Promise.all(reqs);
        if (genRef.current !== gen) return;
        const merged = settled
          .flat()
          .filter((x): x is InboxItem => x !== null)
          .filter((it, i, arr) => arr.findIndex((o) => o.id === it.id && o.host === it.host) === i);
        setAllItems(merged);
        // A run gone from the failed list was taken by its retry.
        setRetryMarks((prev) => {
          const stillFailed = new Set(merged.filter((it) => it.kind === 'failed').map((it) => it.id));
          const kept = Object.entries(prev).filter(([id]) => stillFailed.has(id));
          return kept.length === Object.keys(prev).length ? prev : Object.fromEntries(kept);
        });
        setError(null);
        setLoading(false);
      },
      (e: unknown) => {
        if (genRef.current !== gen) return;
        setError(apiErrorMessage(e));
        setLoading(false);
      },
    );
  }, []);

  useEffect(() => {
    load();
    const poll = setInterval(() => {
      if (document.visibilityState === 'visible') load();
    }, RECONCILE_MS);
    return () => clearInterval(poll);
    // re-run when the customer scope changes
  }, [load, customer]);

  // Ticking clock: keeps ages, the stalled total and snooze expiry live, and
  // re-prunes lapsed snoozes so a returning item reappears without a refetch.
  useEffect(() => {
    const t = setInterval(() => {
      setNow(Date.now());
      setSnoozeMap((prev) => {
        const pruned = activeSnoozes(Date.now());
        // keep identity stable when nothing lapsed
        const sameSize = Object.keys(pruned).length === Object.keys(prev).length;
        return sameSize ? prev : pruned;
      });
    }, TICK_MS);
    return () => clearInterval(t);
  }, []);

  const act = useCallback(
    (item: InboxItem, verb: InboxVerb, run: () => Promise<void>) => {
      setPending((p) => ({ ...p, [item.id]: verb }));
      run().then(
        () => {
          setPending((p) => {
            const { [item.id]: _drop, ...rest } = p;
            return rest;
          });
          setCleared((c) => c + 1);
          load();
        },
        (e: unknown) => {
          setPending((p) => {
            const { [item.id]: _drop, ...rest } = p;
            return rest;
          });
          setError(apiErrorMessage(e));
        },
      );
    },
    [load],
  );

  const approve = useCallback(
    (item: InboxItem) => act(item, 'approve', () => api.approveRun(item.id, undefined, hostArg(item.host))),
    [act],
  );
  const reject = useCallback(
    (item: InboxItem) =>
      act(item, 'reject', () => api.rejectRun(item.id, 'Rejected from inbox', hostArg(item.host))),
    [act],
  );
  const resume = useCallback(
    (item: InboxItem) => act(item, 'resume', () => api.resumeRun(item.id, hostArg(item.host))),
    [act],
  );

  const retry = useCallback(
    (item: InboxItem) => {
      if (item.host !== 'local' || isAgentRunRecord(item.workflowName)) return;
      setRetryErrors(({ [item.id]: _drop, ...rest }) => rest);
      setPending((p) => ({ ...p, [item.id]: 'retry' }));
      runsApi.retryRun(item.id, {}).then(
        () => {
          setPending(({ [item.id]: _drop, ...rest }) => rest);
          setRetryMarks((r) => ({ ...r, [item.id]: { at: Date.now(), failedAt: item.since } }));
          setCleared((c) => c + 1);
        },
        (e: unknown) => {
          setPending(({ [item.id]: _drop, ...rest }) => rest);
          setRetryErrors((r) => ({ ...r, [item.id]: apiErrorMessage(e) }));
        },
      );
    },
    [],
  );

  // While a retry waits to take its run, read each such run's record faster
  // than the minute: the resume consuming its marker, a new failure time or a
  // status that isn't retryable confirms it (then the board reloads); one
  // that outlives RETRY_STALL_MS never started.
  const marksRef = useRef(retryMarks);
  marksRef.current = retryMarks;
  const anyRetryRequested = Object.keys(retryMarks).length > 0;
  useEffect(() => {
    if (!anyRetryRequested) return;
    let cancelled = false;
    const tick = async () => {
      if (document.visibilityState !== 'visible') return;
      const marks = Object.entries(marksRef.current);
      const reads = await Promise.all(
        marks.map(([id, mark]) =>
          api.getRun(id).then(
            ({ run }) => ({ id, mark, run }),
            () => ({ id, mark, run: null }),
          ),
        ),
      );
      if (cancelled) return;
      const taken: string[] = [];
      const stalled: string[] = [];
      for (const { id, mark, run } of reads) {
        if (run && (!isRetryable(run.status) || !run.resume_requested_at || (run.finished_at ?? run.started_at) !== mark.failedAt)) {
          taken.push(id);
        } else if (Date.now() - mark.at > RETRY_STALL_MS) {
          stalled.push(id);
        }
      }
      if (taken.length + stalled.length === 0) return;
      const done = new Set([...taken, ...stalled]);
      setRetryMarks((prev) => Object.fromEntries(Object.entries(prev).filter(([id]) => !done.has(id))));
      if (stalled.length > 0) {
        setRetryErrors((prev) => ({ ...prev, ...Object.fromEntries(stalled.map((id) => [id, RETRY_STALLED_MESSAGE])) }));
      }
      if (taken.length > 0) load();
    };
    const t = setInterval(() => void tick(), RETRY_RECONCILE_MS);
    return () => {
      cancelled = true;
      clearInterval(t);
    };
  }, [anyRetryRequested, load]);
  const retryRequested = useMemo(
    () => Object.fromEntries(Object.keys(retryMarks).map((id) => [id, true as const])) as Record<string, true>,
    [retryMarks],
  );

  const snooze = useCallback((item: InboxItem, dur: SnoozeDuration) => {
    const until = snoozeUntil(dur, Date.now());
    setSnoozeMap(addSnooze(item.id, until));
  }, []);
  const unsnooze = useCallback((id: string) => {
    setSnoozeMap(removeSnooze(id));
  }, []);
  const dismiss = useCallback((item: InboxItem) => {
    setDismissedIds(new Set(addDismissed(item.id)));
    setCleared((c) => c + 1);
  }, []);
  const undismiss = useCallback((id: string) => {
    setDismissedIds(new Set(removeDismissed(id)));
  }, []);

  // Partition live items by view state. Snooze wins over dismiss (a snoozed
  // item is coming back), and both hide an item from the working queue.
  const view = useMemo(() => {
    const isSnoozed = (id: string) => (snoozeMap[id] ?? 0) > now;
    const isDismissed = (id: string) => dismissedIds.has(id);

    const actionable = allItems
      .filter((it) => it.kind !== 'failed' && !isSnoozed(it.id) && !isDismissed(it.id))
      .sort((a, b) => sinceMs(a) - sinceMs(b)); // oldest wait first
    const failures = allItems
      .filter((it) => it.kind === 'failed' && !isSnoozed(it.id) && !isDismissed(it.id))
      .sort((a, b) => sinceMs(b) - sinceMs(a)); // most recent first
    const snoozed: SnoozedEntry[] = allItems
      .filter((it) => isSnoozed(it.id))
      .map((it) => ({ item: it, until: snoozeMap[it.id] }))
      .sort((a, b) => a.until - b.until);
    const dismissed = allItems.filter((it) => isDismissed(it.id) && !isSnoozed(it.id));

    const stalledMs = actionable.reduce((s, it) => s + Math.max(0, now - sinceMs(it)), 0);
    const counts = {
      gates: actionable.filter((it) => it.kind === 'awaiting').length,
      paused: actionable.filter((it) => it.kind === 'paused').length,
      failed: failures.length,
    };
    return {
      actionable,
      failures,
      snoozed,
      dismissed,
      stalledMs,
      counts,
      oldestActionable: actionable[0] ?? null,
    };
  }, [allItems, dismissedIds, snoozeMap, now]);

  return useMemo(
    () => ({
      ...view,
      clearedThisSession: cleared,
      loading,
      error,
      pending,
      now,
      approve,
      reject,
      resume,
      retry,
      retryRequested,
      retryErrors,
      snooze,
      dismiss,
      unsnooze,
      undismiss,
      refresh: load,
    }),
    [
      view,
      cleared,
      loading,
      error,
      pending,
      now,
      approve,
      reject,
      resume,
      retry,
      retryRequested,
      retryErrors,
      snooze,
      dismiss,
      unsnooze,
      undismiss,
      load,
    ],
  );
}
