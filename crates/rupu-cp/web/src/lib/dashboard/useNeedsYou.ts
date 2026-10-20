// useNeedsYou — the dashboard "Needs you" triage list, loaded across the fleet
// from EXISTING endpoints (no new API). It answers the dashboard's real job:
// "what is blocked on me?" — parked approval gates and paused runs (which the
// operator can act on inline) plus recent failures (which link out to the run).
//
// Per registered host it runs two cheap list reads: the blocking set
// (`status:awaiting_approval,paused`) and recent failures (`status:failed`,
// small limit). Results merge across hosts, sort blocking-first (oldest wait on
// top) then failures (most recent), and cap. A host that errors (e.g. a remote
// 501 under a customer filter) is skipped, not fatal.
//
// Inline actions call the SAME mutations RunDetail uses, each only where it is
// real: approve/reject an awaiting gate (`approveRun`/`rejectRun`), resume a
// paused run (`resumeRun`). A failed run has no web "retry" endpoint, so it has
// no mutation here — it links to the run. Mutations are pending-state, never
// optimistic: a 200 means recorded; the item leaves the list only when a
// refetch no longer returns it.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  apiErrorMessage,
  type CustomerScope,
  type RunListRow,
  type RunStatusStr,
} from '../api';

/** A run that needs the operator. */
export interface NeedsYouItem {
  id: string;
  host: string;
  codename: string;
  codenameDerived: boolean;
  workflowName: string;
  status: RunStatusStr;
  kind: 'awaiting' | 'paused' | 'failed';
  /** RFC-3339 the wait/failure began (approx): started_at, or finished_at for failed. */
  since: string | null;
}

export type NeedsYouVerb = 'approve' | 'reject' | 'resume';

export interface UseNeedsYou {
  items: NeedsYouItem[];
  loading: boolean;
  error: string | null;
  /** Per-run-id in-flight action (hides the row's buttons while it settles). */
  pending: Record<string, NeedsYouVerb>;
  approve: (item: NeedsYouItem) => void;
  reject: (item: NeedsYouItem) => void;
  resume: (item: NeedsYouItem) => void;
  refresh: () => void;
}

const BLOCKING_LIMIT = 10;
const FAILED_LIMIT = 4;
const CAP = 8;
const RECONCILE_MS = 60_000;

/** Mutation host arg: remote host id, or undefined for local. */
function hostArg(host: string): string | undefined {
  return host && host !== 'local' ? host : undefined;
}

function kindOf(status: RunStatusStr): NeedsYouItem['kind'] | null {
  if (status === 'awaiting_approval') return 'awaiting';
  if (status === 'paused') return 'paused';
  if (status === 'failed') return 'failed';
  return null;
}

function toItem(row: RunListRow, queriedHost: string): NeedsYouItem | null {
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

/** Blocking first (oldest wait on top), then failures (most recent first). */
function rank(it: NeedsYouItem): number {
  if (it.kind === 'awaiting') return 0;
  if (it.kind === 'paused') return 1;
  return 2;
}
function sortItems(items: NeedsYouItem[]): NeedsYouItem[] {
  return [...items].sort((a, b) => {
    if (rank(a) !== rank(b)) return rank(a) - rank(b);
    const ta = a.since ? Date.parse(a.since) : 0;
    const tb = b.since ? Date.parse(b.since) : 0;
    // blocking: oldest first (ascending); failed: most recent first (descending)
    return a.kind === 'failed' ? tb - ta : ta - tb;
  });
}

export function useNeedsYou(customer?: CustomerScope): UseNeedsYou {
  const [items, setItems] = useState<NeedsYouItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<Record<string, NeedsYouVerb>>({});

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
              .catch(() => [] as (NeedsYouItem | null)[]),
            api
              .getWorkflowRuns({ ...base, q: 'status:failed', limit: FAILED_LIMIT })
              .then((rows) => rows.map((r) => toItem(r, h.id)))
              .catch(() => [] as (NeedsYouItem | null)[]),
          ];
        });
        const settled = await Promise.all(reqs);
        if (genRef.current !== gen) return;
        const merged = settled
          .flat()
          .filter((x): x is NeedsYouItem => x !== null)
          // de-dup a run that both queries could return (shouldn't, but cheap)
          .filter((it, i, arr) => arr.findIndex((o) => o.id === it.id && o.host === it.host) === i);
        setItems(sortItems(merged).slice(0, CAP));
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

  const act = useCallback(
    (item: NeedsYouItem, verb: NeedsYouVerb, run: () => Promise<void>) => {
      setPending((p) => ({ ...p, [item.id]: verb }));
      run().then(
        () => {
          // Recorded — reconcile from the server; the item drops when it no
          // longer matches. Clear pending after the refetch is kicked off.
          setPending((p) => {
            const { [item.id]: _drop, ...rest } = p;
            return rest;
          });
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
    (item: NeedsYouItem) => act(item, 'approve', () => api.approveRun(item.id, undefined, hostArg(item.host))),
    [act],
  );
  const reject = useCallback(
    (item: NeedsYouItem) =>
      act(item, 'reject', () => api.rejectRun(item.id, 'Rejected from dashboard', hostArg(item.host))),
    [act],
  );
  const resume = useCallback(
    (item: NeedsYouItem) => act(item, 'resume', () => api.resumeRun(item.id, hostArg(item.host))),
    [act],
  );

  return useMemo(
    () => ({ items, loading, error, pending, approve, reject, resume, refresh: load }),
    [items, loading, error, pending, approve, reject, resume, load],
  );
}
