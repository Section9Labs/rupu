// useRecentActivity — the Overview's "what just happened" feed: the latest
// workflow runs across the fleet, any status, newest first. Read-only and
// glanceable — it gives the dashboard a sense of motion without being a second
// runs table to work. Backed by the existing per-host run list (no new API);
// the explicit status set is needed because the list defaults to running-only
// when a query names no status.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, apiErrorMessage, type CustomerScope, type RunListRow, type RunStatusStr } from '../api';

export interface RecentRun {
  id: string;
  host: string;
  codename: string;
  codenameDerived: boolean;
  workflowName: string;
  status: RunStatusStr;
  trigger: RunListRow['trigger'];
  /** Most relevant time: finished_at when terminal, else started_at. */
  at: string | null;
  running: boolean;
}

const PER_HOST = 8;
const CAP = 6;
const RECONCILE_MS = 60_000;
const ALL_STATUSES =
  'status:running,completed,failed,awaiting_approval,paused,cancelled,rejected';

const TERMINAL: ReadonlySet<RunStatusStr> = new Set([
  'completed',
  'failed',
  'cancelled',
  'rejected',
]);

function toRecent(row: RunListRow, queriedHost: string): RecentRun {
  const terminal = TERMINAL.has(row.status);
  return {
    id: row.id,
    host: row.host_id ?? queriedHost,
    codename: row.codename,
    codenameDerived: row.codename_derived,
    workflowName: row.workflow_name,
    status: row.status,
    trigger: row.trigger,
    at: terminal ? row.finished_at ?? row.started_at : row.started_at,
    running: row.status === 'running',
  };
}

function atMs(r: RecentRun): number {
  return r.at ? Date.parse(r.at) : 0;
}

export function useRecentActivity(customer?: CustomerScope) {
  const [runs, setRuns] = useState<RecentRun[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const customerRef = useRef(customer);
  customerRef.current = customer;
  const genRef = useRef(0);

  const load = useCallback(() => {
    const gen = ++genRef.current;
    const scoped = customerRef.current;
    api.getRegisteredHosts().then(
      async (hosts) => {
        if (genRef.current !== gen) return;
        const reqs = hosts.map((h) => {
          const base = scoped ? { host: h.id, customer: scoped } : { host: h.id };
          return api
            .getWorkflowRuns({ ...base, q: ALL_STATUSES, limit: PER_HOST })
            .then((rows) => rows.map((r) => toRecent(r, h.id)))
            .catch(() => [] as RecentRun[]);
        });
        const settled = await Promise.all(reqs);
        if (genRef.current !== gen) return;
        const merged = settled
          .flat()
          .filter((it, i, arr) => arr.findIndex((o) => o.id === it.id && o.host === it.host) === i)
          .sort((a, b) => atMs(b) - atMs(a))
          .slice(0, CAP);
        setRuns(merged);
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
    const unsubscribe = api.subscribeEvents(() => load());
    const poll = setInterval(() => {
      if (document.visibilityState === 'visible') load();
    }, RECONCILE_MS);
    return () => {
      unsubscribe();
      clearInterval(poll);
    };
  }, [load, customer]);

  return useMemo(() => ({ runs, loading, error, refresh: load }), [runs, loading, error, load]);
}
