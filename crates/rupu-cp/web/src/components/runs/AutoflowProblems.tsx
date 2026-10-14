// AutoflowProblems — the strip above Runs → Autoflows (spec 2026-10-14, CP
// autoflow runs as one table, decision 4): autoflow failures that never
// reached a workflow attempt — a repo whose autoflow config fails to load, a
// worker whose cycle crashed — one line per ongoing problem. The server
// bounds "ongoing" to the past hour (`GET /api/runs/autoflows/problems`);
// this component only loads and renders.
//
// Per host like the list it sits above: a picked host is asked alone; All
// hosts asks every registered host and concatenates, skipping one that fails
// (the list's own per-host strip says who is unavailable). Polled every 30 s.
// Renders nothing while there is nothing to show.

import { useEffect, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { api, type AutoflowProblem } from '../../lib/api';
import { relativeTime } from '../../lib/time';

const POLL_MS = 30_000;

async function loadProblems(host: string | null, signal: AbortSignal): Promise<AutoflowProblem[]> {
  if (host) return api.getAutoflowProblems({ host, signal });
  const hosts = await api.getRegisteredHosts();
  const settled = await Promise.allSettled(hosts.map((h) => api.getAutoflowProblems({ host: h.id, signal })));
  return settled
    .flatMap((r) => (r.status === 'fulfilled' ? r.value : []))
    .sort((a, b) => b.last_seen.localeCompare(a.last_seen));
}

export function AutoflowProblems({ host }: { host: string | null }) {
  const [problems, setProblems] = useState<AutoflowProblem[]>([]);

  useEffect(() => {
    const ctl = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      try {
        const next = await loadProblems(host, ctl.signal);
        if (!ctl.signal.aborted) setProblems(next);
      } catch {
        // A failed read keeps the last answer; the next tick tries again.
      }
      if (!ctl.signal.aborted) timer = setTimeout(() => void tick(), POLL_MS);
    };
    void tick();
    return () => {
      ctl.abort();
      if (timer) clearTimeout(timer);
    };
  }, [host]);

  if (problems.length === 0) return null;
  return (
    <div className="mb-4 rounded-xl border border-warn/30 bg-warn-bg px-4 py-3 space-y-2">
      {problems.map((p) => (
        <div key={`${p.host_id}:${p.subject_kind}:${p.subject}`} className="flex items-start gap-2 text-sm">
          <AlertTriangle size={14} className="mt-0.5 shrink-0 text-warn" aria-hidden />
          <div className="min-w-0 flex-1">
            <div className="text-ink">
              {p.subject_kind === 'repo' ? 'repo ' : 'worker '}
              <span className="font-mono">{p.subject}</span>
              {p.subject_kind === 'repo' ? ': autoflow cycle failed' : ': cycle crashed'}
            </div>
            {p.detail && (
              <div className="truncate text-note text-err" title={p.detail}>
                {p.detail}
              </div>
            )}
            <div className="text-note text-ink-dim">
              since {relativeTime(p.first_seen)} · {p.count} cycles · {p.host_id}
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

export default AutoflowProblems;
