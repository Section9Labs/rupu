// Workers list — workers registered with this control plane, with their
// declared capabilities and last-seen freshness. No detail route. Read-only.

import { useEffect, useState } from 'react';
import { Server } from 'lucide-react';
import { api, type WorkerRecord } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import { SectionHeader } from '../components/lists/SectionHeader';
import { cn } from '../lib/cn';
import { relativeTime } from '../lib/time';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const STEP = 20;

// A worker is considered stale if it hasn't been seen in this many ms.
const STALE_MS = 5 * 60 * 1000;

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 10)}…` : id;
}

function isStale(lastSeen: string): boolean {
  const t = Date.parse(lastSeen);
  if (Number.isNaN(t)) return false;
  return Date.now() - t > STALE_MS;
}

export default function Workers() {
  const [workers, setWorkers] = useState<WorkerRecord[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(STEP);

  useEffect(() => {
    let cancelled = false;
    api
      .getWorkers()
      .then((data) => {
        if (cancelled) return;
        setWorkers(data);
        setVisible(STEP);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load workers');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const sorted = (workers ?? []).slice().sort((a, b) => a.name.localeCompare(b.name));
  const shown = sorted.slice(0, visible);
  const { sentinelRef } = useInfiniteScroll({
    hasMore: visible < sorted.length,
    loadMore: () => setVisible((v) => v + STEP),
  });

  return (
    <div className="p-8 max-w-5xl">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Workers</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Workers registered with this control plane — their kind, host, and declared capabilities.
        </p>
      </header>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {workers === null ? (
        <div className="text-sm text-ink-dim">Loading workers…</div>
      ) : sorted.length === 0 ? (
        <EmptyState />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Workers" count={sorted.length} />
          <ListCard>
            {shown.map((w) => (
              <WorkerRow key={w.worker_id} worker={w} />
            ))}
          </ListCard>
          {sorted.length > visible && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              scroll for more
            </div>
          )}
        </section>
      )}
    </div>
  );
}

function WorkerRow({ worker }: { worker: WorkerRecord }) {
  const stale = isStale(worker.last_seen_at);
  const caps = worker.capabilities ?? {};
  const backends = caps.backends ?? [];
  const scmHosts = caps.scm_hosts ?? [];
  const modes = caps.permission_modes ?? [];

  return (
    <div className="flex items-start gap-4 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-medium text-ink truncate">{worker.name}</span>
          <span className="text-[11px] text-ink-mute font-mono">{shortId(worker.worker_id)}</span>
          <Chip className="bg-slate-100 text-ink-mute ring-slate-200">{worker.kind}</Chip>
          <span className="text-[11px] text-ink-mute font-mono">{worker.host}</span>
          {stale && (
            <Chip className="bg-amber-50 text-amber-800 ring-amber-200">stale</Chip>
          )}
        </div>

        {/* Capabilities */}
        {(backends.length > 0 || scmHosts.length > 0 || modes.length > 0) && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
            {backends.map((b) => (
              <Chip key={`b-${b}`} className="bg-blue-50 text-blue-700 ring-blue-200">
                {b}
              </Chip>
            ))}
            {scmHosts.map((h) => (
              <Chip key={`s-${h}`} className="bg-violet-50 text-violet-700 ring-violet-200">
                {h}
              </Chip>
            ))}
            {modes.map((m) => (
              <Chip key={`m-${m}`} className="bg-green-50 text-green-700 ring-green-200">
                {m}
              </Chip>
            ))}
          </div>
        )}

        <div className="mt-1 flex flex-wrap items-center gap-x-3 text-[11px] text-ink-mute">
          <span>registered {relativeTime(worker.registered_at)}</span>
          <span className={cn(stale && 'text-amber-700')}>
            last seen {relativeTime(worker.last_seen_at)}
          </span>
          <span className="tabular-nums">v{worker.version}</span>
        </div>
      </div>
    </div>
  );
}

function Chip({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ring-1',
        className,
      )}
    >
      {children}
    </span>
  );
}

function EmptyState() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Server size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No workers registered</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Workers appear here once they register with this control plane.
      </p>
    </div>
  );
}
